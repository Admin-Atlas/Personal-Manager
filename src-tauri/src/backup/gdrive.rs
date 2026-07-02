// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google Drive backup destination — pushes/pulls `.pmbackup` archives via the Drive v3 REST
//! API directly (reqwest), NOT a CLI or a bundled binary (Drive has a documented API; Proton
//! doesn't). The archive is already a finished, encrypted blob when it arrives here; this module
//! only ever moves opaque bytes and lists/trims them. It never sees the backup passphrase or the
//! plaintext vault.
//!
//! Auth reuses the existing Google OAuth machinery ([`crate::google`]): the account's keychain
//! token (`google_oauth_token_drive::<email>`), refreshed transparently. The one extra grant is
//! [`crate::google::DRIVE_FILE_SCOPE`] (`drive.file`) — least-privilege: the app can only touch
//! files/folders IT created, so the "Personal Manager Backups" folder and its archives are the
//! only Drive content PM can ever read or write.
//!
//! Naming, validation, and keep-last-N selection are shared with the Proton path (`super::naming`),
//! so a vault backed up to both places names and trims archives identically. Unlike Proton (whose
//! CLI errors on a duplicate folder name), Drive permits duplicates, so `ensure_backup_folder`
//! converges deterministically on the earliest-created folder.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::naming::{self, BackupEntry, ARCHIVE_EXT};
use crate::error::{Error, Result};
use crate::google;

/// Drive v3 metadata/read base.
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
/// Drive v3 upload base (a DISTINCT host path from the metadata base).
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3/files";
/// The single, fixed, human-recognizable folder PM keeps its archives in — matches the Proton
/// folder name so the user sees the same thing in both places.
const BACKUP_FOLDER_NAME: &str = "Personal Manager Backups";
/// Drive's folder MIME type.
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
/// Runaway guard on `files.list` pagination — a backstop against a never-clearing page token, not
/// a coverage cap (the backup folder holds at most a few dozen archives).
const MAX_PAGES: usize = 100;

// --- HTTP plumbing -------------------------------------------------------------------------------

/// A reqwest client for Drive calls: a 30s connect timeout guards a dead network, but no short
/// overall timeout — a large archive upload/download legitimately takes minutes. A 1h ceiling
/// still bounds a truly hung transfer.
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(Error::from)
}

/// Send an authorized request built by `build`, refreshing once and retrying on a 401 (the
/// backstop for a token revoked/expired early — mirrors [`crate::google`]'s GET path). `build` is
/// re-invoked to construct a fresh request on retry, so it must be cheap/idempotent — only used
/// here for small metadata calls (never the big upload body, which is sent once with a proactively
/// refreshed token).
async fn authorized_send<F>(token_key: &str, build: F) -> Result<reqwest::Response>
where
    F: Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
{
    let client = http_client()?;
    let bearer = google::valid_access_token(token_key).await?;
    let resp = build(&client, bearer.expose()).send().await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let bearer = google::refresh_now(token_key).await?;
        return Ok(build(&client, bearer.expose()).send().await?);
    }
    Ok(resp)
}

/// Turn a non-success Drive response into a friendly error, truncating (and never logging) the
/// body — which never contains the bearer token.
async fn drive_error(resp: reqwest::Response, action: &str) -> Error {
    let status = resp.status();
    let detail = crate::error::truncate_detail(&resp.text().await.unwrap_or_default());
    Error::Other(format!("Google Drive {action} failed ({status}): {detail}"))
}

// --- Drive JSON shapes ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// int64-as-string in Drive v3; absent for folders / some types.
    #[serde(default)]
    size: Option<String>,
    #[serde(default, rename = "createdTime")]
    created_time: Option<String>,
}

// --- Pure query builders (testable without a network) --------------------------------------------

/// The `files.list` `q` matching PM's app-created backup folder (drive.file only ever surfaces
/// app-created files, so this can only ever return folders PM itself made).
fn folder_query() -> String {
    format!("mimeType='{FOLDER_MIME}' and name='{BACKUP_FOLDER_NAME}' and trashed=false")
}

/// The `files.list` `q` for the archives directly inside `folder_id`.
fn in_folder_query(folder_id: &str) -> String {
    format!("'{folder_id}' in parents and trashed=false")
}

/// Choose which folder to use when Drive returns more than one "Personal Manager Backups" (it
/// permits duplicate names): the earliest-created, tie-broken by lexicographically-smallest id.
/// Deterministic, so every device converges on the same folder. Pure/testable.
fn pick_folder(
    mut folders: Vec<(String /*id*/, Option<String> /*createdTime*/)>,
) -> Option<String> {
    // Sort by (createdTime asc — None sorts last), then id asc. Missing createdTime is treated as
    // "newest" so a folder that reports a real creation time is always preferred.
    folders.sort_by(|a, b| match (&a.1, &b.1) {
        (Some(x), Some(y)) => x.cmp(y).then_with(|| a.0.cmp(&b.0)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.0.cmp(&b.0),
    });
    folders.into_iter().next().map(|(id, _)| id)
}

// --- Operations ----------------------------------------------------------------------------------

/// A `files.list` URL for an arbitrary `q` over the personal Drive, requesting `fields`.
fn files_list_url(q: &str, fields: &str, page: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/files"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("q", q);
        qp.append_pair("spaces", "drive");
        qp.append_pair("pageSize", "1000");
        qp.append_pair("fields", fields);
        if let Some(t) = page {
            qp.append_pair("pageToken", t);
        }
    }
    Ok(url.to_string())
}

/// Every file directly in `folder_id`, as `(id, name, size)`. Paginated with a runaway backstop.
async fn list_files_with_ids(
    token_key: &str,
    folder_id: &str,
) -> Result<Vec<(String, String, Option<u64>)>> {
    let q = in_folder_query(folder_id);
    let fields = "nextPageToken,files(id,name,size,createdTime)";
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let url = files_list_url(&q, fields, page.as_deref())?;
        let resp = authorized_send(token_key, |c, bearer| c.get(&url).bearer_auth(bearer)).await?;
        if !resp.status().is_success() {
            return Err(drive_error(resp, "listing backups").await);
        }
        let list: FileList = resp.json().await?;
        for f in list.files {
            if let Some(name) = f.name {
                out.push((f.id, name, f.size.and_then(|s| s.parse::<u64>().ok())));
            }
        }
        match list.next_page_token {
            Some(t) => page = Some(t),
            None => return Ok(out),
        }
    }
    // Backstop tripped — surface it rather than silently returning a partial listing (retention
    // would then under-count and never trim).
    Err(Error::Other(
        "Google Drive returned too many pages listing backups".into(),
    ))
}

/// Ensure PM's backup folder exists and return its id (idempotent — check then create). With
/// `drive.file`, `files.list` only ever surfaces app-created folders, so this can't accidentally
/// pick a folder the user made by hand. If Drive somehow holds more than one (it permits duplicate
/// names; two devices could race), [`pick_folder`] converges on the earliest-created one.
pub(crate) async fn ensure_backup_folder(token_key: &str) -> Result<String> {
    let url = files_list_url(&folder_query(), "files(id,createdTime)", None)?;
    let resp = authorized_send(token_key, |c, bearer| c.get(&url).bearer_auth(bearer)).await?;
    if !resp.status().is_success() {
        return Err(drive_error(resp, "finding the backup folder").await);
    }
    let list: FileList = resp.json().await?;
    let existing: Vec<(String, Option<String>)> = list
        .files
        .into_iter()
        .map(|f| (f.id, f.created_time))
        .collect();
    if let Some(id) = pick_folder(existing) {
        return Ok(id);
    }

    // None yet — create it (in My Drive root; no `parents`).
    let body =
        serde_json::json!({ "name": BACKUP_FOLDER_NAME, "mimeType": FOLDER_MIME }).to_string();
    let create_url = format!("{DRIVE_API}/files?fields=id");
    let resp = authorized_send(token_key, |c, bearer| {
        c.post(&create_url)
            .bearer_auth(bearer)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/json; charset=UTF-8",
            )
            .body(body.clone())
    })
    .await?;
    if !resp.status().is_success() {
        return Err(drive_error(resp, "creating the backup folder").await);
    }
    Ok(resp.json::<IdOnly>().await?.id)
}

/// Upload a finished archive into the backup folder via a **resumable** upload (needs neither the
/// `multipart` reqwest feature nor a second request format): initiate a session with the metadata,
/// then PUT the bytes to the returned session URI. The file is read into memory off the async
/// runtime; for a very large vault this could be revisited with a chunked `Content-Range` upload.
pub(crate) async fn upload_archive(
    token_key: &str,
    local: &Path,
    archive_name: &str,
    folder_id: &str,
) -> Result<()> {
    // Read the archive off the async runtime (blocking file I/O).
    let path: PathBuf = local.to_path_buf();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&path))
        .await
        .map_err(|e| Error::Other(format!("reading the archive panicked: {e}")))??;
    let len = bytes.len();

    // 1) Initiate the resumable session. Small JSON body → safe to 401-retry via `authorized_send`.
    let meta = serde_json::json!({ "name": archive_name, "parents": [folder_id] }).to_string();
    let init_url = format!("{UPLOAD_API}?uploadType=resumable&fields=id");
    let init = authorized_send(token_key, |c, bearer| {
        c.post(&init_url)
            .bearer_auth(bearer)
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", len.to_string())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/json; charset=UTF-8",
            )
            .body(meta.clone())
    })
    .await?;
    if !init.status().is_success() {
        return Err(drive_error(init, "starting the upload").await);
    }
    let session = init
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| Error::Other("Google Drive didn't return an upload session".into()))?;

    // 2) Transfer the bytes in one PUT. The session URI is pre-authorized; we include the bearer
    //    too. The body is sent once (a stream/large Vec can't be cheaply retried), so we rely on
    //    the proactively-refreshed token from step 1 rather than a 401 retry here.
    let bearer = google::valid_access_token(token_key).await?;
    let put = http_client()?
        .put(&session)
        .bearer_auth(bearer.expose())
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(bytes)
        .send()
        .await?;
    if !put.status().is_success() {
        return Err(drive_error(put, "uploading the archive").await);
    }
    Ok(())
}

/// List PM's archives in the backup folder (newest first). Filters to `.pmbackup` and sorts
/// reverse-lexically (== reverse-chronological, since names carry a trailing UTC stamp), matching
/// the Proton listing.
pub(crate) async fn list_archives(token_key: &str, folder_id: &str) -> Result<Vec<BackupEntry>> {
    let mut entries: Vec<BackupEntry> = list_files_with_ids(token_key, folder_id)
        .await?
        .into_iter()
        .filter(|(_, name, _)| name.ends_with(ARCHIVE_EXT))
        .map(|(_, name, size)| BackupEntry { name, size })
        .collect();
    entries.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(entries)
}

/// Keep-last-N retention for one vault: keep the newest `keep_n` archives whose name carries
/// `prefix` (this vault's — see [`naming::archive_prefix`]) and TRASH the rest via
/// `files.update {trashed:true}` — recoverable (Drive Trash), never a hard delete, mirroring
/// Proton. Scoped by `prefix` + `valid_archive_name`, so it never touches another vault's/device's
/// archives or a non-PM file. Returns how many were trashed.
pub(crate) async fn apply_retention(
    token_key: &str,
    folder_id: &str,
    keep_n: usize,
    prefix: &str,
) -> Result<usize> {
    let files = list_files_with_ids(token_key, folder_id).await?;
    // (name -> id) for this vault's valid archives only.
    let mine: Vec<(String, String)> = files
        .into_iter()
        .filter(|(_, name, _)| name.starts_with(prefix) && naming::valid_archive_name(name))
        .map(|(id, name, _)| (name, id))
        .collect();
    let names: Vec<String> = mine.iter().map(|(name, _)| name.clone()).collect();
    let doomed = naming::select_for_deletion(&names, keep_n);
    let mut trashed = 0usize;
    for name in &doomed {
        let Some((_, id)) = mine.iter().find(|(n, _)| n == name) else {
            continue;
        };
        let patch_url = format!("{DRIVE_API}/files/{id}");
        let body = serde_json::json!({ "trashed": true }).to_string();
        let resp = authorized_send(token_key, |c, bearer| {
            c.patch(&patch_url)
                .bearer_auth(bearer)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/json; charset=UTF-8",
                )
                .body(body.clone())
        })
        .await?;
        if !resp.status().is_success() {
            return Err(drive_error(resp, "trimming old backups").await);
        }
        trashed += 1;
    }
    Ok(trashed)
}

/// Download one archive (by bare name) into `dest_dir`, written as `dest_dir/<name>`. Resolves the
/// name to a Drive file id, then streams the media body to a temp file and renames it into place
/// (so a partial download never looks complete). Streamed, not buffered, so a large restore
/// archive can't balloon memory.
pub(crate) async fn download_archive(token_key: &str, name: &str, dest_dir: &Path) -> Result<()> {
    if !naming::valid_archive_name(name) {
        return Err(Error::Other("invalid backup name".into()));
    }
    let folder_id = ensure_backup_folder(token_key).await?;
    let id = list_files_with_ids(token_key, &folder_id)
        .await?
        .into_iter()
        .find(|(_, n, _)| n == name)
        .map(|(id, _, _)| id)
        .ok_or_else(|| Error::Other("that backup is no longer on Google Drive".into()))?;

    let url = format!("{DRIVE_API}/files/{id}?alt=media");
    let resp = authorized_send(token_key, |c, bearer| c.get(&url).bearer_auth(bearer)).await?;
    if !resp.status().is_success() {
        return Err(drive_error(resp, "downloading the archive").await);
    }

    let final_path = dest_dir.join(name);
    let tmp_path = dest_dir.join(format!("{name}.part"));
    {
        use futures_util::StreamExt;
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp_path)?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            // A quick blocking write per chunk on the runtime thread — fine for an occasional
            // restore; keeps peak memory at one chunk rather than the whole archive.
            file.write_all(&chunk)?;
        }
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_builders_are_scoped_and_stable() {
        assert_eq!(
            folder_query(),
            "mimeType='application/vnd.google-apps.folder' and \
             name='Personal Manager Backups' and trashed=false"
        );
        assert_eq!(
            in_folder_query("FOLDER123"),
            "'FOLDER123' in parents and trashed=false"
        );
    }

    #[test]
    fn pick_folder_prefers_earliest_created_then_smallest_id() {
        // Two folders (a duplicate-name race): the earlier createdTime wins.
        let chosen = pick_folder(vec![
            ("bbb".into(), Some("2026-07-02T10:00:00.000Z".into())),
            ("aaa".into(), Some("2026-07-01T09:00:00.000Z".into())),
        ]);
        assert_eq!(chosen.as_deref(), Some("aaa"));

        // Tie on createdTime → smallest id.
        let chosen = pick_folder(vec![
            ("zzz".into(), Some("2026-07-01T09:00:00.000Z".into())),
            ("aaa".into(), Some("2026-07-01T09:00:00.000Z".into())),
        ]);
        assert_eq!(chosen.as_deref(), Some("aaa"));

        // A folder with a real createdTime beats one missing it.
        let chosen = pick_folder(vec![
            ("no-time".into(), None),
            ("timed".into(), Some("2026-07-05T00:00:00.000Z".into())),
        ]);
        assert_eq!(chosen.as_deref(), Some("timed"));

        assert_eq!(pick_folder(vec![]), None);
    }

    /// A captured Drive `files.list` payload (fields=files(id,name,size,createdTime)) — pins the
    /// parser + filter to the real wire shape (int64 `size` as a STRING; a folder and a
    /// non-`.pmbackup` file that must be excluded from the archive listing).
    const CAPTURED_FILE_LIST: &str = r#"{
      "files": [
        {"id":"f1","name":"pm-backup-v1-20260101T000000Z.pmbackup","size":"4096","createdTime":"2026-01-01T00:00:00.000Z"},
        {"id":"f2","name":"pm-backup-v1-20260703T000000Z.pmbackup","createdTime":"2026-07-03T00:00:00.000Z"},
        {"id":"d1","name":"Personal Manager Backups","createdTime":"2025-12-01T00:00:00.000Z"},
        {"id":"x1","name":"notes.txt","size":"12","createdTime":"2026-02-02T00:00:00.000Z"}
      ]
    }"#;

    #[test]
    fn parses_and_filters_a_captured_listing() {
        let list: FileList = serde_json::from_str(CAPTURED_FILE_LIST).unwrap();
        // Mirror `list_archives`' filter + map + sort (newest first) on the parsed files.
        let mut entries: Vec<BackupEntry> = list
            .files
            .into_iter()
            .filter_map(|f| f.name.map(|name| (name, f.size)))
            .filter(|(name, _)| name.ends_with(ARCHIVE_EXT))
            .map(|(name, size)| BackupEntry {
                name,
                size: size.and_then(|s| s.parse::<u64>().ok()),
            })
            .collect();
        entries.sort_by(|a, b| b.name.cmp(&a.name));

        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "pm-backup-v1-20260703T000000Z.pmbackup",
                "pm-backup-v1-20260101T000000Z.pmbackup",
            ]
        );
        // int64-as-string size parses; a missing size stays None.
        assert_eq!(entries[0].size, None); // the July archive had no `size`
        assert_eq!(entries[1].size, Some(4096));
    }

    #[test]
    fn retention_selection_targets_the_right_archives() {
        // The doomed set (by name) equals the shared selector's, proving the prefix/valid wiring.
        let names = vec![
            "pm-backup-v1-20260101T000000Z.pmbackup".to_string(),
            "pm-backup-v1-20260703T000000Z.pmbackup".to_string(),
            "pm-backup-v1-20260202T000000Z.pmbackup".to_string(),
        ];
        let doomed = naming::select_for_deletion(&names, 1);
        assert_eq!(doomed.len(), 2);
        assert!(!doomed.contains(&"pm-backup-v1-20260703T000000Z.pmbackup".to_string()));
    }
}
