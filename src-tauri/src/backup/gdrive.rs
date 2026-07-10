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

use std::path::Path;

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

/// Bytes sent per resumable-upload chunk. Peak upload memory is one of these, not the whole archive
/// — the point of F-07 on the 8 GB target. Google requires every chunk except the last to be a
/// multiple of 256 KiB; 8 MiB = 32 × 256 KiB. The exact size is a throughput/overhead trade-off
/// pending live measurement — not yet tuned against a real transfer.
const UPLOAD_CHUNK_SIZE: u64 = 8 * 1024 * 1024;
/// Bounded per-chunk retries. On a transient failure we refresh the token, re-query the committed
/// offset, and resume from there — so a blip costs one chunk, not the whole transfer.
const UPLOAD_MAX_RETRIES: usize = 5;

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
    let (out, truncated) = crate::connector_sync::paginate(MAX_PAGES, |page| {
        let q = q.as_str();
        async move {
            let url = files_list_url(q, fields, page.as_deref())?;
            let resp = google::authorized_send(&http_client()?, token_key, |c, bearer| {
                c.get(&url).bearer_auth(bearer)
            })
            .await?;
            if !resp.status().is_success() {
                return Err(drive_error(resp, "listing backups").await);
            }
            let list: FileList = resp.json().await?;
            let items = list
                .files
                .into_iter()
                .filter_map(|f| {
                    f.name
                        .map(|name| (f.id, name, f.size.and_then(|s| s.parse::<u64>().ok())))
                })
                .collect();
            Ok((items, list.next_page_token))
        }
    })
    .await?;
    if truncated {
        // Backstop tripped — surface it rather than silently returning a partial listing
        // (retention would then under-count and never trim).
        return Err(Error::Other(
            "Google Drive returned too many pages listing backups".into(),
        ));
    }
    Ok(out)
}

/// Ensure PM's backup folder exists and return its id (idempotent — check then create). With
/// `drive.file`, `files.list` only ever surfaces app-created folders, so this can't accidentally
/// pick a folder the user made by hand. If Drive somehow holds more than one (it permits duplicate
/// names; two devices could race), [`pick_folder`] converges on the earliest-created one.
pub(crate) async fn ensure_backup_folder(token_key: &str) -> Result<String> {
    let url = files_list_url(&folder_query(), "files(id,createdTime)", None)?;
    let resp = google::authorized_send(&http_client()?, token_key, |c, bearer| {
        c.get(&url).bearer_auth(bearer)
    })
    .await?;
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
    let resp = google::authorized_send(&http_client()?, token_key, |c, bearer| {
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
/// then stream the bytes to the returned session URI in `Content-Range` chunks. Peak memory is one
/// chunk ([`UPLOAD_CHUNK_SIZE`]), not the whole archive, and a mid-transfer blip resumes from the
/// last server-acknowledged byte instead of restarting the whole PUT (F-07).
pub(crate) async fn upload_archive(
    token_key: &str,
    local: &Path,
    archive_name: &str,
    folder_id: &str,
) -> Result<()> {
    // A cheap stat (not a whole-file read) — the total feeds the session's declared length and the
    // per-chunk Content-Range math.
    let len = std::fs::metadata(local)?.len();

    // 1) Initiate the resumable session. Small JSON body → safe to 401-retry via `authorized_send`.
    let meta = serde_json::json!({ "name": archive_name, "parents": [folder_id] }).to_string();
    let init_url = format!("{UPLOAD_API}?uploadType=resumable&fields=id");
    let init = google::authorized_send(&http_client()?, token_key, |c, bearer| {
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

    // 2) Stream the archive to the session URI in bounded chunks (see below).
    transfer_archive(token_key, &session, local, len).await
}

/// The resumable chunk loop: seek to the committed offset, read one [`UPLOAD_CHUNK_SIZE`] slice, PUT
/// it with an inclusive `Content-Range`, and advance to the offset the server acknowledges (`308
/// Resume Incomplete` carries a `Range: bytes=0-<lastByte>` header). A transient failure (network
/// error, 5xx, or a token that expired mid-transfer) is retried up to [`UPLOAD_MAX_RETRIES`] times:
/// refresh the token, re-query the committed offset, and resume — so a blip costs one chunk, not the
/// whole archive. Peak memory is one chunk buffer.
async fn transfer_archive(token_key: &str, session: &str, local: &Path, total: u64) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let client = http_client()?;
    let mut file = std::fs::File::open(local)?;
    let mut bearer = google::valid_access_token(token_key).await?;
    let mut offset: u64 = 0;
    let mut retries = 0usize;

    while offset < total {
        let want = UPLOAD_CHUNK_SIZE.min(total - offset);
        let mut buf = vec![0u8; want as usize];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf)?;

        let sent = client
            .put(session)
            .bearer_auth(bearer.expose())
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(
                reqwest::header::CONTENT_RANGE,
                content_range(offset, want, total),
            )
            .body(buf)
            .send()
            .await;

        match sent {
            // The final chunk commits with 200/201 (the file resource).
            Ok(resp) if resp.status().is_success() => return Ok(()),
            // 308 Resume Incomplete: advance to the server-acknowledged byte.
            Ok(resp) if resp.status() == reqwest::StatusCode::PERMANENT_REDIRECT => {
                offset = next_offset_from_range(&resp).unwrap_or(offset + want);
                retries = 0;
            }
            // A retryable failure (5xx or a mid-transfer 401): refresh, re-sync the offset, resume.
            Ok(resp)
                if (resp.status().is_server_error()
                    || resp.status() == reqwest::StatusCode::UNAUTHORIZED)
                    && retries < UPLOAD_MAX_RETRIES =>
            {
                retries += 1;
                bearer = google::refresh_now(token_key).await?;
                offset = query_committed_offset(&client, token_key, session, total).await?;
                if offset >= total {
                    return Ok(());
                }
            }
            // A hard client error (or retries exhausted): surface it.
            Ok(resp) => return Err(drive_error(resp, "uploading the archive").await),
            // A transport error: retry the same way while we have budget, else give up.
            Err(_) if retries < UPLOAD_MAX_RETRIES => {
                retries += 1;
                bearer = google::refresh_now(token_key).await?;
                offset = query_committed_offset(&client, token_key, session, total).await?;
                if offset >= total {
                    return Ok(());
                }
            }
            Err(e) => return Err(Error::from(e)),
        }
    }
    Ok(())
}

/// Probe how many bytes the resumable session has committed so far: a PUT with an empty body and
/// `Content-Range: bytes */<total>`. A 2xx means it's already fully stored; a 308 carries the
/// `Range` header we parse for the next byte to send.
async fn query_committed_offset(
    client: &reqwest::Client,
    token_key: &str,
    session: &str,
    total: u64,
) -> Result<u64> {
    let bearer = google::valid_access_token(token_key).await?;
    let resp = client
        .put(session)
        .bearer_auth(bearer.expose())
        .header(reqwest::header::CONTENT_RANGE, format!("bytes */{total}"))
        .body(Vec::<u8>::new())
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(total);
    }
    if resp.status() == reqwest::StatusCode::PERMANENT_REDIRECT {
        return Ok(next_offset_from_range(&resp).unwrap_or(0));
    }
    Err(drive_error(resp, "resuming the upload").await)
}

/// Build an inclusive `Content-Range` value (`bytes {offset}-{end}/{total}`) for a chunk. Pure, so
/// the off-by-one that a naive `offset + len` end would introduce is unit-tested.
fn content_range(offset: u64, len: u64, total: u64) -> String {
    let end = offset + len - 1;
    format!("bytes {offset}-{end}/{total}")
}

/// Parse the last committed byte out of Drive's `Range: bytes=0-<lastByte>` header and return the
/// NEXT byte to send (`lastByte + 1`). Pure, so the resume-offset math is unit-tested without a live
/// `Response`. Absent/garbage header → `None` (caller falls back conservatively).
fn parse_range_last(raw: &str) -> Option<u64> {
    raw.rsplit('-')
        .next()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|n| n + 1)
}

fn next_offset_from_range(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range_last)
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
        let resp = google::authorized_send(&http_client()?, token_key, |c, bearer| {
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
    let resp = google::authorized_send(&http_client()?, token_key, |c, bearer| {
        c.get(&url).bearer_auth(bearer)
    })
    .await?;
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

    // --- F-07: resumable chunked upload arithmetic (the two off-by-one-prone points) ---

    #[test]
    fn content_range_is_inclusive() {
        // The first full 8 MiB chunk of a 20 MiB archive, then the short final chunk. The `end` is
        // INCLUSIVE (last byte), so a naive `offset + len` would be one too high.
        let total = 20 * 1024 * 1024;
        assert_eq!(
            content_range(0, UPLOAD_CHUNK_SIZE, total),
            "bytes 0-8388607/20971520"
        );
        assert_eq!(
            content_range(UPLOAD_CHUNK_SIZE, total - UPLOAD_CHUNK_SIZE, total),
            "bytes 8388608-20971519/20971520"
        );
    }

    #[test]
    fn parse_range_last_yields_the_next_byte() {
        // Drive's 308 carries `Range: bytes=0-<lastByte>`; the next byte to send is lastByte + 1.
        assert_eq!(parse_range_last("bytes=0-1048575"), Some(1048576));
        assert_eq!(parse_range_last("bytes=0-0"), Some(1));
        // Absent/garbage → None, so the caller falls back conservatively rather than mis-seeking.
        assert_eq!(parse_range_last(""), None);
        assert_eq!(parse_range_last("garbage"), None);
    }
}
