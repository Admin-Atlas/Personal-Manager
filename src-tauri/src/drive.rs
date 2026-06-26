// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google Drive connector (Stage 3, board card 4A / spec §8.1) — PM's first cloud-API source.
//! Builds on the index-only foundation ([`crate::index_only`]): this module supplies the per-account
//! change DETECTION (the Drive changes feed + a delta page-token) and the body fetch; the foundation
//! owns the source-agnostic SEMANTICS (the [`react`](crate::index_only::react) reducer,
//! `register_pointer`, `apply_actions`, the encrypted manifest, the soft reachability states).
//!
//! PM observes Drive **read-only** — it never writes (scope `drive.readonly`). Files are indexed-only:
//! a metadata row + an embedding + a pointer (the Drive fileId, webViewLink, modifiedTime, content
//! hash), never the bytes — the body is fetched live on demand.
//!
//! **Multi-account.** Each connected Google account is its own [`connector_sources`] row (id
//! `gdrive:<email>`), its own keychain token (`account_token_key`), and namespaces its item ids
//! `gdrive:<email>:<fileId>` — so the foundation's `source_id LIKE 'gdrive:<email>:%'` fan-out flips a
//! single account to `unreachable` on an auth failure without touching the others.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::google::{self, Token};
use crate::index_only::{self, ChangeEvent, ItemState, SourceState};
use crate::{ingest, secrets, AppState};

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
/// Cap on a single fetched/exported file body (25 MiB) so one huge file can't balloon memory; an
/// over-cap file is skipped with a surfaced note rather than indexed.
const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
/// Runaway guard on pagination (≈200k files) — a backstop against a never-clearing page token, not a
/// coverage cap; if it ever trips we log it (we do not silently drop the rest).
const MAX_PAGES: usize = 1000;
/// The field projection for a Drive file — kept tight (only what the connector needs).
const FILE_FIELDS: &str = "id,name,mimeType,modifiedTime,md5Checksum,trashed,webViewLink";

const PROVIDER: &str = "google";
const SERVICE: &str = "drive";

// --- identity / namespacing ---------------------------------------------------------------------

/// The keychain token key for one Drive account (`<prefix><email>`).
pub fn account_token_key(email: &str) -> String {
    format!("{}{}", secrets::GOOGLE_TOKEN_DRIVE_PREFIX, email)
}

/// The `connector_sources.id` for one account, and the item-id namespace prefix.
fn account_id(email: &str) -> String {
    format!("gdrive:{email}")
}

/// The stable index-only `source_id` for one Drive file under one account: `gdrive:<email>:<fileId>`.
pub fn source_id_for(email: &str, file_id: &str) -> String {
    format!("gdrive:{email}:{file_id}")
}

/// Recover the account email from a `gdrive:<email>:<fileId>` source id (emails carry no `:`, Drive
/// fileIds carry no `:`, so the last `:` splits cleanly).
pub fn account_of(source_id: &str) -> Option<String> {
    let rest = source_id.strip_prefix("gdrive:")?;
    rest.rsplit_once(':').map(|(email, _)| email.to_string())
}

// --- account registry (connector_sources rows for provider=google service=drive) ----------------

/// A connected Drive account, for the Settings list + status.
#[derive(Clone, Serialize)]
pub struct DriveAccount {
    pub id: String,
    pub email: String,
    pub label: String,
    pub last_synced_at: Option<String>,
    /// `'ok' | 'unreachable' | 'error'`.
    pub state: String,
    /// How many index-only documents this account currently has.
    pub indexed: i64,
}

/// Insert (or refresh) the registry row for a connected account; resets its state to `ok`.
pub fn upsert_account(conn: &Connection, email: &str, label: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO connector_sources(id, provider, service, label, account_email) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(id) DO UPDATE SET label = excluded.label, \
             account_email = excluded.account_email, state = 'ok'",
        params![account_id(email), PROVIDER, SERVICE, label, email],
    )?;
    Ok(())
}

/// One row read from the account registry: `(id, account_email, label, last_synced_at, state)`.
type AccountRow = (String, Option<String>, String, Option<String>, String);

/// Every connected Drive account, with a live count of its indexed documents.
pub fn list_accounts(conn: &Connection) -> Result<Vec<DriveAccount>> {
    let mut stmt = conn.prepare(
        "SELECT id, account_email, label, last_synced_at, state FROM connector_sources \
         WHERE provider = ?1 AND service = ?2 ORDER BY created_at",
    )?;
    let rows: Vec<AccountRow> = stmt
        .query_map(params![PROVIDER, SERVICE], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, email, label, last_synced_at, state) in rows {
        let email = email.unwrap_or_default();
        let indexed: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents \
                 WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%'",
                params![account_id(&email)],
                |r| r.get(0),
            )
            .unwrap_or(0);
        out.push(DriveAccount {
            id,
            email,
            label,
            last_synced_at,
            state,
            indexed,
        });
    }
    Ok(out)
}

/// The stored delta cursor (Drive changes page token) for an account, if any.
pub fn get_cursor(conn: &Connection, email: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT cursor FROM connector_sources WHERE id = ?1",
        params![account_id(email)],
        |r| r.get(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(Error::from)
}

/// Record a clean sync: advance the cursor, stamp the time, and clear any failure state.
pub fn set_synced(conn: &Connection, email: &str, cursor: &str) -> Result<()> {
    conn.execute(
        "UPDATE connector_sources \
         SET cursor = ?2, last_synced_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), state = 'ok' \
         WHERE id = ?1",
        params![account_id(email), cursor],
    )?;
    Ok(())
}

/// Set an account's connection state (`'ok' | 'unreachable' | 'error'`).
pub fn set_state(conn: &Connection, email: &str, state: &str) -> Result<()> {
    conn.execute(
        "UPDATE connector_sources SET state = ?2 WHERE id = ?1",
        params![account_id(email), state],
    )?;
    Ok(())
}

/// Disconnect one account: soft-flag its items `unreachable` (kept findable), drop the registry row,
/// and forget its token. Never hard-deletes the indexed documents.
pub fn forget_account(conn: &Connection, email: &str) -> Result<()> {
    conn.execute(
        "UPDATE documents SET source_state = 'unreachable' \
         WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%'",
        params![account_id(email)],
    )?;
    conn.execute(
        "DELETE FROM connector_sources WHERE id = ?1",
        params![account_id(email)],
    )?;
    secrets::clear_google_token_for(&account_token_key(email)).ok();
    Ok(())
}

/// Forget every Drive account — used when the shared Google client credentials are cleared (they all
/// depend on that client, so none can refresh any more).
pub fn forget_all_accounts(conn: &Connection) -> Result<()> {
    let emails: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT account_email FROM connector_sources WHERE provider = ?1 AND service = ?2",
        )?;
        let rows: Vec<Option<String>> = stmt
            .query_map(params![PROVIDER, SERVICE], |r| {
                r.get::<_, Option<String>>(0)
            })?
            .collect::<std::result::Result<_, _>>()?;
        rows.into_iter().flatten().collect()
    };
    for email in emails {
        forget_account(conn, &email)?;
    }
    Ok(())
}

/// The persisted state of a Drive item, in the shape the foundation's reducer needs (or `None` if the
/// source id has never been seen).
pub fn read_item_state(conn: &Connection, source_id: &str) -> Result<Option<ItemState>> {
    conn.query_row(
        "SELECT source_modified_at, source_content_hash, source_state \
         FROM documents WHERE source_id = ?1 AND source_type = 'index_only'",
        params![source_id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )
    .optional()
    .map_err(Error::from)
    .map(|opt| {
        opt.map(|(modified, hash, state)| ItemState {
            source_id: source_id.to_string(),
            source_modified_at: modified,
            source_content_hash: hash,
            source_state: SourceState::from_db(&state),
        })
    })
}

// --- Drive file model + pure parsing/mapping (the unit-tested core) ------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub modified_time: Option<String>,
    pub md5: Option<String>,
    pub trashed: bool,
    pub web_view_link: Option<String>,
}

impl DriveFile {
    /// The source content hash for change detection: Drive's `md5Checksum` when present (binary /
    /// uploaded files), else `modifiedTime` (Google-native docs have no md5; modifiedTime bumps on
    /// every edit, so it is an honest change signal).
    pub fn content_hash(&self) -> Option<String> {
        self.md5.clone().or_else(|| self.modified_time.clone())
    }

    /// A `PointerInput` for the foundation, given the freshly-fetched body.
    pub fn pointer(&self, source_id: String, body: String) -> index_only::PointerInput {
        index_only::PointerInput {
            source_id,
            title: self.name.clone(),
            external_ref: self.web_view_link.clone(),
            source_modified_at: self.modified_time.clone(),
            source_content_hash: self.content_hash(),
            body,
        }
    }
}

/// One entry from the changes feed: a file changed, or an id was removed/trashed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveChange {
    pub file_id: String,
    pub removed: bool,
    pub file: Option<DriveFile>,
}

fn parse_file(v: &Value) -> Option<DriveFile> {
    let id = v.get("id")?.as_str()?.to_string();
    Some(DriveFile {
        id,
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_string(),
        mime_type: v
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        modified_time: v
            .get("modifiedTime")
            .and_then(Value::as_str)
            .map(String::from),
        md5: v
            .get("md5Checksum")
            .and_then(Value::as_str)
            .map(String::from),
        trashed: v.get("trashed").and_then(Value::as_bool).unwrap_or(false),
        web_view_link: v
            .get("webViewLink")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

/// Parse a `files.list` page → its files (trashed ones excluded by the query) + the next page token.
pub fn parse_files(value: &Value) -> (Vec<DriveFile>, Option<String>) {
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_file).collect())
        .unwrap_or_default();
    let next = value
        .get("nextPageToken")
        .and_then(Value::as_str)
        .map(String::from);
    (files, next)
}

/// Parse a `changes.list` page → its changes + `(nextPageToken, newStartPageToken)`.
pub fn parse_changes(value: &Value) -> (Vec<DriveChange>, Option<String>, Option<String>) {
    let changes = value
        .get("changes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let file = c.get("file").and_then(parse_file);
                    let file_id = c
                        .get("fileId")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .or_else(|| file.as_ref().map(|f| f.id.clone()))?;
                    let removed = c.get("removed").and_then(Value::as_bool).unwrap_or(false)
                        || file.as_ref().map(|f| f.trashed).unwrap_or(false);
                    Some(DriveChange {
                        file_id,
                        removed,
                        file,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("nextPageToken")
        .and_then(Value::as_str)
        .map(String::from);
    let new_start = value
        .get("newStartPageToken")
        .and_then(Value::as_str)
        .map(String::from);
    (changes, next, new_start)
}

/// Map a Drive change onto a foundation [`ChangeEvent`] — the **pure heart of detection**. `None`
/// means "skip" (a non-removal change with no file payload — nothing actionable). A rename in Drive
/// keeps the same fileId (the stable source id), so classification is preserved either way; a
/// content edit bumps the hash and re-embeds, carrying the new title along.
pub fn map_change(change: &DriveChange, email: &str, known: bool) -> Option<ChangeEvent> {
    let source_id = source_id_for(email, &change.file_id);
    if change.removed {
        return Some(ChangeEvent::Delete { source_id });
    }
    let file = change.file.as_ref()?;
    if !known {
        return Some(ChangeEvent::Add {
            source_id,
            modified_at: file.modified_time.clone(),
        });
    }
    Some(ChangeEvent::Update {
        source_id,
        modified_at: file.modified_time.clone(),
        new_content_hash: file.content_hash(),
    })
}

/// How to turn a Drive file's bytes into indexable text, decided purely by its MIME type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchPlan {
    /// A Google-native doc — export to this (always text-ish) MIME, decode as UTF-8.
    Export { mime: &'static str },
    /// A file whose bytes are already text — download and decode as UTF-8.
    DownloadText,
    /// A binary document (pdf/docx/…) — download to a temp file and convert via the sidecar.
    DownloadBinary,
    /// Nothing useful to index (folders, forms, drawings, shortcuts) — skip.
    Skip,
}

/// Decide how to fetch a file's text from its MIME type (pure, unit-tested).
pub fn fetch_plan(mime: &str) -> FetchPlan {
    match mime {
        "application/vnd.google-apps.document" => FetchPlan::Export { mime: "text/plain" },
        "application/vnd.google-apps.spreadsheet" => FetchPlan::Export { mime: "text/csv" },
        "application/vnd.google-apps.presentation" => FetchPlan::Export { mime: "text/plain" },
        // Every other google-apps type (folder, form, drawing, site, map, script, shortcut, …) has
        // no useful plain-text export.
        m if m.starts_with("application/vnd.google-apps.") => FetchPlan::Skip,
        "application/json" | "application/xml" => FetchPlan::DownloadText,
        m if m.starts_with("text/") => FetchPlan::DownloadText,
        _ => FetchPlan::DownloadBinary,
    }
}

// --- network (async, DB-free; callers hold no lock across these — rule #4) ----------------------

/// The account a fresh token grants (email + display name), via Drive's `about`. Uses the in-hand
/// token (not yet persisted), so it runs right after consent to learn which account to save under.
pub async fn about_user(token: &Token) -> Result<(String, String)> {
    let v = google::get_json_with_token(
        token,
        &format!("{DRIVE_API}/about?fields=user(emailAddress,displayName)"),
    )
    .await?;
    let user = v.get("user");
    let email = user
        .and_then(|u| u.get("emailAddress"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Other("Google didn't return the account email.".into()))?
        .to_string();
    let name = user
        .and_then(|u| u.get("displayName"))
        .and_then(Value::as_str)
        .unwrap_or(&email)
        .to_string();
    Ok((email, name))
}

fn files_url(page: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/files"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("q", "trashed = false");
        q.append_pair("pageSize", "200");
        q.append_pair("spaces", "drive");
        q.append_pair("orderBy", "modifiedTime desc");
        q.append_pair("fields", &format!("nextPageToken,files({FILE_FIELDS})"));
        if let Some(t) = page {
            q.append_pair("pageToken", t);
        }
    }
    Ok(url.to_string())
}

/// Enumerate every non-trashed file in My Drive (paginated) — the first-sync baseline.
pub async fn enumerate_drive(token_key: &str) -> Result<Vec<DriveFile>> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let v = google::authorized_get(token_key, &files_url(page.as_deref())?).await?;
        let (files, next) = parse_files(&v);
        out.extend(files);
        match next {
            Some(t) => page = Some(t),
            None => return Ok(out),
        }
    }
    eprintln!(
        "drive: enumerate hit the page guard at {MAX_PAGES} pages; some files were not listed"
    );
    Ok(out)
}

/// The delta baseline cursor (`changes.getStartPageToken`).
pub async fn start_page_token(token_key: &str) -> Result<String> {
    let v =
        google::authorized_get(token_key, &format!("{DRIVE_API}/changes/startPageToken")).await?;
    v.get("startPageToken")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| Error::Other("Drive didn't return a change cursor.".into()))
}

fn changes_url(page_token: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/changes"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("pageToken", page_token);
        q.append_pair("pageSize", "200");
        q.append_pair("includeRemoved", "true");
        q.append_pair("spaces", "drive");
        q.append_pair(
            "fields",
            &format!("nextPageToken,newStartPageToken,changes(fileId,removed,file({FILE_FIELDS}))"),
        );
    }
    Ok(url.to_string())
}

/// Pull every change since `cursor`, following pages, and return the changes + the next baseline
/// cursor (`newStartPageToken`). On an expired cursor Google returns HTTP 410 — surfaced as an error
/// the caller detects ([`is_cursor_expired`]) to fall back to a full re-list.
pub async fn list_changes(token_key: &str, cursor: &str) -> Result<(Vec<DriveChange>, String)> {
    let mut all = Vec::new();
    let mut page = cursor.to_string();
    for _ in 0..MAX_PAGES {
        let v = google::authorized_get(token_key, &changes_url(&page)?).await?;
        let (changes, next, new_start) = parse_changes(&v);
        all.extend(changes);
        match (next, new_start) {
            (Some(t), _) => page = t,
            (None, Some(start)) => return Ok((all, start)),
            (None, None) => return Ok((all, page)),
        }
    }
    eprintln!("drive: changes hit the page guard at {MAX_PAGES} pages");
    Ok((all, page))
}

/// Fetch one file's metadata (for body-on-demand, where we hold only the stored pointer).
pub async fn fetch_file(token_key: &str, file_id: &str) -> Result<DriveFile> {
    let url = format!("{DRIVE_API}/files/{file_id}?fields={FILE_FIELDS}");
    let v = google::authorized_get(token_key, &url).await?;
    parse_file(&v).ok_or_else(|| Error::Other("Drive returned no file for that id.".into()))
}

/// True if a Drive API error is the "page token expired" 410 — the signal to discard the cursor and
/// re-baseline with a full `files.list`.
pub fn is_cursor_expired(err: &Error) -> bool {
    err.to_string().contains("(410")
}

/// True if a Drive API error is an auth failure (revoked/expired) for the whole account — the signal
/// to fan the account out to `unreachable` rather than treat it as mass deletion.
pub fn is_auth_failure(err: &Error) -> bool {
    let s = err.to_string();
    s.contains("(401") || s.contains("(403")
}

/// Fetch a file's body as indexable text, or `None` if it has no useful text (skipped type, empty
/// export, or over the size cap). Google-native docs are exported to text; text files downloaded
/// directly; binaries downloaded to a temp file and converted via the sidecar. Never holds the DB
/// lock; the sidecar must already be installed.
pub async fn fetch_body(
    state: &AppState,
    token_key: &str,
    file: &DriveFile,
) -> Result<Option<String>> {
    match fetch_plan(&file.mime_type) {
        FetchPlan::Skip => Ok(None),
        FetchPlan::Export { mime } => {
            let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/files/{}/export", file.id))
                .map_err(|e| Error::Other(e.to_string()))?;
            url.query_pairs_mut().append_pair("mimeType", mime);
            let bytes =
                google::authorized_get_bytes(token_key, url.as_str(), MAX_FILE_BYTES).await?;
            Ok(non_empty(&String::from_utf8_lossy(&bytes)))
        }
        FetchPlan::DownloadText => {
            let url = format!("{DRIVE_API}/files/{}?alt=media", file.id);
            let bytes = google::authorized_get_bytes(token_key, &url, MAX_FILE_BYTES).await?;
            Ok(non_empty(&String::from_utf8_lossy(&bytes)))
        }
        FetchPlan::DownloadBinary => {
            let url = format!("{DRIVE_API}/files/{}?alt=media", file.id);
            let bytes = match google::authorized_get_bytes(token_key, &url, MAX_FILE_BYTES).await {
                Ok(b) => b,
                // An over-cap download is a skip (kept findable via its title), not a hard error.
                Err(e) if e.to_string().contains("too large") => return Ok(None),
                Err(e) => return Err(e),
            };
            let tmp = stage_temp(&file.name, &bytes)?;
            let converted = state.sidecar.convert(&tmp);
            let _ = std::fs::remove_file(&tmp);
            let (markdown, _title) = converted?;
            Ok(non_empty(&markdown))
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Stage downloaded bytes to a temp file named with the source's extension, so the sidecar
/// (MarkItDown) picks the right converter. Content-addressed so a re-fetch reuses the name; removed
/// by the caller after conversion.
fn stage_temp(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 8)
        .unwrap_or("bin");
    let digest = ingest::hex_digest(bytes);
    let path = std::env::temp_dir().join(format!("pm-drive-{}.{ext}", &digest[..16]));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

// --- progress event (rendered by the shared IngestProgress component) ---------------------------

/// Sync progress for the Settings UI. Distinct from `IngestEvent` because a sync also re-embeds and
/// removes (which carry no freshly-ingested `Document`); the frontend maps `processed`/`total` onto
/// the shared `IngestProgress` bar.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DriveSyncEvent {
    /// The total number of files/changes this run will work through (sent once, before the items).
    Counted { total: usize },
    /// One item processed (1-based `processed` of `total`).
    Item {
        processed: usize,
        total: usize,
        name: String,
    },
    /// The run is done, with a breakdown.
    Finished {
        indexed: usize,
        updated: usize,
        removed: usize,
        skipped: usize,
        failed: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, mime: &str, md5: Option<&str>, modified: &str) -> DriveFile {
        DriveFile {
            id: id.into(),
            name: format!("{id}.bin"),
            mime_type: mime.into(),
            modified_time: Some(modified.into()),
            md5: md5.map(String::from),
            trashed: false,
            web_view_link: Some(format!("https://drive/{id}")),
        }
    }

    #[test]
    fn source_id_namespacing_round_trips_and_matches_the_fanout_shape() {
        let sid = source_id_for("a@b.com", "FILE123");
        assert_eq!(sid, "gdrive:a@b.com:FILE123");
        // The fan-out matches `gdrive:<email>:%`, so the account prefix must be exactly this.
        assert!(sid.starts_with("gdrive:a@b.com:"));
        assert_eq!(account_of(&sid).as_deref(), Some("a@b.com"));
        assert_eq!(account_of("not-a-drive-id"), None);
    }

    #[test]
    fn content_hash_prefers_md5_then_modified_time() {
        let binary = file(
            "f1",
            "application/pdf",
            Some("deadbeef"),
            "2026-06-26T00:00:00Z",
        );
        assert_eq!(binary.content_hash().as_deref(), Some("deadbeef"));
        let native = file(
            "f2",
            "application/vnd.google-apps.document",
            None,
            "2026-06-26T00:00:00Z",
        );
        assert_eq!(
            native.content_hash().as_deref(),
            Some("2026-06-26T00:00:00Z")
        );
    }

    #[test]
    fn map_change_covers_remove_add_and_update() {
        let f = file("f1", "application/pdf", Some("h1"), "2026-06-26T00:00:00Z");
        // Removed → Delete (regardless of whether we knew it).
        let removed = DriveChange {
            file_id: "f1".into(),
            removed: true,
            file: None,
        };
        assert_eq!(
            map_change(&removed, "a@b.com", true),
            Some(ChangeEvent::Delete {
                source_id: "gdrive:a@b.com:f1".into()
            })
        );
        // Unknown file → Add.
        let changed = DriveChange {
            file_id: "f1".into(),
            removed: false,
            file: Some(f.clone()),
        };
        assert_eq!(
            map_change(&changed, "a@b.com", false),
            Some(ChangeEvent::Add {
                source_id: "gdrive:a@b.com:f1".into(),
                modified_at: Some("2026-06-26T00:00:00Z".into())
            })
        );
        // Known file → Update carrying the content hash (md5 here).
        assert_eq!(
            map_change(&changed, "a@b.com", true),
            Some(ChangeEvent::Update {
                source_id: "gdrive:a@b.com:f1".into(),
                modified_at: Some("2026-06-26T00:00:00Z".into()),
                new_content_hash: Some("h1".into())
            })
        );
        // A native doc with no md5 → Update keyed on modifiedTime.
        let native = DriveChange {
            file_id: "d1".into(),
            removed: false,
            file: Some(file(
                "d1",
                "application/vnd.google-apps.document",
                None,
                "2026-06-26T01:00:00Z",
            )),
        };
        assert_eq!(
            map_change(&native, "a@b.com", true),
            Some(ChangeEvent::Update {
                source_id: "gdrive:a@b.com:d1".into(),
                modified_at: Some("2026-06-26T01:00:00Z".into()),
                new_content_hash: Some("2026-06-26T01:00:00Z".into())
            })
        );
        // A non-removal change with no file payload → skip.
        let empty = DriveChange {
            file_id: "x".into(),
            removed: false,
            file: None,
        };
        assert_eq!(map_change(&empty, "a@b.com", false), None);
    }

    #[test]
    fn fetch_plan_routes_each_mime() {
        assert_eq!(
            fetch_plan("application/vnd.google-apps.document"),
            FetchPlan::Export { mime: "text/plain" }
        );
        assert_eq!(
            fetch_plan("application/vnd.google-apps.spreadsheet"),
            FetchPlan::Export { mime: "text/csv" }
        );
        assert_eq!(
            fetch_plan("application/vnd.google-apps.folder"),
            FetchPlan::Skip
        );
        assert_eq!(
            fetch_plan("application/vnd.google-apps.form"),
            FetchPlan::Skip
        );
        assert_eq!(fetch_plan("text/plain"), FetchPlan::DownloadText);
        assert_eq!(fetch_plan("text/markdown"), FetchPlan::DownloadText);
        assert_eq!(fetch_plan("application/json"), FetchPlan::DownloadText);
        assert_eq!(fetch_plan("application/pdf"), FetchPlan::DownloadBinary);
        assert_eq!(
            fetch_plan("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            FetchPlan::DownloadBinary
        );
    }

    #[test]
    fn parse_files_and_changes_read_the_shapes() {
        let files = serde_json::json!({
            "nextPageToken": "PAGE2",
            "files": [
                {"id": "f1", "name": "A.pdf", "mimeType": "application/pdf", "md5Checksum": "h1", "modifiedTime": "2026-06-26T00:00:00Z", "webViewLink": "https://drive/f1"},
                {"id": "f2", "name": "Notes", "mimeType": "application/vnd.google-apps.document", "modifiedTime": "2026-06-26T01:00:00Z"}
            ]
        });
        let (parsed, next) = parse_files(&files);
        assert_eq!(next.as_deref(), Some("PAGE2"));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].md5.as_deref(), Some("h1"));
        assert!(parsed[1].md5.is_none());

        let changes = serde_json::json!({
            "newStartPageToken": "TOK9",
            "changes": [
                {"fileId": "f1", "removed": false, "file": {"id": "f1", "name": "A.pdf", "mimeType": "application/pdf", "md5Checksum": "h2", "modifiedTime": "2026-06-26T02:00:00Z"}},
                {"fileId": "f3", "removed": true}
            ]
        });
        let (parsed, next, new_start) = parse_changes(&changes);
        assert!(next.is_none());
        assert_eq!(new_start.as_deref(), Some("TOK9"));
        assert_eq!(parsed.len(), 2);
        assert!(!parsed[0].removed);
        assert_eq!(parsed[0].file.as_ref().unwrap().md5.as_deref(), Some("h2"));
        assert!(parsed[1].removed);
        assert!(parsed[1].file.is_none());
    }

    #[test]
    fn a_trashed_file_in_changes_reads_as_removed() {
        let changes = serde_json::json!({
            "changes": [
                {"fileId": "f1", "removed": false, "file": {"id": "f1", "name": "x", "mimeType": "text/plain", "trashed": true}}
            ]
        });
        let (parsed, _, _) = parse_changes(&changes);
        assert!(parsed[0].removed, "a trashed file is a removal");
    }
}
