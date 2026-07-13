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
//! `gdrive:<email>`), its own keychain token (`account_token_key`), and namespaces its **My Drive**
//! item ids `gdrive:<email>:<fileId>` — so the foundation's `source_id LIKE 'gdrive:<email>:%'` fan-out
//! flips a single account to `unreachable` on an auth failure without touching the others.
//!
//! **Shared (Team) drives are de-duplicated across accounts** (v19). Their files are the same whoever
//! reaches them, so they're indexed ONCE under an account-independent id `gdrive:sd:<driveId>:<fileId>`
//! by whichever account syncs the drive first (its "owner"). The `shared_drive_access` relation records
//! which accounts can reach each drive and which one owns it; other accounts with access don't
//! re-index it (the scope UI greys those out), and a drive's items only go `unreachable` once NO
//! connected account can reach it.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cloud_sync::{convert_downloaded_binary, non_empty, stage_temp};
use crate::error::{Error, Result};
use crate::google::{self, Token};
use crate::index_only::{self, ChangeEvent, ItemState};
use crate::{connector_sync, secrets, AppState};

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
/// Google Sheets API v4 base — used ONLY for the metadata-only Google Sheets index (tab names + each
/// tab's header row + grid dimensions), never to read the full grid.
const SHEETS_API: &str = "https://sheets.googleapis.com/v4/spreadsheets";
/// The Google-native Sheet MIME type (routed to the metadata-only path, not a full-grid CSV export).
const SHEET_MIME: &str = "application/vnd.google-apps.spreadsheet";
/// The export MIME for pulling a Google Sheet's FULL grid as an `.xlsx` workbook — every tab
/// preserved with its cell types. Used ONLY by the "import fully" promote flow ([`export_sheet_xlsx`]);
/// the index-only sync never fetches the grid. `.xlsx` (not CSV) so a multi-tab workbook survives in
/// one file, which the local spreadsheet processor then parses tab-by-tab.
const SHEET_EXPORT_XLSX_MIME: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Whether a Drive MIME type is a Google Sheet — the promote flow gate (only a Sheet has a full grid
/// to import). `SHEET_MIME` is module-private, so this is the public predicate the command uses.
pub fn is_sheet(mime: &str) -> bool {
    mime == SHEET_MIME
}
/// Cap on a single fetched/exported file body (25 MiB) so one huge file can't balloon memory; an
/// over-cap file is skipped with a surfaced note rather than indexed.
const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
/// Runaway guard on pagination (≈200k files) — a backstop against a never-clearing page token, not a
/// coverage cap; if it ever trips we log it (we do not silently drop the rest).
const MAX_PAGES: usize = 1000;
/// The field projection for a Drive file — kept tight (only what the connector needs). `parents` is
/// requested explicitly (Drive API v3 returns nothing not named here) so a synced file can be tagged
/// with the folder it was found in; a file usually has one parent, and we keep only the first.
const FILE_FIELDS: &str = "id,name,mimeType,modifiedTime,md5Checksum,trashed,webViewLink,parents";
/// Drive's folder MIME type (folders are containers we walk, never files we index).
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

/// My Drive's root-folder alias. Doubles as the sentinel `drive_id` the folder picker / enumeration
/// pass to mean "the personal drive" rather than a shared drive's id (`'root' in parents` lists
/// My Drive's top level; shared-drive ids never equal the literal `"root"`).
pub const MY_DRIVE_ROOT: &str = "root";

const PROVIDER: &str = "google";
const SERVICE: &str = "drive";

// --- identity / namespacing ---------------------------------------------------------------------

/// The keychain token key for one Drive account (`<prefix><email>`).
pub fn account_token_key(email: &str) -> String {
    secrets::token_key_for(PROVIDER, SERVICE, email).expect("google/drive is a token-bearing pair")
}

/// The `connector_sources.id` for one account, and the item-id namespace prefix.
fn account_id(email: &str) -> String {
    format!("gdrive:{email}")
}

/// The stable index-only `source_id` for one **My Drive** file under one account:
/// `gdrive:<email>:<fileId>`.
pub fn source_id_for(email: &str, file_id: &str) -> String {
    format!("gdrive:{email}:{file_id}")
}

/// The stable index-only `source_id` for a file in a **shared drive**: `gdrive:sd:<driveId>:<fileId>`.
/// **Account-independent** (no email) — a shared drive's files are the same whichever account reaches
/// them (Drive file ids are globally stable), so PM indexes them ONCE under this namespace rather than
/// once per account (board card 4A dedup, v19). Which account owns/reconciles the drive lives in the
/// `shared_drive_access` relation. The `sd:<driveId>` segment still isolates one drive for reconcile.
pub fn shared_source_id(drive_id: &str, file_id: &str) -> String {
    format!("gdrive:sd:{drive_id}:{file_id}")
}

/// The `source_id` prefix that matches every indexed item of one shared drive (for reconcile +
/// per-drive cleanup): `gdrive:sd:<driveId>:`.
fn shared_prefix(drive_id: &str) -> String {
    format!("gdrive:sd:{drive_id}:")
}

/// Recover the account email from a **My Drive** source id (`gdrive:<email>:<fileId>`). Shared-drive
/// ids are account-independent (`gdrive:sd:<driveId>:<fileId>`), so they have no owning account here —
/// [`token_key_for_source`] resolves an account that can reach them instead. An email carries no `:`,
/// so the first `:` after the prefix splits it off.
pub fn account_of(source_id: &str) -> Option<String> {
    let rest = source_id.strip_prefix("gdrive:")?;
    if rest.starts_with("sd:") {
        return None; // a shared-drive id — no single owning account in the id itself
    }
    rest.split_once(':').map(|(email, _)| email.to_string())
}

/// The shared drive id embedded in a shared-drive source id (`gdrive:sd:<driveId>:<fileId>`), or None
/// for a My Drive / non-Drive id.
pub fn shared_drive_of(source_id: &str) -> Option<String> {
    source_id
        .strip_prefix("gdrive:sd:")?
        .split_once(':')
        .map(|(drive_id, _)| drive_id.to_string())
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
    /// Whether this account's token carries the `spreadsheets.readonly` scope. `false` for accounts
    /// that last consented before Sheets support existed — the UI shows a "Reconnect for Sheets" prompt,
    /// and their Google Sheets index by name only until they re-consent. Read from the keychain token,
    /// no network.
    pub has_sheets_scope: bool,
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
        // Whether this account granted the Sheets scope (keychain read, no network). Drives the
        // "Reconnect for Sheets" prompt; a keychain error reads as "not granted" rather than failing
        // the whole account list.
        let has_sheets_scope =
            google::token_has_scope(&account_token_key(&email), google::SHEETS_SCOPE)
                .unwrap_or(false);
        // This account's My Drive items, plus the shared-drive items of any drive it OWNS (shared
        // drives are account-independent and indexed once by their owner — a non-owner counts only
        // its own My Drive).
        let indexed: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents d \
                 WHERE d.source_type = 'index_only' AND ( \
                     d.source_id LIKE ?1 || ':%' \
                     OR EXISTS (SELECT 1 FROM shared_drive_access a \
                                WHERE a.account_id = ?1 AND a.is_owner = 1 \
                                  AND d.source_id LIKE 'gdrive:sd:' || a.drive_id || ':%') )",
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
            has_sheets_scope,
        });
    }
    Ok(out)
}

// --- delta cursors -------------------------------------------------------------------------------
//
// The `connector_sources.cursor` column holds a JSON **map** of changes-feed page tokens, one per
// independently-tracked corpus: key `"my"` for the personal My Drive, and one key per *whole-drive*
// shared selection (its driveId). Folder-scoped shared selections have no cursor — the changes feed
// can't be scoped to folders, so they re-enumerate + reconcile instead (see `gather_shared`). A
// legacy bare token (pre-shared-drives, when the column held just the My-Drive token) is still read
// as the `"my"` cursor and upgraded to the map shape on the next clean sync.

/// The cursor-map key for the personal My Drive changes feed (shared selections key on their driveId,
/// which is never the literal `"my"`).
const MY_DRIVE_CURSOR_KEY: &str = "my";

type CursorMap = std::collections::BTreeMap<String, String>;

/// Decode the `cursor` column: a JSON object → that map; a non-empty bare string → a legacy My-Drive
/// token; absent/empty/garbage → an empty map.
fn decode_cursors(raw: Option<String>) -> CursorMap {
    match raw {
        Some(s) if s.trim_start().starts_with('{') => serde_json::from_str(&s).unwrap_or_default(),
        Some(s) if !s.trim().is_empty() => {
            let mut m = CursorMap::new();
            m.insert(MY_DRIVE_CURSOR_KEY.to_string(), s);
            m
        }
        _ => CursorMap::new(),
    }
}

/// Read an account's whole cursor map.
fn read_cursors(conn: &Connection, email: &str) -> Result<CursorMap> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT cursor FROM connector_sources WHERE id = ?1",
            params![account_id(email)],
            |r| r.get(0),
        )
        .optional()
        .map(Option::flatten)?;
    Ok(decode_cursors(raw))
}

/// The stored My-Drive delta cursor for an account, if any.
pub fn get_cursor(conn: &Connection, email: &str) -> Result<Option<String>> {
    Ok(read_cursors(conn, email)?.remove(MY_DRIVE_CURSOR_KEY))
}

/// The stored delta cursor for one whole-drive shared selection (keyed on its driveId), if any.
pub fn get_shared_cursor(conn: &Connection, email: &str, drive_id: &str) -> Result<Option<String>> {
    Ok(read_cursors(conn, email)?.remove(drive_id))
}

/// Record a clean sync: advance the My-Drive cursor (when My Drive synced this pass) and any
/// whole-drive shared cursors that advanced, stamp the time, and clear any failure state. Cursors for
/// corpora not touched this pass are left as-is, so a drive that errored mid-pass retries from its
/// existing token next time.
pub fn finalize_sync(
    conn: &Connection,
    email: &str,
    my_cursor: Option<&str>,
    shared_cursors: &[(String, String)],
) -> Result<()> {
    let mut cursors = read_cursors(conn, email)?;
    if let Some(c) = my_cursor {
        cursors.insert(MY_DRIVE_CURSOR_KEY.to_string(), c.to_string());
    }
    for (drive_id, cursor) in shared_cursors {
        cursors.insert(drive_id.clone(), cursor.clone());
    }
    let json = serde_json::to_string(&cursors)
        .map_err(|e| Error::Other(format!("encode drive cursors: {e}")))?;
    conn.execute(
        "UPDATE connector_sources \
         SET cursor = ?2, last_synced_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), state = 'ok' \
         WHERE id = ?1",
        params![account_id(email), json],
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
    my_cursor: Option<&str>,
    shared_cursors: &[(String, String)],
) -> Result<()> {
    if account_failed {
        set_state(conn, email, "error")
    } else {
        finalize_sync(conn, email, my_cursor, shared_cursors)
    }
}

/// Disconnect one account: soft-flag its items `unreachable` (kept findable), drop the registry row,
/// and forget its token plus any per-account (Advanced-Protection) client. Never hard-deletes the
/// indexed documents.
///
/// **My Drive** items (`gdrive:<email>:%`) belong to this account, so they're flagged outright.
/// **Shared-drive** items are account-independent and may still be reachable by another connected
/// account — so they're only flagged once NO remaining account can reach the drive. The registry row
/// is dropped *between* the two so the cascade clears this account's `shared_drive_access` rows first
/// (leaving any still-reachable drive owner-less, to be re-claimed on another account's next sync).
pub fn forget_account(conn: &Connection, email: &str) -> Result<()> {
    let account = account_id(email);
    conn.execute(
        "UPDATE documents SET source_state = 'unreachable' \
         WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%'",
        params![account],
    )?;
    // Shared drives this account could reach — captured before the cascade removes its access rows.
    let drives: Vec<String> = {
        let mut stmt =
            conn.prepare("SELECT drive_id FROM shared_drive_access WHERE account_id = ?1")?;
        let rows: Vec<String> = stmt
            .query_map(params![account], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        rows
    };
    conn.execute(
        "DELETE FROM connector_sources WHERE id = ?1",
        params![account],
    )?;
    for drive_id in drives {
        soft_flag_orphaned_shared_drive(conn, &drive_id)?;
    }
    secrets::clear_google_token_for(&account_token_key(email)).ok();
    // Forget the account's own Cloud-project client too, so reconnecting later with the shared
    // client isn't silently overridden by stale per-account creds (see `client_creds_for_key`).
    secrets::clear_google_client_for_account(email).ok();
    Ok(())
}

/// If NO connected account can still reach shared drive `drive_id`, soft-flag its items `unreachable`
/// (kept findable, never deleted). A no-op while any account retains access — that account (re)claims
/// ownership and keeps the drive indexed.
fn soft_flag_orphaned_shared_drive(conn: &Connection, drive_id: &str) -> Result<()> {
    let still_reachable: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM shared_drive_access WHERE drive_id = ?1)",
        params![drive_id],
        |r| r.get(0),
    )?;
    if !still_reachable {
        conn.execute(
            "UPDATE documents SET source_state = 'unreachable' \
             WHERE source_type = 'index_only' AND source_id LIKE ?1 || '%'",
            params![shared_prefix(drive_id)],
        )?;
    }
    Ok(())
}

/// Record that `email` can reach shared drive `drive_id` (caching `name` for the UI), and decide
/// whether THIS account should index it. The first account to sync a drive claims ownership and
/// indexes it; later accounts with access don't re-index (the scope UI greys those out). Returns true
/// iff this account owns the drive — i.e. the caller should gather + reconcile it this pass.
pub fn claim_or_skip_shared_drive(
    conn: &Connection,
    email: &str,
    drive_id: &str,
    name: &str,
) -> Result<bool> {
    let account = account_id(email);
    conn.execute(
        "INSERT INTO shared_drive_access(drive_id, account_id, name) VALUES (?1, ?2, ?3) \
         ON CONFLICT(drive_id, account_id) DO UPDATE SET name = excluded.name",
        params![drive_id, account, name],
    )?;
    let owner: Option<String> = conn
        .query_row(
            "SELECT account_id FROM shared_drive_access WHERE drive_id = ?1 AND is_owner = 1",
            params![drive_id],
            |r| r.get(0),
        )
        .optional()?;
    match owner {
        Some(o) if o == account => Ok(true),
        Some(_) => Ok(false), // already owned + indexed by another account
        None => {
            conn.execute(
                "UPDATE shared_drive_access SET is_owner = 1 \
                 WHERE drive_id = ?1 AND account_id = ?2",
                params![drive_id, account],
            )?;
            Ok(true)
        }
    }
}

/// Shared drives already owned (indexed) by a DIFFERENT account → `driveId → owner email`. The scope
/// picker greys these out for `email` with a "synced by <owner>" note instead of re-indexing them.
pub fn shared_drive_owners_elsewhere(
    conn: &Connection,
    email: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let me = account_id(email);
    let mut stmt = conn.prepare(
        "SELECT drive_id, account_id FROM shared_drive_access WHERE is_owner = 1 AND account_id != ?1",
    )?;
    let rows = stmt.query_map(params![me], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (drive_id, owner_account) = row?;
        let owner_email = owner_account
            .strip_prefix("gdrive:")
            .unwrap_or(&owner_account)
            .to_string();
        map.insert(drive_id, owner_email);
    }
    Ok(map)
}

/// The token key of an account that can reach the source behind `source_id`, for an on-demand body
/// fetch. A **My Drive** id names its account directly. A **shared-drive** id is account-independent,
/// so this resolves an account with access (preferring the owner) from `shared_drive_access`.
pub fn token_key_for_source(conn: &Connection, source_id: &str) -> Result<Option<String>> {
    if let Some(drive_id) = shared_drive_of(source_id) {
        let owner: Option<String> = conn
            .query_row(
                "SELECT account_id FROM shared_drive_access WHERE drive_id = ?1 \
                 ORDER BY is_owner DESC LIMIT 1",
                params![drive_id],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(owner.and_then(|a| a.strip_prefix("gdrive:").map(account_token_key)));
    }
    Ok(account_of(source_id).map(|email| account_token_key(&email)))
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
///
/// Matches on `source_id` ALONE (`include_promoted`) — deliberately NOT restricted to
/// `source_type = 'index_only'` — so a document that was **promoted to a full local import**
/// (["import fully"](crate::ingest::promote_spreadsheet), which flips its `source_type` off
/// `index_only` but keeps the Drive `source_id` as a claim marker) is still seen as the item's
/// current state. That makes the sync reducer treat the promoted file as present-and-reachable: an
/// `Add` re-fires as a `Noop` (never re-ingesting a second, index-only copy) instead of an
/// `IngestNew`. Only index-only and promoted-Drive docs ever carry a `gdrive:` id, so widening the
/// match can't pull in an unrelated row.
pub fn read_item_state(conn: &Connection, source_id: &str) -> Result<Option<ItemState>> {
    index_only::read_item_state(conn, source_id, /* include_promoted */ true)
}

// --- per-account indexing scope (which drives/folders to index) ----------------------------------

/// One shared drive the account opted into, and how much of it to index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSelection {
    /// The shared drive's id (its root folder id, too).
    pub drive_id: String,
    /// Display name (cached from `drives.list` so the UI/label needs no extra call).
    pub name: String,
    /// `None` = the **entire** shared drive; `Some(ids)` = only these folders (recursively). The
    /// default in the UI is folder-scoped (shared drives are often huge and org-wide).
    #[serde(default)]
    pub folders: Option<Vec<String>>,
    /// Subfolders to skip while walking the selected `folders` (a folder id excludes that folder and
    /// its whole subtree). Only meaningful in folder-scoped mode; the whole-drive path can't be
    /// folder-scoped. Empty/absent in older stored scopes, so nothing is excluded by default.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// What an account indexes: My Drive (the whole personal drive, the existing default) plus any
/// opted-in shared drives. Persisted as JSON in `connector_sources.folder_ids` (no migration — the
/// column already exists and was nullable/unused). A missing/empty value means the default scope
/// (My Drive on, no shared drives) so every pre-existing account behaves exactly as before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveScope {
    /// Index the personal My Drive. Default `true`.
    #[serde(default = "yes")]
    pub my_drive: bool,
    /// How much of My Drive to index: `None` = the **entire** personal drive (the default —
    /// delta-cursor sync); `Some(ids)` = only these folders (recursively, re-enumerated + reconciled
    /// each sync, no cursor — the changes feed can't be folder-scoped). Absent in older stored scopes,
    /// so it defaults to whole-drive and every pre-existing account behaves exactly as before.
    #[serde(default)]
    pub my_drive_folders: Option<Vec<String>>,
    /// Subfolders to skip while walking the selected `my_drive_folders` (a folder id excludes that
    /// folder and its whole subtree). Only meaningful in folder-scoped My Drive; the whole-drive delta
    /// path can't be folder-scoped. Empty/absent in older stored scopes, so nothing is excluded by
    /// default.
    #[serde(default)]
    pub my_drive_exclude: Vec<String>,
    /// Opted-in shared drives (each re-enumerated + reconciled per sync).
    #[serde(default)]
    pub shared: Vec<SharedSelection>,
}

fn yes() -> bool {
    true
}

impl Default for DriveScope {
    fn default() -> Self {
        DriveScope {
            my_drive: true,
            my_drive_folders: None,
            my_drive_exclude: Vec::new(),
            shared: Vec::new(),
        }
    }
}

/// Read an account's indexing scope (the default scope when none is stored yet).
pub fn get_scope(conn: &Connection, email: &str) -> Result<DriveScope> {
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
            serde_json::from_str(&s).map_err(|e| Error::Other(format!("bad drive scope: {e}")))
        }
        _ => Ok(DriveScope::default()),
    }
}

/// Persist an account's indexing scope, and **prune stale delta cursors** to match: keep the cursor
/// of any corpus that is still a *whole-drive* selection — My Drive (when on and not folder-scoped)
/// and each whole-drive shared selection — and drop the rest. A corpus that was removed, or switched
/// to folder-scoped (which uses no cursor), thus loses its cursor — so a later switch back to
/// whole-drive re-baselines with a fresh enumeration instead of resuming from a token that predates
/// out-of-scope soft-deletes.
pub fn set_scope(conn: &Connection, email: &str, scope: &DriveScope) -> Result<()> {
    let json = serde_json::to_string(scope)
        .map_err(|e| Error::Other(format!("encode drive scope: {e}")))?;
    let keep: std::collections::HashSet<&str> = scope
        .shared
        .iter()
        .filter(|s| s.folders.is_none())
        .map(|s| s.drive_id.as_str())
        .collect();
    // Keep My Drive's cursor only while it stays a *whole-drive* selection; switching it to
    // folder-scoped (which uses no cursor) drops it, so a later switch back re-baselines with a fresh
    // enumeration instead of resuming from a token that predates out-of-scope soft-deletes.
    let keep_my = scope.my_drive && scope.my_drive_folders.is_none();
    let mut cursors = read_cursors(conn, email)?;
    cursors.retain(|k, _| {
        if k == MY_DRIVE_CURSOR_KEY {
            keep_my
        } else {
            keep.contains(k.as_str())
        }
    });
    let cursor_json = serde_json::to_string(&cursors)
        .map_err(|e| Error::Other(format!("encode drive cursors: {e}")))?;
    conn.execute(
        "UPDATE connector_sources SET folder_ids = ?2, cursor = ?3 WHERE id = ?1",
        params![account_id(email), json, cursor_json],
    )?;
    // Reconcile shared-drive access to the new scope: this account keeps access only to the shared
    // drives still in scope. Any it dropped is released — freeing its ownership for another account to
    // re-claim on that account's next sync, and soft-flagging the drive's items if no account can
    // reach it any more (the same orphan rule as disconnect).
    let keep: std::collections::HashSet<&str> =
        scope.shared.iter().map(|s| s.drive_id.as_str()).collect();
    let had: Vec<String> = {
        let mut stmt =
            conn.prepare("SELECT drive_id FROM shared_drive_access WHERE account_id = ?1")?;
        let rows: Vec<String> = stmt
            .query_map(params![account_id(email)], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        rows
    };
    for drive_id in had {
        if keep.contains(drive_id.as_str()) {
            continue;
        }
        conn.execute(
            "DELETE FROM shared_drive_access WHERE drive_id = ?1 AND account_id = ?2",
            params![drive_id, account_id(email)],
        )?;
        soft_flag_orphaned_shared_drive(conn, &drive_id)?;
    }
    Ok(())
}

/// Every **currently-healthy** (`source_state = 'ok'`) indexed item id belonging to one shared drive
/// — the set the reconcile diffs the live enumeration against. A present id in this set gets an
/// `Update` (catches edits, no-ops otherwise); a present id NOT in it gets an `Add` (ingests a new
/// file, or reactivates one that was previously flagged missing/unreachable — e.g. a folder the user
/// removed and re-added); an id in this set that is no longer present is a deletion.
pub fn known_shared_source_ids(conn: &Connection, drive_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT source_id FROM documents \
         WHERE source_type = 'index_only' AND source_state = 'ok' \
           AND source_id LIKE ?1 || '%'",
    )?;
    let rows: Vec<String> = stmt
        .query_map(params![shared_prefix(drive_id)], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// Every currently-healthy indexed **My Drive** item id for one account — the set the folder-scoped
/// reconcile diffs against (same role as [`known_shared_source_ids`]). My Drive items are
/// `gdrive:<email>:<fileId>`; the `NOT LIKE …:sd:%` excludes shared-drive items, which share the
/// account prefix but are reconciled per shared drive on their own.
pub fn known_my_drive_source_ids(conn: &Connection, email: &str) -> Result<Vec<String>> {
    let prefix = format!("{}:", account_id(email));
    let mut stmt = conn.prepare(
        "SELECT source_id FROM documents \
         WHERE source_type = 'index_only' AND source_state = 'ok' \
           AND source_id LIKE ?1 || '%' AND source_id NOT LIKE ?1 || 'sd:%'",
    )?;
    let rows: Vec<String> = stmt
        .query_map(params![prefix], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
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
    /// The id of the folder this file was found in (Drive's first `parents` entry) — resolved to a
    /// human-readable name at sync time and snapshotted onto the document as sorting-review context.
    /// `None` when Drive reports no parent (e.g. a shared-drive root item) or the field was absent.
    pub parent_id: Option<String>,
}

/// Lets a folder-scoped enumeration reconcile through the shared [`index_only::reconcile_enumeration`]
/// planner. `content_hash` forwards to the inherent method (md5-or-modifiedTime).
impl index_only::EnumeratedFile for DriveFile {
    fn local_id(&self) -> &str {
        &self.id
    }
    fn modified_at(&self) -> Option<String> {
        self.modified_time.clone()
    }
    fn content_hash(&self) -> Option<String> {
        // Method-call syntax resolves to the inherent `DriveFile::content_hash` (inherent methods
        // shadow trait methods of the same name), so this forwards rather than recursing.
        self.content_hash()
    }
}

impl DriveFile {
    /// The source content hash for change detection: Drive's `md5Checksum` when present (binary /
    /// uploaded files), else `modifiedTime` (Google-native docs have no md5; modifiedTime bumps on
    /// every edit, so it is an honest change signal).
    pub fn content_hash(&self) -> Option<String> {
        self.md5.clone().or_else(|| self.modified_time.clone())
    }

    /// A `PointerInput` for the foundation, given the freshly-fetched body and this file's
    /// already-resolved parent-folder name (the caller resolves `parent_id` → name once per sync run;
    /// see [`fetch_folder_name`]). The folder id/name ride on the pointer purely as review context and
    /// never touch the chunker/embedder.
    pub fn pointer(
        &self,
        source_id: String,
        body: String,
        folder_name: Option<String>,
    ) -> index_only::PointerInput {
        index_only::PointerInput {
            source_id,
            title: self.name.clone(),
            external_ref: self.web_view_link.clone(),
            source_modified_at: self.modified_time.clone(),
            source_content_hash: self.content_hash(),
            body,
            source_parent_folder_id: self.parent_id.clone(),
            source_parent_folder_name: folder_name,
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

/// A shared drive the account can see (`drives.list`) — for the "add shared drives" picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SharedDrive {
    pub id: String,
    pub name: String,
}

/// A folder inside a (shared) drive — one node of the folder picker's lazy tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DriveFolder {
    pub id: String,
    pub name: String,
}

/// Parse a `drives.list` page → its shared drives + the next page token.
pub fn parse_shared_drives(value: &Value) -> (Vec<SharedDrive>, Option<String>) {
    let drives = value
        .get("drives")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    Some(SharedDrive {
                        id: d.get("id")?.as_str()?.to_string(),
                        name: d
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("Shared drive")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("nextPageToken")
        .and_then(Value::as_str)
        .map(String::from);
    (drives, next)
}

/// Parse a `files.list` folder page → its folders + the next page token (only id/name projected).
pub fn parse_folders(value: &Value) -> (Vec<DriveFolder>, Option<String>) {
    let folders = value
        .get("files")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some(DriveFolder {
                        id: f.get("id")?.as_str()?.to_string(),
                        name: f
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("Untitled folder")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("nextPageToken")
        .and_then(Value::as_str)
        .map(String::from);
    (folders, next)
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
        // A file can technically have several parents; the first is the folder we tag it with.
        parent_id: v
            .get("parents")
            .and_then(Value::as_array)
            .and_then(|ps| ps.first())
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

/// Map a Drive change onto a foundation [`ChangeEvent`], given the change's already-namespaced source
/// id — the **pure heart of detection**. `None` means "skip" (a non-removal change with no file
/// payload — nothing actionable). A rename in Drive keeps the same fileId (the stable source id), so
/// classification is preserved either way; a content edit bumps the hash and re-embeds.
fn change_event(source_id: String, change: &DriveChange, known: bool) -> Option<ChangeEvent> {
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

/// Map a **My Drive** change (the `gdrive:<email>:<fileId>` namespace).
pub fn map_change(change: &DriveChange, email: &str, known: bool) -> Option<ChangeEvent> {
    change_event(source_id_for(email, &change.file_id), change, known)
}

/// Map a **shared-drive** change (the account-independent `gdrive:sd:<driveId>:<fileId>` namespace).
pub fn map_shared_change(change: &DriveChange, drive_id: &str, known: bool) -> Option<ChangeEvent> {
    change_event(shared_source_id(drive_id, &change.file_id), change, known)
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
    /// A Google Sheet — index METADATA ONLY (tab names + header row + grid dimensions) via the Sheets
    /// API; the full grid is never pulled. This is the "index-only Sheets" path: retrieval surfaces the
    /// `webViewLink`, and a user who wants the cell content promotes it to a full local import.
    SheetMetadata,
    /// Nothing useful to index (folders, forms, drawings, shortcuts) — skip.
    Skip,
}

/// Decide how to fetch a file's text from its MIME type (pure, unit-tested).
pub fn fetch_plan(mime: &str) -> FetchPlan {
    match mime {
        "application/vnd.google-apps.document" => FetchPlan::Export { mime: "text/plain" },
        // A Sheet is indexed metadata-only (never the full grid) — see FetchPlan::SheetMetadata.
        SHEET_MIME => FetchPlan::SheetMetadata,
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

/// A personal-corpus (My Drive) `files.list` URL for an arbitrary `q` — the My-Drive counterpart to
/// `shared_files_url` (no `corpora`/`driveId`, so it stays on the personal drive). Used by the My-Drive
/// folder walk and folder picker.
fn my_files_url(q: &str, page: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/files"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("q", q);
        qp.append_pair("pageSize", "200");
        qp.append_pair("spaces", "drive");
        qp.append_pair("fields", &format!("nextPageToken,files({FILE_FIELDS})"));
        if let Some(t) = page {
            qp.append_pair("pageToken", t);
        }
    }
    Ok(url.to_string())
}

/// Enumerate every non-trashed file in My Drive (paginated) — the first-sync baseline. Returns the
/// files plus whether the page guard tripped (`true` ⇒ INCOMPLETE: the caller must not baseline a
/// cursor past a partial listing — see [`connector_sync::paginate`]).
pub async fn enumerate_drive(token_key: &str) -> Result<(Vec<DriveFile>, bool)> {
    connector_sync::paginate(MAX_PAGES, |page| async move {
        let v = google::authorized_get(token_key, &files_url(page.as_deref())?).await?;
        Ok(parse_files(&v))
    })
    .await
}

// --- shared drives (Team Drives) — a separate corpus from My Drive ------------------------------

fn shared_drives_url(page: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/drives"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("pageSize", "100");
        q.append_pair("fields", "nextPageToken,drives(id,name)");
        if let Some(t) = page {
            q.append_pair("pageToken", t);
        }
    }
    Ok(url.to_string())
}

/// Every shared drive this account can see (`drives.list`) — for the "add shared drives" picker. A
/// picker has no deletion/cursor semantics, so a truncated listing just shows fewer rows; the guard is
/// a pure runaway backstop here (hence the discarded flag).
pub async fn list_shared_drives(token_key: &str) -> Result<Vec<SharedDrive>> {
    let (drives, _truncated) = connector_sync::paginate(MAX_PAGES, |page| async move {
        let v = google::authorized_get(token_key, &shared_drives_url(page.as_deref())?).await?;
        Ok(parse_shared_drives(&v))
    })
    .await?;
    Ok(drives)
}

/// A `files.list` URL scoped to one shared drive (`corpora=drive` + the all-drives flags) for an
/// arbitrary `q`. Shared drives need `supportsAllDrives`/`includeItemsFromAllDrives`; without them the
/// call silently returns nothing.
fn shared_files_url(
    drive_id: &str,
    q: &str,
    fields_inner: &str,
    order_by: &str,
    page: Option<&str>,
) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/files"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("q", q);
        qp.append_pair("corpora", "drive");
        qp.append_pair("driveId", drive_id);
        qp.append_pair("includeItemsFromAllDrives", "true");
        qp.append_pair("supportsAllDrives", "true");
        qp.append_pair("spaces", "drive");
        qp.append_pair("pageSize", "200");
        if !order_by.is_empty() {
            qp.append_pair("orderBy", order_by);
        }
        qp.append_pair("fields", &format!("nextPageToken,files({fields_inner})"));
        if let Some(t) = page {
            qp.append_pair("pageToken", t);
        }
    }
    Ok(url.to_string())
}

/// The immediate subfolders of `parent_id` — one lazy level of the folder picker. `drive_id` selects
/// the corpus: the `MY_DRIVE_ROOT` sentinel walks the **personal** My Drive (its top level passes
/// `parent_id == MY_DRIVE_ROOT`); any other id is a **shared** drive (whose root id equals the drive
/// id, so the top level passes `parent_id == drive_id`).
pub async fn list_folders(
    token_key: &str,
    drive_id: &str,
    parent_id: &str,
) -> Result<Vec<DriveFolder>> {
    let my_drive = drive_id == MY_DRIVE_ROOT;
    let q = format!("'{parent_id}' in parents and mimeType = '{FOLDER_MIME}' and trashed = false");
    // A picker (one lazy level): a truncated listing just shows fewer rows, so the guard flag is
    // discarded. `q` is borrowed (not moved) into each page's future so the `Fn` closure can re-run.
    let (mut out, _truncated) = connector_sync::paginate(MAX_PAGES, |page| {
        let q = q.as_str();
        async move {
            let url = if my_drive {
                my_files_url(q, page.as_deref())?
            } else {
                shared_files_url(drive_id, q, "id,name", "name", page.as_deref())?
            };
            let v = google::authorized_get(token_key, &url).await?;
            Ok(parse_folders(&v))
        }
    })
    .await?;
    // The personal corpus isn't server-sorted by name (no `orderBy`); sort for a stable picker.
    if my_drive {
        out.sort_by_key(|a| a.name.to_lowercase());
    }
    Ok(out)
}

/// Enumerate the files of a shared drive to index: the **whole** drive (`folders == None`), or only
/// the selected folders walked recursively (`Some`, minus any `exclude`d subtrees). Folders
/// themselves are never indexed — only the files beneath them. Deduplicates files reachable from more
/// than one selected folder. `exclude` is ignored in whole-drive mode (the changes feed can't be
/// folder-scoped).
pub async fn enumerate_shared(
    token_key: &str,
    drive_id: &str,
    folders: Option<&[String]>,
    exclude: &[String],
) -> Result<(Vec<DriveFile>, bool)> {
    match folders {
        None => {
            connector_sync::paginate(MAX_PAGES, |page| async move {
                let url = shared_files_url(
                    drive_id,
                    "trashed = false",
                    FILE_FIELDS,
                    "modifiedTime desc",
                    page.as_deref(),
                )?;
                let v = google::authorized_get(token_key, &url).await?;
                Ok(parse_files(&v))
            })
            .await
        }
        Some(roots) => {
            walk_folders(token_key, roots, exclude, |q, page| {
                shared_files_url(drive_id, q, FILE_FIELDS, "", page)
            })
            .await
        }
    }
}

/// Walk a set of root folders (recursively, breadth via a queue), collecting the non-folder files
/// beneath them — deduped, and each folder walked once even if reachable from two selections. The
/// `url_for(q, page)` closure builds each `files.list` page URL, so My Drive (`my_files_url`) and
/// shared drives (`shared_files_url`) share one walk. Folders themselves are never returned.
/// Any folder id in `exclude` is never enqueued — pruning that folder and its whole subtree (its
/// files are only ever discovered by walking into it), both as a seed root and as a descended child.
/// Returns the deduped files plus whether the walk was cut short by the folder-count guard (`true` ⇒
/// INCOMPLETE — the caller must not treat an unseen file as deleted; see [`connector_sync::paginate`]).
async fn walk_folders(
    token_key: &str,
    roots: &[String],
    exclude: &[String],
    url_for: impl Fn(&str, Option<&str>) -> Result<String>,
) -> Result<(Vec<DriveFile>, bool)> {
    use std::collections::HashSet;
    let excluded: HashSet<&str> = exclude.iter().map(String::as_str).collect();
    let mut out: Vec<DriveFile> = Vec::new();
    let mut seen_folders: HashSet<String> = HashSet::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = roots
        .iter()
        .filter(|r| !excluded.contains(r.as_str()))
        .cloned()
        .collect();
    let mut nodes = 0usize;
    while let Some(folder) = queue.pop() {
        if !seen_folders.insert(folder.clone()) {
            continue; // a folder reachable two ways (nested selections) — walk it once.
        }
        nodes += 1;
        if nodes > MAX_PAGES {
            eprintln!("drive: folder walk hit the node guard at {MAX_PAGES} folders");
            return Ok((out, true));
        }
        let q = format!("'{folder}' in parents and trashed = false");
        let mut page: Option<String> = None;
        loop {
            let url = url_for(&q, page.as_deref())?;
            let v = google::authorized_get(token_key, &url).await?;
            let (children, next) = parse_files(&v);
            for child in children {
                if child.mime_type == FOLDER_MIME {
                    if !excluded.contains(child.id.as_str()) {
                        queue.push(child.id);
                    }
                } else if seen_files.insert(child.id.clone()) {
                    out.push(child);
                }
            }
            match next {
                Some(t) => page = Some(t),
                None => break,
            }
        }
    }
    Ok((out, false))
}

/// Enumerate the files under the selected **My Drive** folders (recursively, deduped, minus any
/// `exclude`d subtrees) — the personal counterpart to `enumerate_shared`'s folder branch, for
/// folder-scoped My Drive. Carries the same truncated flag as [`walk_folders`].
pub async fn enumerate_my_folders(
    token_key: &str,
    folders: &[String],
    exclude: &[String],
) -> Result<(Vec<DriveFile>, bool)> {
    walk_folders(token_key, folders, exclude, my_files_url).await
}

/// The delta baseline cursor (`changes.getStartPageToken`). `drive_id: Some` scopes it to one shared
/// drive's change feed; `None` is the personal My Drive.
pub async fn start_page_token(token_key: &str, drive_id: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/changes/startPageToken"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("supportsAllDrives", "true");
        if let Some(d) = drive_id {
            q.append_pair("driveId", d);
        }
    }
    let v = google::authorized_get(token_key, url.as_str()).await?;
    v.get("startPageToken")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| Error::Other("Drive didn't return a change cursor.".into()))
}

fn changes_url(page_token: &str, drive_id: Option<&str>) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/changes"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("pageToken", page_token);
        q.append_pair("pageSize", "200");
        q.append_pair("includeRemoved", "true");
        q.append_pair("spaces", "drive");
        // Scope to a shared drive's own change feed (My Drive omits these → its own feed).
        if let Some(d) = drive_id {
            q.append_pair("driveId", d);
            q.append_pair("includeItemsFromAllDrives", "true");
            q.append_pair("supportsAllDrives", "true");
        }
        q.append_pair(
            "fields",
            &format!("nextPageToken,newStartPageToken,changes(fileId,removed,file({FILE_FIELDS}))"),
        );
    }
    Ok(url.to_string())
}

/// Returns the changes, the cursor to persist, and whether the page guard tripped (`true` ⇒ the caller
/// must NOT advance its stored cursor — the changes seen still apply idempotently, and the next sync
/// resumes from the old cursor; see [`connector_sync::paginate`]). Keeps a hand-rolled loop rather than
/// [`connector_sync::paginate`] because it must also carry the `newStartPageToken` that ends the feed.
async fn list_changes_for(
    token_key: &str,
    drive_id: Option<&str>,
    cursor: &str,
) -> Result<(Vec<DriveChange>, String, bool)> {
    let mut all = Vec::new();
    let mut page = cursor.to_string();
    for _ in 0..MAX_PAGES {
        let v = google::authorized_get(token_key, &changes_url(&page, drive_id)?).await?;
        let (changes, next, new_start) = parse_changes(&v);
        all.extend(changes);
        match (next, new_start) {
            (Some(t), _) => page = t,
            (None, Some(start)) => return Ok((all, start, false)),
            (None, None) => return Ok((all, page, false)),
        }
    }
    eprintln!("drive: changes hit the page guard at {MAX_PAGES} pages");
    Ok((all, page, true))
}

/// Pull every **My Drive** change since `cursor` + the next baseline cursor (`newStartPageToken`). On
/// an expired cursor Google returns HTTP 410 — surfaced as an error the caller detects
/// ([`is_cursor_expired`]) to fall back to a full re-list.
pub async fn list_changes(
    token_key: &str,
    cursor: &str,
) -> Result<(Vec<DriveChange>, String, bool)> {
    list_changes_for(token_key, None, cursor).await
}

/// Pull every change since `cursor` for one **shared drive's** own change feed + its next cursor.
pub async fn list_shared_changes(
    token_key: &str,
    drive_id: &str,
    cursor: &str,
) -> Result<(Vec<DriveChange>, String, bool)> {
    list_changes_for(token_key, Some(drive_id), cursor).await
}

/// Fetch one file's metadata (for body-on-demand, where we hold only the stored pointer).
/// `supportsAllDrives` so a shared-drive file resolves too (harmless for My Drive files).
pub async fn fetch_file(token_key: &str, file_id: &str) -> Result<DriveFile> {
    let url = format!("{DRIVE_API}/files/{file_id}?fields={FILE_FIELDS}&supportsAllDrives=true");
    let v = google::authorized_get(token_key, &url).await?;
    parse_file(&v).ok_or_else(|| Error::Other("Drive returned no file for that id.".into()))
}

/// Export a Google Sheet's FULL grid as an `.xlsx` workbook to a temp file, for the "import fully"
/// promote flow — the ONE place the whole grid is fetched (the index-only sync only ever pulls
/// metadata). The temp file keeps an `.xlsx` extension so the local spreadsheet processor picks the
/// right (openpyxl) parser; the caller removes it after ingest. Capped at [`MAX_FILE_BYTES`], but
/// Google's `export` endpoint itself refuses sheets over ~10 MB, so a very large sheet surfaces a
/// clear error here rather than importing a partial grid.
pub async fn export_sheet_xlsx(token_key: &str, file: &DriveFile) -> Result<PathBuf> {
    let mut url = reqwest::Url::parse(&format!("{DRIVE_API}/files/{}/export", file.id))
        .map_err(|e| Error::Other(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("mimeType", SHEET_EXPORT_XLSX_MIME)
        .append_pair("supportsAllDrives", "true");
    let bytes = google::authorized_get_bytes(token_key, url.as_str(), MAX_FILE_BYTES).await?;
    if bytes.is_empty() {
        return Err(Error::Other(
            "Google returned an empty export for this spreadsheet.".into(),
        ));
    }
    // Force an `.xlsx` name (the Sheet's own name has no extension) so `stage_temp` tags the temp file
    // correctly for the extension-routed sidecar parser.
    stage_temp("pm-drive-", "export.xlsx", &bytes)
}

/// Resolve a folder id to its display name — the label a synced file is tagged with. A folder is just
/// a file in Drive, so this projects only `name`. Best-effort: `None` if the folder can't be reached
/// (deleted, out of scope, or the id was never resolvable), since folder context is a soft review hint
/// and must never fail a sync. `supportsAllDrives` so a shared-drive folder resolves too. Callers
/// cache by id per sync run so each unique folder is fetched at most once.
pub async fn fetch_folder_name(token_key: &str, folder_id: &str) -> Option<String> {
    let url = format!("{DRIVE_API}/files/{folder_id}?fields=name&supportsAllDrives=true");
    google::authorized_get(token_key, &url)
        .await
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(String::from))
}

/// True if a Drive API error is the "page token expired" 410 — the signal to discard the cursor and
/// re-baseline with a full `files.list`.
pub fn is_cursor_expired(err: &Error) -> bool {
    err.to_string().contains("(410")
}

/// True if a Drive error is transient rate-limiting rather than an auth problem. Google surfaces a
/// throttle as an HTTP 429, or as a 403 whose error `reason` is a usage-limit (`rateLimitExceeded`,
/// `userRateLimitExceeded`, domain `usageLimits`). The grant is fine — the account is just being
/// throttled — so a sync must treat it as retryable (leave the account's state, don't advance the
/// cursor) instead of fanning every item out to `unreachable` (F-26). The reason strings ride in the
/// truncated error body; a 403 with no usage-limit reason (e.g. `insufficientPermissions`) is a real
/// auth failure and is NOT matched here.
pub fn is_rate_limited(err: &Error) -> bool {
    let s = err.to_string();
    (s.contains("(429") || s.contains("(403"))
        && (s.contains("rateLimitExceeded")
            || s.contains("userRateLimitExceeded")
            || s.contains("usageLimits"))
}

/// True if a Drive API error is an auth failure (revoked/expired) for the whole account — the signal
/// to fan the account out to `unreachable` rather than treat it as mass deletion. A rate-limit 403
/// ([`is_rate_limited`]) is explicitly excluded: it's transient, so it must not masquerade as a
/// revoked grant and flip a healthy account to `unreachable` over a momentary quota blip (F-26).
pub fn is_auth_failure(err: &Error) -> bool {
    if is_rate_limited(err) {
        return false;
    }
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
            url.query_pairs_mut()
                .append_pair("mimeType", mime)
                .append_pair("supportsAllDrives", "true");
            let bytes =
                google::authorized_get_bytes(token_key, url.as_str(), MAX_FILE_BYTES).await?;
            Ok(non_empty(&String::from_utf8_lossy(&bytes)))
        }
        FetchPlan::DownloadText => {
            let url = format!(
                "{DRIVE_API}/files/{}?alt=media&supportsAllDrives=true",
                file.id
            );
            let bytes = google::authorized_get_bytes(token_key, &url, MAX_FILE_BYTES).await?;
            Ok(non_empty(&String::from_utf8_lossy(&bytes)))
        }
        FetchPlan::DownloadBinary => {
            let url = format!(
                "{DRIVE_API}/files/{}?alt=media&supportsAllDrives=true",
                file.id
            );
            let downloaded = google::authorized_get_bytes(token_key, &url, MAX_FILE_BYTES).await;
            convert_downloaded_binary(state, "pm-drive-", &file.name, downloaded)
        }
        FetchPlan::SheetMetadata => fetch_sheet_metadata(token_key, file).await,
    }
}

/// One tab of a Google Sheet, from the Sheets API `sheets.properties` (grid dimensions are the
/// allocated grid, not the populated-row count — labelled as such in the body).
struct SheetTab {
    title: String,
    rows: i64,
    cols: i64,
}

/// Build the metadata-only indexable body for a Google Sheet — tab names, each tab's header row and
/// grid dimensions — via the Sheets API, NEVER the full grid (that is the whole point of index-only
/// Sheets). Two tiny calls: one `spreadsheets.get` (properties only, `includeGridData=false`) and one
/// `values:batchGet` for row 1 of every tab. Gated on the account holding `spreadsheets.readonly`: an
/// account that hasn't re-consented for Sheets (or any Sheets-API read error) degrades to a
/// Drive-metadata-only body so the Sheet stays index-only-findable and its `webViewLink` still works —
/// no 403 is ever issued into the sync error path. `None` only when there is genuinely nothing to index.
async fn fetch_sheet_metadata(token_key: &str, file: &DriveFile) -> Result<Option<String>> {
    if !google::token_has_scope(token_key, google::SHEETS_SCOPE)? {
        return Ok(sheet_reconnect_body(file));
    }
    // Build via query_pairs_mut so the `fields` mask's parens/commas are properly percent-encoded.
    let mut meta_url = reqwest::Url::parse(&format!("{SHEETS_API}/{}", file.id))
        .map_err(|e| Error::Other(e.to_string()))?;
    meta_url
        .query_pairs_mut()
        .append_pair("includeGridData", "false")
        .append_pair(
            "fields",
            "properties.title,sheets(properties(title,gridProperties(rowCount,columnCount)))",
        );
    let meta = match google::authorized_get(token_key, meta_url.as_str()).await {
        Ok(v) => v,
        // A Sheets-API read failed after the scope check passed. Never fail the whole sync item over it,
        // but don't embed a misleading "reconnect" instruction for a transient blip: only a genuine auth
        // failure keeps the reconnect prompt; a 429/5xx/network error degrades to a neutral name-only
        // body so the Sheet stays findable + linkable and its real metadata fills in on the next clean
        // sync (see `sheet_error_fallback_body`).
        Err(e) => return Ok(sheet_error_fallback_body(file, &e)),
    };
    let tabs = parse_sheet_tabs(&meta);
    let title = meta["properties"]["title"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&file.name);
    // Header rows are best-effort: an empty sheet or a failed batchGet just omits column names.
    let headers = fetch_sheet_headers(token_key, &file.id, &tabs)
        .await
        .unwrap_or_default();
    Ok(Some(build_sheet_body(file, title, &tabs, &headers)))
}

/// Parse the per-tab properties from a `spreadsheets.get` response into [`SheetTab`]s.
fn parse_sheet_tabs(meta: &Value) -> Vec<SheetTab> {
    meta["sheets"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let p = &s["properties"];
                    let title = p["title"].as_str()?.to_string();
                    let gp = &p["gridProperties"];
                    Some(SheetTab {
                        title,
                        rows: gp["rowCount"].as_i64().unwrap_or(0),
                        cols: gp["columnCount"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Row 1 (the header) of every tab, in one `values:batchGet`. Each range is `'Tab'!1:1` with the tab
/// name single-quoted (A1 notation) and any internal quote doubled. Returns one header-cell vec per
/// tab, in the tabs' order; a tab with no row 1 yields an empty vec.
async fn fetch_sheet_headers(
    token_key: &str,
    spreadsheet_id: &str,
    tabs: &[SheetTab],
) -> Result<Vec<Vec<String>>> {
    if tabs.is_empty() {
        return Ok(Vec::new());
    }
    let mut url = reqwest::Url::parse(&format!("{SHEETS_API}/{spreadsheet_id}/values:batchGet"))
        .map_err(|e| Error::Other(e.to_string()))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("majorDimension", "ROWS");
        for tab in tabs {
            q.append_pair("ranges", &header_range(&tab.title));
        }
    }
    let v = google::authorized_get(token_key, url.as_str()).await?;
    let ranges = v["valueRanges"].as_array().cloned().unwrap_or_default();
    Ok(tabs
        .iter()
        .enumerate()
        .map(|(i, _)| {
            ranges
                .get(i)
                .and_then(|r| r["values"].as_array())
                .and_then(|rows| rows.first())
                .and_then(|row| row.as_array())
                .map(|cells| {
                    cells
                        .iter()
                        .map(|c| c.as_str().unwrap_or("").trim().to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect())
}

/// A1-notation range for a tab's header row, tab name single-quoted with internal quotes doubled.
fn header_range(tab_title: &str) -> String {
    format!("'{}'!1:1", tab_title.replace('\'', "''"))
}

/// Assemble the metadata-only Sheet body from the parsed tabs + header rows. Kept compact (ideally
/// ≤ the 500-char offline summary) so it round-trips a Rebuild verbatim via `stored_summary`.
fn build_sheet_body(
    file: &DriveFile,
    spreadsheet_title: &str,
    tabs: &[SheetTab],
    headers: &[Vec<String>],
) -> String {
    let mut s = format!("Google Sheet \"{spreadsheet_title}\"");
    if let Some(m) = &file.modified_time {
        s.push_str(&format!(" (last modified {})", &m[..m.len().min(10)]));
    }
    s.push_str(&format!(
        " — {} tab{}.\n",
        tabs.len(),
        if tabs.len() == 1 { "" } else { "s" }
    ));
    for (i, tab) in tabs.iter().enumerate() {
        s.push_str(&format!(
            "Tab \"{}\" (grid {}x{}",
            tab.title, tab.rows, tab.cols
        ));
        let cols: Vec<&String> = headers
            .get(i)
            .map(|h| h.iter().filter(|c| !c.is_empty()).collect())
            .unwrap_or_default();
        if cols.is_empty() {
            s.push_str(").\n");
        } else {
            let names: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
            s.push_str(&format!("); columns: {}.\n", names.join(", ")));
        }
    }
    s.trim_end().to_string()
}

/// The fallback body for a Sheet on an account that hasn't granted `spreadsheets.readonly`: index it by
/// name so it stays findable + its `webViewLink` opens, and tell the user how to enrich it. The tab
/// names + headers fill in on the next sync after they reconnect.
fn sheet_reconnect_body(file: &DriveFile) -> Option<String> {
    Some(format!(
        "Google Sheet \"{}\". Reconnect this Google account in Settings \u{2192} Connectors to index \
         its tab names and column headers.",
        file.name
    ))
}

/// The fallback body for a Sheet whose metadata couldn't be read this sync because of a transient
/// Sheets-API error (429/5xx/network) rather than a missing grant. Names the Sheet so it stays findable
/// and its `webViewLink` still opens, WITHOUT a "reconnect" instruction — the account is fine; the tab
/// names + column headers fill in on the next sync that reads it cleanly.
fn sheet_offline_body(file: &DriveFile) -> Option<String> {
    Some(format!("Google Sheet \"{}\".", file.name))
}

/// Choose the fallback body when the Sheets metadata read errored *after* the scope check passed. A
/// genuine auth failure ([`is_auth_failure`] — a revoked/insufficient grant) keeps the actionable
/// reconnect prompt; any other error is transient (rate-limit, 5xx, network) and must NOT embed a
/// misleading reconnect instruction that would then stick until the Sheet next changes — so it degrades
/// to a neutral name-only body ([`sheet_offline_body`]). Pure so the transient-vs-auth split is tested
/// without a live API (F-28).
fn sheet_error_fallback_body(file: &DriveFile, err: &Error) -> Option<String> {
    if is_auth_failure(err) {
        sheet_reconnect_body(file)
    } else {
        sheet_offline_body(file)
    }
}

// `non_empty` / `stage_temp` / the DownloadBinary convert tail now live in [`crate::cloud_sync`]
// (shared with OneDrive — the copies differed only in the temp-file prefix, passed as `pm-drive-`).

// The sync progress event, report, and not-indexed issue types now live unified (shared with OneDrive)
// as `CloudSyncEvent` / `CloudSyncReport` / `CloudSyncIssue` in [`crate::cloud_sync`] — the two
// providers' copies were byte-identical (audit X-D1).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_only::SourceState;

    fn file(id: &str, mime: &str, md5: Option<&str>, modified: &str) -> DriveFile {
        DriveFile {
            id: id.into(),
            name: format!("{id}.bin"),
            mime_type: mime.into(),
            modified_time: Some(modified.into()),
            md5: md5.map(String::from),
            trashed: false,
            web_view_link: Some(format!("https://drive/{id}")),
            parent_id: None,
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
    fn is_sheet_matches_only_the_sheet_mime() {
        assert!(is_sheet("application/vnd.google-apps.spreadsheet"));
        assert!(!is_sheet("application/vnd.google-apps.document"));
        assert!(!is_sheet("text/csv"));
    }

    #[test]
    fn rate_limit_403_is_retryable_not_an_auth_failure() {
        // The exact error shape `json_or_err` builds for a Drive quota blip.
        let err = Error::Other(
            "Google API request failed (403): {\"error\":{\"errors\":[{\"domain\":\"usageLimits\",\
             \"reason\":\"rateLimitExceeded\",\"message\":\"Rate Limit Exceeded\"}],\"code\":403}}"
                .into(),
        );
        assert!(is_rate_limited(&err));
        // The point of F-26: a throttle must NOT flip the whole account to `unreachable`.
        assert!(!is_auth_failure(&err));
    }

    #[test]
    fn user_rate_limit_403_is_retryable() {
        let err = Error::Other(
            "Google API request failed (403): {\"error\":{\"errors\":[{\"reason\":\
             \"userRateLimitExceeded\"}]}}"
                .into(),
        );
        assert!(is_rate_limited(&err));
        assert!(!is_auth_failure(&err));
    }

    #[test]
    fn throttle_429_is_retryable_not_an_auth_failure() {
        let err = Error::Other("Google API request failed (429): rateLimitExceeded".into());
        assert!(is_rate_limited(&err));
        assert!(!is_auth_failure(&err));
    }

    #[test]
    fn permission_403_is_a_real_auth_failure() {
        // A 403 with NO usage-limit reason (revoked/insufficient scope) still fans out to `unreachable`.
        let err = Error::Other(
            "Google API request failed (403): {\"error\":{\"errors\":[{\"reason\":\
             \"insufficientPermissions\"}]}}"
                .into(),
        );
        assert!(!is_rate_limited(&err));
        assert!(is_auth_failure(&err));
    }

    #[test]
    fn revoked_401_is_a_real_auth_failure() {
        let err = Error::Other("Google API request failed (401): invalid_grant".into());
        assert!(!is_rate_limited(&err));
        assert!(is_auth_failure(&err));
    }

    #[test]
    fn read_item_state_claims_a_promoted_row() {
        // A promoted Sheet keeps its `gdrive:` source_id under source_type='spreadsheet'. read_item_state
        // must still see it (it matches on source_id alone) so the sync treats the still-present source as
        // already-imported and no-ops instead of re-ingesting an index-only duplicate.
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), key).unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id, \
                    source_state, source_content_hash) \
             VALUES ('v.md','Budget','h1','spreadsheet','gdrive:a@b.com:F1','ok','hash1')",
            [],
        )
        .unwrap();
        let st = read_item_state(&conn, "gdrive:a@b.com:F1")
            .unwrap()
            .expect("a promoted row is still a known item");
        assert_eq!(st.source_content_hash.as_deref(), Some("hash1"));
        assert_eq!(st.source_state, SourceState::Ok);
        // An unknown id is still None.
        assert!(read_item_state(&conn, "gdrive:a@b.com:NOPE")
            .unwrap()
            .is_none());
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
        let my_cursor = |c: &Connection| {
            read_cursors(c, "a@b.com")
                .unwrap()
                .get(MY_DRIVE_CURSOR_KEY)
                .cloned()
        };

        // A clean pass (account_failed = false) commits: cursor advanced, time stamped, state 'ok'.
        finalize_or_flag(&conn, "a@b.com", false, Some("CUR1"), &[]).unwrap();
        let (state_ok, synced_ok) = row(&conn);
        assert_eq!(state_ok, "ok");
        assert!(synced_ok.is_some(), "a clean pass stamps last_synced_at");
        assert_eq!(my_cursor(&conn).as_deref(), Some("CUR1"));

        // A failed pass (account_failed = true) takes the error path: even though a fresh "CUR2" cursor
        // is offered, state flips to 'error' and the cursor + last-good time are left exactly as the
        // clean pass set them.
        finalize_or_flag(&conn, "a@b.com", true, Some("CUR2"), &[]).unwrap();
        let (state_err, synced_err) = row(&conn);
        assert_eq!(state_err, "error");
        assert_eq!(
            synced_err, synced_ok,
            "the failure path must not restamp last_synced_at"
        );
        assert_eq!(
            my_cursor(&conn).as_deref(),
            Some("CUR1"),
            "the failure path must not advance the cursor",
        );
    }

    #[test]
    fn shared_source_ids_are_account_independent_and_namespace_per_drive() {
        // Shared-drive ids carry NO account (dedup, v19) — the same id whoever reaches the drive.
        let sid = shared_source_id("0ADrive", "FILE123");
        assert_eq!(sid, "gdrive:sd:0ADrive:FILE123");
        // Under its own per-drive prefix (so reconcile can isolate one shared drive)…
        assert!(sid.starts_with(&shared_prefix("0ADrive")));
        assert!(!sid.starts_with(&shared_prefix("0BOther")));
        // …with no owning account in the id itself (resolved via shared_drive_access instead)…
        assert_eq!(account_of(&sid), None);
        // …and the drive id is recoverable for access lookups.
        assert_eq!(shared_drive_of(&sid).as_deref(), Some("0ADrive"));
        assert_eq!(shared_drive_of("gdrive:a@b.com:FILE123"), None);
        // A My-Drive id still resolves to its account and isn't mistaken for a shared one.
        let my = source_id_for("a@b.com", "FILE123");
        assert_eq!(account_of(&my).as_deref(), Some("a@b.com"));
        assert!(!my.starts_with(&shared_prefix("0ADrive")));
    }

    #[test]
    fn drive_scope_defaults_to_my_drive_only_and_tolerates_partial_json() {
        // The default (no row value) indexes My Drive and no shared drives.
        let def = DriveScope::default();
        assert!(def.my_drive);
        assert!(def.shared.is_empty());
        // A stored blob with only `shared` still defaults `my_drive` to true (serde `default = yes`).
        let parsed: DriveScope =
            serde_json::from_str(r#"{"shared":[{"drive_id":"0A","name":"Team"}]}"#).unwrap();
        assert!(parsed.my_drive);
        assert_eq!(parsed.shared.len(), 1);
        assert_eq!(parsed.shared[0].drive_id, "0A");
        assert_eq!(parsed.shared[0].folders, None); // missing → whole drive
                                                    // Folder-scoped round-trips.
        let scoped: DriveScope = serde_json::from_str(
            r#"{"my_drive":false,"shared":[{"drive_id":"0A","name":"Team","folders":["f1","f2"]}]}"#,
        )
        .unwrap();
        assert!(!scoped.my_drive);
        assert_eq!(
            scoped.shared[0].folders.as_deref(),
            Some(&["f1".to_string(), "f2".to_string()][..])
        );
    }

    #[test]
    fn cursor_column_decodes_map_legacy_and_empty() {
        // The current shape: a JSON object of per-corpus tokens.
        let m = decode_cursors(Some(r#"{"my":"T1","0ADrive":"T2"}"#.to_string()));
        assert_eq!(m.get("my").map(String::as_str), Some("T1"));
        assert_eq!(m.get("0ADrive").map(String::as_str), Some("T2"));
        // A legacy bare token (pre-shared-drives) reads as the My-Drive cursor and upgrades on write.
        let legacy = decode_cursors(Some("LEGACYTOKEN".to_string()));
        assert_eq!(legacy.len(), 1);
        assert_eq!(
            legacy.get(MY_DRIVE_CURSOR_KEY).map(String::as_str),
            Some("LEGACYTOKEN")
        );
        // Absent / blank → empty (a first sync).
        assert!(decode_cursors(None).is_empty());
        assert!(decode_cursors(Some("   ".to_string())).is_empty());
    }

    #[test]
    fn parse_shared_drives_and_folders_read_the_shapes() {
        let drives = serde_json::json!({
            "nextPageToken": "P2",
            "drives": [{"id": "0A", "name": "Team A"}, {"id": "0B"}]
        });
        let (parsed, next) = parse_shared_drives(&drives);
        assert_eq!(next.as_deref(), Some("P2"));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "Team A");
        assert_eq!(parsed[1].name, "Shared drive"); // name defaulted

        let folders = serde_json::json!({
            "files": [{"id": "f1", "name": "Reports"}, {"id": "f2", "name": "Archive"}]
        });
        let (parsed, next) = parse_folders(&folders);
        assert!(next.is_none());
        assert_eq!(
            parsed,
            vec![
                DriveFolder {
                    id: "f1".into(),
                    name: "Reports".into()
                },
                DriveFolder {
                    id: "f2".into(),
                    name: "Archive".into()
                },
            ]
        );
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
        // A Sheet is metadata-only now (was Export text/csv) — never the full grid.
        assert_eq!(
            fetch_plan("application/vnd.google-apps.spreadsheet"),
            FetchPlan::SheetMetadata
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

    fn sheet_file() -> DriveFile {
        DriveFile {
            id: "sid".into(),
            name: "Q2 Budget".into(),
            mime_type: SHEET_MIME.into(),
            modified_time: Some("2026-07-01T09:30:00.000Z".into()),
            md5: None,
            trashed: false,
            web_view_link: Some("https://docs.google.com/spreadsheets/d/sid".into()),
            parent_id: None,
        }
    }

    #[test]
    fn parse_sheet_tabs_reads_properties() {
        let meta = serde_json::json!({
            "properties": {"title": "Q2 Budget"},
            "sheets": [
                {"properties": {"title": "Summary", "gridProperties": {"rowCount": 128, "columnCount": 4}}},
                {"properties": {"title": "Detail",  "gridProperties": {"rowCount": 1450, "columnCount": 6}}}
            ]
        });
        let tabs = parse_sheet_tabs(&meta);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].title, "Summary");
        assert_eq!((tabs[0].rows, tabs[0].cols), (128, 4));
        assert_eq!(tabs[1].title, "Detail");
    }

    #[test]
    fn header_range_quotes_and_escapes_tab_names() {
        assert_eq!(header_range("Summary"), "'Summary'!1:1");
        // A tab name with a single quote gets it doubled (A1 escaping) so the range stays valid.
        assert_eq!(header_range("Bob's tab"), "'Bob''s tab'!1:1");
    }

    #[test]
    fn build_sheet_body_is_metadata_only_and_dated() {
        let file = sheet_file();
        let tabs = vec![
            SheetTab {
                title: "Summary".into(),
                rows: 128,
                cols: 3,
            },
            SheetTab {
                title: "Detail".into(),
                rows: 1450,
                cols: 2,
            },
        ];
        let headers = vec![
            vec!["Project".to_string(), "Amount".to_string(), "".to_string()], // trailing empty dropped
            vec![], // no header row on this tab
        ];
        let body = build_sheet_body(&file, "Q2 Budget", &tabs, &headers);
        assert!(body.starts_with("Google Sheet \"Q2 Budget\" (last modified 2026-07-01) — 2 tabs."));
        assert!(body.contains("Tab \"Summary\" (grid 128x3); columns: Project, Amount."));
        // A tab with no header row shows just its grid dimensions, no "columns:".
        assert!(
            body.contains("Tab \"Detail\" (grid 1450x2).\n")
                || body.contains("Tab \"Detail\" (grid 1450x2).")
        );
        // Never any cell values beyond the header — this is metadata-only.
        assert!(!body.contains("1200"));
    }

    #[test]
    fn sheet_reconnect_body_names_the_sheet() {
        let body = sheet_reconnect_body(&sheet_file()).unwrap();
        assert!(body.contains("Google Sheet \"Q2 Budget\""));
        assert!(body.contains("Reconnect"));
    }

    #[test]
    fn transient_sheets_error_degrades_to_a_neutral_name_only_body() {
        // A 429 rate-limit is transient: the fallback must name the Sheet (so it stays findable) WITHOUT
        // the misleading "reconnect" instruction that would otherwise stick until the Sheet next changed.
        let err = Error::Other("Google API request failed (429): rateLimitExceeded".into());
        let body = sheet_error_fallback_body(&sheet_file(), &err).unwrap();
        assert!(body.contains("Google Sheet \"Q2 Budget\""));
        assert!(!body.contains("Reconnect"));
    }

    #[test]
    fn genuine_auth_failure_keeps_the_reconnect_prompt() {
        // A 403 with no usage-limit reason is a real grant problem (not a throttle) — here the reconnect
        // prompt IS the right, actionable message, unlike a transient blip.
        let err = Error::Other(
            "Google API request failed (403): {\"error\":{\"errors\":[{\"reason\":\
             \"insufficientPermissions\"}]}}"
                .into(),
        );
        let body = sheet_error_fallback_body(&sheet_file(), &err).unwrap();
        assert!(body.contains("Reconnect"));
    }

    #[test]
    fn parse_files_and_changes_read_the_shapes() {
        let files = serde_json::json!({
            "nextPageToken": "PAGE2",
            "files": [
                {"id": "f1", "name": "A.pdf", "mimeType": "application/pdf", "md5Checksum": "h1", "modifiedTime": "2026-06-26T00:00:00Z", "webViewLink": "https://drive/f1", "parents": ["FOLDER1"]},
                {"id": "f2", "name": "Notes", "mimeType": "application/vnd.google-apps.document", "modifiedTime": "2026-06-26T01:00:00Z"}
            ]
        });
        let (parsed, next) = parse_files(&files);
        assert_eq!(next.as_deref(), Some("PAGE2"));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].md5.as_deref(), Some("h1"));
        assert!(parsed[1].md5.is_none());
        // The folder a file sits in is read from Drive's first `parents` entry; absent → None.
        assert_eq!(parsed[0].parent_id.as_deref(), Some("FOLDER1"));
        assert!(parsed[1].parent_id.is_none());

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
