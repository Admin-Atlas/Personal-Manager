// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Microsoft OneDrive connector (Stage 3, board card 4B / spec §8.1) — PM's second cloud-API source,
//! a near-mirror of the Google Drive connector ([`crate::drive`]). Builds on the same index-only
//! foundation ([`crate::index_only`]): this module supplies the per-account change DETECTION (the
//! Microsoft Graph **delta query**) and the body fetch; the foundation owns the source-agnostic
//! SEMANTICS (the [`react`](crate::index_only::react) reducer, `register_pointer`, `apply_actions`,
//! the encrypted manifest, the soft reachability states).
//!
//! PM observes OneDrive **read-only** — it never writes (scope `Files.Read`). Files are indexed-only:
//! a metadata row + an embedding + a pointer (the Graph itemId, webUrl, lastModified, content hash),
//! never the bytes — the body is fetched live on demand.
//!
//! **Multi-account.** Each connected Microsoft account is its own [`connector_sources`] row (id
//! `onedrive:<email>`), its own keychain token (`account_token_key`), and namespaces its item ids
//! `onedrive:<email>:<itemId>` — so the foundation's `source_id LIKE 'onedrive:<email>:%'` fan-out
//! flips a single account to `unreachable` on an auth failure without touching the others.
//!
//! **Scope.** The personal OneDrive is indexed whole (the efficient delta cursor) by default, or
//! folder-scoped (re-enumerate + reconcile each sync, no cursor) — exactly mirroring the personal
//! My-Drive scope model in [`crate::drive`]. There is one drive per account (no shared-drive corpus),
//! so item ids stay in the single `onedrive:<email>:<itemId>` namespace.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::index_only::{self, ChangeEvent, ItemState, SourceState};
use crate::microsoft;
use crate::{ingest, secrets, AppState};

/// Cap on a single fetched file body (25 MiB) so one huge file can't balloon memory; an over-cap file
/// is skipped with a surfaced note rather than indexed.
const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
/// Runaway guard on pagination / folder-walk depth — a backstop, not a coverage cap; if it ever trips
/// we log it (we do not silently drop the rest).
const MAX_PAGES: usize = 1000;
/// The field projection for a driveItem — kept tight (only what the connector needs). `file` and
/// `folder` are facets we select whole (so `file.mimeType` / `file.hashes` come back); `deleted`
/// marks a tombstone in the delta feed.
const SELECT_ITEM: &str = "id,name,file,folder,deleted,webUrl,lastModifiedDateTime,size";

const PROVIDER: &str = "microsoft";
const SERVICE: &str = "onedrive";

// --- identity / namespacing ---------------------------------------------------------------------

/// The keychain token key for one OneDrive account (`<prefix><email>`).
pub fn account_token_key(email: &str) -> String {
    format!("{}{}", secrets::MICROSOFT_TOKEN_ONEDRIVE_PREFIX, email)
}

/// The `connector_sources.id` for one account, and the item-id namespace prefix.
fn account_id(email: &str) -> String {
    format!("onedrive:{email}")
}

/// The stable index-only `source_id` for one OneDrive file under one account:
/// `onedrive:<email>:<itemId>`. Graph item ids carry no `:`, so the email splits off cleanly.
pub fn source_id_for(email: &str, item_id: &str) -> String {
    format!("onedrive:{email}:{item_id}")
}

/// Recover the account email from any OneDrive source id. It starts `onedrive:<email>:…`, and an
/// email carries no `:`, so the **first** `:` after the prefix splits the email off cleanly.
pub fn account_of(source_id: &str) -> Option<String> {
    let rest = source_id.strip_prefix("onedrive:")?;
    rest.split_once(':').map(|(email, _)| email.to_string())
}

// --- account registry (connector_sources rows for provider=microsoft service=onedrive) ----------

/// A connected OneDrive account, for the Settings list + status.
#[derive(Clone, Serialize)]
pub struct OneDriveAccount {
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

/// Every connected OneDrive account, with a live count of its indexed documents.
pub fn list_accounts(conn: &Connection) -> Result<Vec<OneDriveAccount>> {
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
        out.push(OneDriveAccount {
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
    secrets::clear_microsoft_token_for(&account_token_key(email)).ok();
    Ok(())
}

/// Forget every OneDrive account — used when the shared Microsoft client id is cleared (every account
/// depends on it, so none can refresh any more).
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

/// The persisted state of a OneDrive item, in the shape the foundation's reducer needs (or `None` if
/// the source id has never been seen).
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

// --- delta cursor + per-account scope ------------------------------------------------------------
//
// OneDrive has ONE drive per account, so (unlike Drive's per-corpus cursor map) the `cursor` column
// holds a single value: the Graph delta link (a full URL) for the whole-drive feed. Folder-scoped
// accounts keep NO cursor (the delta is whole-drive; folder scope re-enumerates + reconciles), so
// switching to folder-scoped clears it and a switch back re-baselines.

/// The stored whole-drive delta link for an account, if any (absent = first sync / folder-scoped).
pub fn get_cursor(conn: &Connection, email: &str) -> Result<Option<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT cursor FROM connector_sources WHERE id = ?1",
            params![account_id(email)],
            |r| r.get(0),
        )
        .optional()
        .map(Option::flatten)?;
    Ok(raw.filter(|s| !s.trim().is_empty()))
}

/// Record a clean sync: advance the whole-drive delta link (when whole-drive synced this pass), stamp
/// the time, and clear any failure state. A folder-scoped pass passes `None` (it keeps no cursor) but
/// still stamps the time + clears failure.
pub fn finalize_sync(conn: &Connection, email: &str, cursor: Option<&str>) -> Result<()> {
    match cursor {
        Some(c) => conn.execute(
            "UPDATE connector_sources \
             SET cursor = ?2, last_synced_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), state = 'ok' \
             WHERE id = ?1",
            params![account_id(email), c],
        )?,
        None => conn.execute(
            "UPDATE connector_sources \
             SET last_synced_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), state = 'ok' \
             WHERE id = ?1",
            params![account_id(email)],
        )?,
    };
    Ok(())
}

/// Persist a finished sync pass for one account, honoring whether any item failed — the decision seam
/// for F-29. A clean pass commits via [`finalize_sync`] (advances the cursor, stamps the time, state
/// `'ok'`). A pass with any failed item is instead flagged `'error'` via [`set_state`], which leaves
/// the cursor **unadvanced** and `last_synced_at` at its last-good value: the failure surfaces in the
/// Connectors warning instead of hiding behind a misleading `'ok'`, and the failed items retry on the
/// next sync (`index_only::react` makes the already-good items cheap no-ops) rather than being skipped
/// past an advanced cursor. Kept out of the engine so the branch is unit-testable end to end. Mirrors
/// the calendar sync's "check failures first" rule.
pub fn finalize_or_flag(
    conn: &Connection,
    email: &str,
    account_failed: bool,
    cursor: Option<&str>,
) -> Result<()> {
    if account_failed {
        set_state(conn, email, "error")
    } else {
        finalize_sync(conn, email, cursor)
    }
}

/// What an account indexes: the personal OneDrive, whole or folder-scoped. Persisted as JSON in
/// `connector_sources.folder_ids` (no migration — the column already exists, nullable). A
/// missing/empty value means the default scope (whole drive) so a freshly-connected account behaves
/// as expected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneDriveScope {
    /// How much of OneDrive to index: `None` = the **entire** drive (the default — delta-cursor
    /// sync); `Some(ids)` = only these folders (recursively, re-enumerated + reconciled each sync, no
    /// cursor). Absent in older stored scopes, so it defaults to whole-drive.
    #[serde(default)]
    pub folders: Option<Vec<String>>,
}

/// Read an account's indexing scope (the default scope when none is stored yet).
pub fn get_scope(conn: &Connection, email: &str) -> Result<OneDriveScope> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT folder_ids FROM connector_sources WHERE id = ?1",
            params![account_id(email)],
            |r| r.get(0),
        )
        .optional()
        .map(Option::flatten)?;
    match raw {
        Some(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).map_err(|e| Error::Other(format!("bad onedrive scope: {e}")))
        }
        _ => Ok(OneDriveScope::default()),
    }
}

/// Persist an account's indexing scope. Folder-scoped uses no delta cursor, so switching TO
/// folder-scoped clears the stored cursor — a later switch back to whole-drive then re-baselines with
/// a fresh delta enumeration (so reactivations and out-of-scope soft-deletes settle cleanly) rather
/// than resuming from a token that predates the scope change.
pub fn set_scope(conn: &Connection, email: &str, scope: &OneDriveScope) -> Result<()> {
    let json = serde_json::to_string(scope)
        .map_err(|e| Error::Other(format!("encode onedrive scope: {e}")))?;
    if scope.folders.is_some() {
        conn.execute(
            "UPDATE connector_sources SET folder_ids = ?2, cursor = NULL WHERE id = ?1",
            params![account_id(email), json],
        )?;
    } else {
        conn.execute(
            "UPDATE connector_sources SET folder_ids = ?2 WHERE id = ?1",
            params![account_id(email), json],
        )?;
    }
    Ok(())
}

/// Every **currently-healthy** (`source_state = 'ok'`) indexed item id for one account — the set the
/// folder-scoped reconcile diffs the live enumeration against. A present id in this set gets an
/// `Update`; a present id NOT in it gets an `Add` (ingests a new file, or reactivates one previously
/// flagged missing/unreachable — e.g. a folder removed and re-added); an id in this set that is no
/// longer present is a deletion. There is one drive per account, so this is simply every account item.
pub fn known_source_ids(conn: &Connection, email: &str) -> Result<Vec<String>> {
    let prefix = format!("{}:", account_id(email));
    let mut stmt = conn.prepare(
        "SELECT source_id FROM documents \
         WHERE source_type = 'index_only' AND source_state = 'ok' \
           AND source_id LIKE ?1 || '%'",
    )?;
    let rows: Vec<String> = stmt
        .query_map(params![prefix], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

// --- driveItem model + pure parsing/mapping (the unit-tested core) -------------------------------

/// A Microsoft Graph driveItem, reduced to what the connector needs. `is_folder` / `is_file` come
/// from the `folder` / `file` facets (an item is one or the other; rare facet-less items like
/// OneNote packages are neither, and skipped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveItem {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub modified_time: Option<String>,
    pub quick_xor_hash: Option<String>,
    pub sha256_hash: Option<String>,
    pub size: Option<i64>,
    pub is_folder: bool,
    pub is_file: bool,
    pub web_url: Option<String>,
}

/// Lets a folder-scoped enumeration reconcile through the shared [`index_only::reconcile_enumeration`]
/// planner. `content_hash` forwards to the inherent method (quickXor-or-sha256-or-modifiedTime).
impl index_only::EnumeratedFile for DriveItem {
    fn local_id(&self) -> &str {
        &self.id
    }
    fn modified_at(&self) -> Option<String> {
        self.modified_time.clone()
    }
    fn content_hash(&self) -> Option<String> {
        // Method-call syntax resolves to the inherent `DriveItem::content_hash` (inherent methods
        // shadow trait methods of the same name), so this forwards rather than recursing.
        self.content_hash()
    }
}

impl DriveItem {
    /// The source content hash for change detection: OneDrive's `quickXorHash` when present (on both
    /// personal and business drives), else `sha256Hash`, else `lastModifiedDateTime` (an honest change
    /// signal — it bumps on every edit).
    pub fn content_hash(&self) -> Option<String> {
        self.quick_xor_hash
            .clone()
            .or_else(|| self.sha256_hash.clone())
            .or_else(|| self.modified_time.clone())
    }

    /// A `PointerInput` for the foundation, given the freshly-fetched body.
    pub fn pointer(&self, source_id: String, body: String) -> index_only::PointerInput {
        index_only::PointerInput {
            source_id,
            title: self.name.clone(),
            external_ref: self.web_url.clone(),
            source_modified_at: self.modified_time.clone(),
            source_content_hash: self.content_hash(),
            body,
            // OneDrive parent-folder parity is a deferred follow-up: Graph exposes the parent via
            // `parentReference` (id + path), a different shape from Drive's `parents[]`, so it stays
            // out of this PR. Files sync exactly as before, untagged.
            source_parent_folder_id: None,
            source_parent_folder_name: None,
        }
    }
}

/// One entry from the delta feed: an item changed, or an id was removed (the `deleted` facet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveDelta {
    pub item_id: String,
    pub removed: bool,
    /// The live **file** this entry concerns (`Some` only for a non-removed item with a `file` facet);
    /// folders, the drive root, and tombstones carry `None`.
    pub file: Option<DriveItem>,
}

/// A folder inside the drive — one node of the folder picker's lazy tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OneDriveFolder {
    pub id: String,
    pub name: String,
}

fn parse_item(v: &Value) -> Option<DriveItem> {
    let id = v.get("id")?.as_str()?.to_string();
    let is_folder = v.get("folder").is_some();
    let file_facet = v.get("file");
    let is_file = file_facet.is_some();
    let hashes = file_facet.and_then(|f| f.get("hashes"));
    Some(DriveItem {
        id,
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_string(),
        mime_type: file_facet
            .and_then(|f| f.get("mimeType"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        modified_time: v
            .get("lastModifiedDateTime")
            .and_then(Value::as_str)
            .map(String::from),
        quick_xor_hash: hashes
            .and_then(|h| h.get("quickXorHash"))
            .and_then(Value::as_str)
            .map(String::from),
        sha256_hash: hashes
            .and_then(|h| h.get("sha256Hash"))
            .and_then(Value::as_str)
            .map(String::from),
        size: v.get("size").and_then(Value::as_i64),
        is_folder,
        is_file,
        web_url: v.get("webUrl").and_then(Value::as_str).map(String::from),
    })
}

fn parse_delta_entry(v: &Value) -> Option<DriveDelta> {
    let id = v.get("id")?.as_str()?.to_string();
    let removed = v.get("deleted").is_some();
    // Keep the parsed item only when it's a live (non-removed) FILE; folders, root, and tombstones
    // carry no file payload to index.
    let file = if removed {
        None
    } else {
        parse_item(v).filter(|it| it.is_file && !it.is_folder)
    };
    Some(DriveDelta {
        item_id: id,
        removed,
        file,
    })
}

/// Parse a delta page → its entries + `(@odata.nextLink, @odata.deltaLink)`. `nextLink` paginates the
/// current run; `deltaLink` (only on the final page) is the cursor to store for next time.
pub fn parse_delta(value: &Value) -> (Vec<DriveDelta>, Option<String>, Option<String>) {
    let entries = value
        .get("value")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_delta_entry).collect())
        .unwrap_or_default();
    let next = value
        .get("@odata.nextLink")
        .and_then(Value::as_str)
        .map(String::from);
    let delta = value
        .get("@odata.deltaLink")
        .and_then(Value::as_str)
        .map(String::from);
    (entries, next, delta)
}

/// Parse a `children` page → its items (files + folders) + the next page link.
pub fn parse_children(value: &Value) -> (Vec<DriveItem>, Option<String>) {
    let items = value
        .get("value")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_item).collect())
        .unwrap_or_default();
    let next = value
        .get("@odata.nextLink")
        .and_then(Value::as_str)
        .map(String::from);
    (items, next)
}

/// Parse a `children` page → only its folders (for the picker tree) + the next page link.
pub fn parse_folders(value: &Value) -> (Vec<OneDriveFolder>, Option<String>) {
    let (items, next) = parse_children(value);
    let folders = items
        .into_iter()
        .filter(|it| it.is_folder)
        .map(|it| OneDriveFolder {
            id: it.id,
            name: it.name,
        })
        .collect();
    (folders, next)
}

/// Map a delta entry onto a foundation [`ChangeEvent`], given the already-namespaced source id — the
/// **pure heart of detection**. `None` means "skip" (a non-removal change that isn't a file — a
/// folder/root entry, nothing to index). A rename/move in OneDrive keeps the same itemId (the stable
/// source id), so a pure move is an `Update` with an unchanged hash → the reducer no-ops it.
fn change_event(source_id: String, delta: &DriveDelta, known: bool) -> Option<ChangeEvent> {
    if delta.removed {
        return Some(ChangeEvent::Delete { source_id });
    }
    let file = delta.file.as_ref()?;
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

/// Map a delta entry (the `onedrive:<email>:<itemId>` namespace).
pub fn map_change(delta: &DriveDelta, email: &str, known: bool) -> Option<ChangeEvent> {
    change_event(source_id_for(email, &delta.item_id), delta, known)
}

/// How to turn a file's bytes into indexable text, decided purely by its MIME type. (No `Export` arm
/// like Drive — OneDrive has no provider-native docs; Office files are real OOXML the sidecar reads.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchPlan {
    /// A file whose bytes are already text — download and decode as UTF-8.
    DownloadText,
    /// A binary document (pdf/docx/…) — download to a temp file and convert via the sidecar.
    DownloadBinary,
    /// Nothing useful to index — skip.
    Skip,
}

/// Decide how to fetch a file's text from its MIME type (pure, unit-tested).
pub fn fetch_plan(mime: &str) -> FetchPlan {
    match mime {
        "" => FetchPlan::Skip,
        "application/json" | "application/xml" => FetchPlan::DownloadText,
        m if m.starts_with("text/") => FetchPlan::DownloadText,
        _ => FetchPlan::DownloadBinary,
    }
}

// --- network (async, DB-free; callers hold no lock across these — rule #4) -----------------------

fn root_delta_url() -> String {
    format!("{}/me/drive/root/delta", microsoft::GRAPH_API)
}

fn root_children_url() -> String {
    format!(
        "{}/me/drive/root/children?$select={SELECT_ITEM}&$top=200",
        microsoft::GRAPH_API
    )
}

fn item_children_url(parent_id: &str) -> String {
    format!(
        "{}/me/drive/items/{parent_id}/children?$select={SELECT_ITEM}&$top=200",
        microsoft::GRAPH_API
    )
}

/// The account a fresh token grants (email + display name), via Graph `/me`. Uses the in-hand token
/// (not yet persisted), so it runs right after consent to learn which account to save under.
pub async fn me_account(token: &microsoft::Token) -> Result<(String, String)> {
    microsoft::me(token).await
}

/// Pull the whole-drive delta since `cursor` (a stored `@odata.deltaLink` URL) + the next delta link.
/// `cursor = None` is the first sync / a re-baseline: the no-token `/root/delta` returns the full
/// enumeration AND a fresh delta link in one walk. Follows `@odata.nextLink` across pages.
pub async fn list_delta(
    token_key: &str,
    cursor: Option<&str>,
) -> Result<(Vec<DriveDelta>, String)> {
    let mut url = cursor.map(String::from).unwrap_or_else(root_delta_url);
    let mut all = Vec::new();
    for _ in 0..MAX_PAGES {
        let v = microsoft::authorized_get(token_key, &url).await?;
        let (entries, next, delta) = parse_delta(&v);
        all.extend(entries);
        match (next, delta) {
            (Some(n), _) => url = n,
            (None, Some(link)) => return Ok((all, link)),
            (None, None) => {
                return Err(Error::Other(
                    "OneDrive delta returned no continuation token.".into(),
                ))
            }
        }
    }
    Err(Error::Other(
        "OneDrive delta exceeded the page guard — too many changes in one pass.".into(),
    ))
}

/// Enumerate the files under the selected folders (recursively, breadth via a queue), deduped, each
/// folder walked once even if reachable from two selections. Folders themselves are never returned —
/// only the files beneath them. The folder-scoped counterpart to [`list_delta`].
pub async fn enumerate_folders(token_key: &str, roots: &[String]) -> Result<Vec<DriveItem>> {
    use std::collections::HashSet;
    let mut out: Vec<DriveItem> = Vec::new();
    let mut seen_folders: HashSet<String> = HashSet::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = roots.to_vec();
    let mut nodes = 0usize;
    while let Some(folder) = queue.pop() {
        if !seen_folders.insert(folder.clone()) {
            continue; // a folder reachable two ways (nested selections) — walk it once.
        }
        nodes += 1;
        if nodes > MAX_PAGES {
            eprintln!("onedrive: folder walk hit the node guard at {MAX_PAGES} folders");
            break;
        }
        let mut url = item_children_url(&folder);
        loop {
            let v = microsoft::authorized_get(token_key, &url).await?;
            let (children, next) = parse_children(&v);
            for child in children {
                if child.is_folder {
                    queue.push(child.id);
                } else if child.is_file && seen_files.insert(child.id.clone()) {
                    out.push(child);
                }
            }
            match next {
                Some(n) => url = n,
                None => break,
            }
        }
    }
    Ok(out)
}

/// The immediate subfolders of `parent_id` (or the drive root when `None`) — one lazy level of the
/// folder picker. Sorted by name for a stable tree.
pub async fn list_folders(token_key: &str, parent_id: Option<&str>) -> Result<Vec<OneDriveFolder>> {
    let mut url = match parent_id {
        Some(id) => item_children_url(id),
        None => root_children_url(),
    };
    let mut out = Vec::new();
    for _ in 0..MAX_PAGES {
        let v = microsoft::authorized_get(token_key, &url).await?;
        let (folders, next) = parse_folders(&v);
        out.extend(folders);
        match next {
            Some(n) => url = n,
            None => break,
        }
    }
    out.sort_by_key(|f| f.name.to_lowercase());
    Ok(out)
}

/// Fetch one file's metadata (for body-on-demand, where we hold only the stored pointer).
pub async fn fetch_item(token_key: &str, item_id: &str) -> Result<DriveItem> {
    let url = format!(
        "{}/me/drive/items/{item_id}?$select={SELECT_ITEM}",
        microsoft::GRAPH_API
    );
    let v = microsoft::authorized_get(token_key, &url).await?;
    parse_item(&v).ok_or_else(|| Error::Other("OneDrive returned no item for that id.".into()))
}

/// True if a Graph error is the "delta token expired" signal (HTTP 410 / `resyncRequired`) — discard
/// the cursor and re-baseline with a full delta.
pub fn is_cursor_expired(err: &Error) -> bool {
    let s = err.to_string();
    s.contains("(410") || s.contains("resyncRequired")
}

/// True if a Graph error is an auth failure (revoked/expired) for the whole account — the signal to
/// fan the account out to `unreachable` rather than treat it as mass deletion.
pub fn is_auth_failure(err: &Error) -> bool {
    let s = err.to_string();
    s.contains("(401") || s.contains("(403")
}

/// Fetch a file's body as indexable text, or `None` if it has no useful text (skipped type, empty, or
/// over the size cap). Text files are downloaded directly; binaries downloaded to a temp file and
/// converted via the sidecar. Never holds the DB lock; the sidecar must already be installed.
pub async fn fetch_body(
    state: &AppState,
    token_key: &str,
    item: &DriveItem,
) -> Result<Option<String>> {
    match fetch_plan(&item.mime_type) {
        FetchPlan::Skip => Ok(None),
        FetchPlan::DownloadText => {
            let url = format!(
                "{}/me/drive/items/{}/content",
                microsoft::GRAPH_API,
                item.id
            );
            let bytes = microsoft::authorized_get_bytes(token_key, &url, MAX_FILE_BYTES).await?;
            Ok(non_empty(&String::from_utf8_lossy(&bytes)))
        }
        FetchPlan::DownloadBinary => {
            let url = format!(
                "{}/me/drive/items/{}/content",
                microsoft::GRAPH_API,
                item.id
            );
            let bytes = match microsoft::authorized_get_bytes(token_key, &url, MAX_FILE_BYTES).await
            {
                Ok(b) => b,
                // An over-cap download is a skip (kept findable via its title), not a hard error.
                Err(e) if e.to_string().contains("too large") => return Ok(None),
                Err(e) => return Err(e),
            };
            let tmp = stage_temp(&item.name, &bytes)?;
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
/// (MarkItDown) picks the right converter. Content-addressed so a re-fetch reuses the name; removed by
/// the caller after conversion.
fn stage_temp(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 8)
        .unwrap_or("bin");
    let digest = ingest::hex_digest(bytes);
    let path = std::env::temp_dir().join(format!("pm-onedrive-{}.{ext}", &digest[..16]));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

// --- progress event (rendered by the shared IngestProgress component) ----------------------------

/// A file PM tried to index but couldn't, surfaced in the post-sync report so the user knows what was
/// left out (e.g. an unsupported file type, or a fetch error). Not fatal — the sync carries on.
#[derive(Clone, Serialize, Default)]
pub struct OneDriveSyncIssue {
    pub name: String,
    pub reason: String,
}

/// The outcome of a sync pass: how many items were indexed/updated/removed, the list of files that
/// couldn't be indexed (capped), and whether the user stopped it early.
#[derive(Clone, Serialize, Default)]
pub struct OneDriveSyncReport {
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    /// The user pressed Stop — already-indexed files are kept; the rest were left for next time.
    pub cancelled: bool,
    /// Files attempted but not indexed (unsupported/empty, or a fetch error), capped for memory.
    pub issues: Vec<OneDriveSyncIssue>,
    /// True when more files couldn't be indexed than the capped `issues` list holds.
    pub issues_truncated: bool,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OneDriveSyncEvent {
    /// The total number of files/changes this run will work through (sent once, before the items).
    Counted { total: usize },
    /// One item processed (1-based `processed` of `total`).
    Item {
        processed: usize,
        total: usize,
        name: String,
    },
    /// The run is done; `report` carries the breakdown + the not-indexed list (+ a `cancelled` flag).
    Finished { report: OneDriveSyncReport },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, mime: &str, qxor: Option<&str>, modified: &str) -> DriveItem {
        DriveItem {
            id: id.into(),
            name: format!("{id}.bin"),
            mime_type: mime.into(),
            modified_time: Some(modified.into()),
            quick_xor_hash: qxor.map(String::from),
            sha256_hash: None,
            size: Some(10),
            is_folder: false,
            is_file: true,
            web_url: Some(format!("https://onedrive/{id}")),
        }
    }

    #[test]
    fn source_id_namespacing_round_trips_and_matches_the_fanout_shape() {
        let sid = source_id_for("a@b.com", "01ITEM!23");
        assert_eq!(sid, "onedrive:a@b.com:01ITEM!23");
        // The fan-out matches `onedrive:<email>:%`, so the account prefix must be exactly this.
        assert!(sid.starts_with("onedrive:a@b.com:"));
        assert_eq!(account_of(&sid).as_deref(), Some("a@b.com"));
        assert_eq!(account_of("gdrive:a@b.com:FILE"), None);
        assert_eq!(account_of("not-an-id"), None);
    }

    #[test]
    fn finalize_or_flag_advances_on_a_clean_pass_and_holds_last_good_on_a_failed_one() {
        // F-29: the engine routes each account through `finalize_or_flag(.., account_failed, ..)`. This
        // drives that decision directly (not just the helpers it calls): a clean pass advances the
        // cursor + stamps the time + state 'ok'; a failed pass flips state to 'error' and — even when a
        // *fresh* cursor is offered — leaves the cursor and last-good time exactly as they were, so the
        // failed items retry next sync and the Connectors warning shows. A revert of the branch back to
        // an unconditional finalize would advance the cursor on the failed pass and fail this test.
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), key).unwrap();
        upsert_account(&conn, "a@b.com", "A").unwrap();

        let row = |c: &Connection| -> (String, Option<String>) {
            c.query_row(
                "SELECT state, last_synced_at FROM connector_sources WHERE id = ?1",
                params![account_id("a@b.com")],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };

        // A clean pass (account_failed = false) commits: cursor advanced, time stamped, state 'ok'.
        finalize_or_flag(&conn, "a@b.com", false, Some("CUR1")).unwrap();
        let (state_ok, synced_ok) = row(&conn);
        assert_eq!(state_ok, "ok");
        assert!(synced_ok.is_some(), "a clean pass stamps last_synced_at");
        assert_eq!(
            get_cursor(&conn, "a@b.com").unwrap().as_deref(),
            Some("CUR1")
        );

        // A failed pass (account_failed = true) takes the error path: even though a fresh "CUR2" cursor
        // is offered, state flips to 'error' and the cursor + last-good time are left exactly as the
        // clean pass set them.
        finalize_or_flag(&conn, "a@b.com", true, Some("CUR2")).unwrap();
        let (state_err, synced_err) = row(&conn);
        assert_eq!(state_err, "error");
        assert_eq!(
            synced_err, synced_ok,
            "the failure path must not restamp last_synced_at"
        );
        assert_eq!(
            get_cursor(&conn, "a@b.com").unwrap().as_deref(),
            Some("CUR1"),
            "the failure path must not advance the cursor",
        );
    }

    #[test]
    fn scope_defaults_to_whole_drive_and_round_trips_folders() {
        let def = OneDriveScope::default();
        assert!(def.folders.is_none()); // whole drive (delta cursor)
        let parsed: OneDriveScope = serde_json::from_str("{}").unwrap();
        assert!(parsed.folders.is_none());
        let scoped: OneDriveScope = serde_json::from_str(r#"{"folders":["f1","f2"]}"#).unwrap();
        assert_eq!(
            scoped.folders.as_deref(),
            Some(&["f1".to_string(), "f2".to_string()][..])
        );
    }

    #[test]
    fn content_hash_prefers_quickxor_then_sha256_then_modified() {
        let with_xor = file(
            "f1",
            "application/pdf",
            Some("XOR=="),
            "2026-06-27T00:00:00Z",
        );
        assert_eq!(with_xor.content_hash().as_deref(), Some("XOR=="));
        let mut with_sha = file("f2", "application/pdf", None, "2026-06-27T00:00:00Z");
        with_sha.sha256_hash = Some("SHA".into());
        assert_eq!(with_sha.content_hash().as_deref(), Some("SHA"));
        let bare = file("f3", "text/plain", None, "2026-06-27T01:00:00Z");
        assert_eq!(bare.content_hash().as_deref(), Some("2026-06-27T01:00:00Z"));
    }

    #[test]
    fn parse_item_reads_file_folder_and_tombstone_shapes() {
        let f = serde_json::json!({
            "id": "01F", "name": "A.pdf",
            "file": {"mimeType": "application/pdf", "hashes": {"quickXorHash": "QX"}},
            "lastModifiedDateTime": "2026-06-27T00:00:00Z", "size": 99,
            "webUrl": "https://onedrive/01F"
        });
        let parsed = parse_item(&f).unwrap();
        assert!(parsed.is_file && !parsed.is_folder);
        assert_eq!(parsed.mime_type, "application/pdf");
        assert_eq!(parsed.quick_xor_hash.as_deref(), Some("QX"));

        let folder =
            serde_json::json!({"id": "01D", "name": "Reports", "folder": {"childCount": 3}});
        let pf = parse_item(&folder).unwrap();
        assert!(pf.is_folder && !pf.is_file);

        // A deletion delta entry → removed, no file payload.
        let del = serde_json::json!({"id": "01G", "name": "gone", "deleted": {"state": "deleted"}});
        let d = parse_delta_entry(&del).unwrap();
        assert!(d.removed);
        assert!(d.file.is_none());
    }

    #[test]
    fn parse_delta_reads_value_next_and_delta_links() {
        let page1 = serde_json::json!({
            "value": [
                {"id": "01F", "name": "A.pdf", "file": {"mimeType": "application/pdf"}, "lastModifiedDateTime": "2026-06-27T00:00:00Z"},
                {"id": "01D", "name": "Folder", "folder": {}}
            ],
            "@odata.nextLink": "https://graph/next"
        });
        let (entries, next, delta) = parse_delta(&page1);
        assert_eq!(next.as_deref(), Some("https://graph/next"));
        assert!(delta.is_none());
        assert_eq!(entries.len(), 2);
        assert!(entries[0].file.is_some()); // the file
        assert!(entries[1].file.is_none()); // the folder → no payload

        let last = serde_json::json!({
            "value": [{"id": "01G", "deleted": {}}],
            "@odata.deltaLink": "https://graph/delta?token=TOK"
        });
        let (entries, next, delta) = parse_delta(&last);
        assert!(next.is_none());
        assert_eq!(delta.as_deref(), Some("https://graph/delta?token=TOK"));
        assert!(entries[0].removed);
    }

    #[test]
    fn map_change_covers_remove_add_update_and_skips_folders() {
        let f = file("01F", "application/pdf", Some("h1"), "2026-06-27T00:00:00Z");
        let removed = DriveDelta {
            item_id: "01F".into(),
            removed: true,
            file: None,
        };
        assert_eq!(
            map_change(&removed, "a@b.com", true),
            Some(ChangeEvent::Delete {
                source_id: "onedrive:a@b.com:01F".into()
            })
        );
        let changed = DriveDelta {
            item_id: "01F".into(),
            removed: false,
            file: Some(f.clone()),
        };
        assert_eq!(
            map_change(&changed, "a@b.com", false),
            Some(ChangeEvent::Add {
                source_id: "onedrive:a@b.com:01F".into(),
                modified_at: Some("2026-06-27T00:00:00Z".into())
            })
        );
        assert_eq!(
            map_change(&changed, "a@b.com", true),
            Some(ChangeEvent::Update {
                source_id: "onedrive:a@b.com:01F".into(),
                modified_at: Some("2026-06-27T00:00:00Z".into()),
                new_content_hash: Some("h1".into())
            })
        );
        // A non-removal entry that isn't a file (folder/root) → skip.
        let folder = DriveDelta {
            item_id: "01D".into(),
            removed: false,
            file: None,
        };
        assert_eq!(map_change(&folder, "a@b.com", false), None);
    }

    #[test]
    fn fetch_plan_routes_each_mime() {
        assert_eq!(fetch_plan("text/plain"), FetchPlan::DownloadText);
        assert_eq!(fetch_plan("text/markdown"), FetchPlan::DownloadText);
        assert_eq!(fetch_plan("application/json"), FetchPlan::DownloadText);
        assert_eq!(fetch_plan("application/pdf"), FetchPlan::DownloadBinary);
        assert_eq!(
            fetch_plan("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            FetchPlan::DownloadBinary
        );
        assert_eq!(fetch_plan(""), FetchPlan::Skip);
    }
}
