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
    format!("{}{}", secrets::GOOGLE_TOKEN_DRIVE_PREFIX, email)
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

/// Every shared drive this account can see (`drives.list`) — for the "add shared drives" picker.
pub async fn list_shared_drives(token_key: &str) -> Result<Vec<SharedDrive>> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let v = google::authorized_get(token_key, &shared_drives_url(page.as_deref())?).await?;
        let (drives, next) = parse_shared_drives(&v);
        out.extend(drives);
        match next {
            Some(t) => page = Some(t),
            None => return Ok(out),
        }
    }
    Ok(out)
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
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let url = if my_drive {
            my_files_url(&q, page.as_deref())?
        } else {
            shared_files_url(drive_id, &q, "id,name", "name", page.as_deref())?
        };
        let v = google::authorized_get(token_key, &url).await?;
        let (folders, next) = parse_folders(&v);
        out.extend(folders);
        if next.is_none() {
            break;
        }
        page = next;
    }
    // The personal corpus isn't server-sorted by name (no `orderBy`); sort for a stable picker.
    if my_drive {
        out.sort_by_key(|a| a.name.to_lowercase());
    }
    Ok(out)
}

/// Enumerate the files of a shared drive to index: the **whole** drive (`folders == None`), or only
/// the selected folders walked recursively (`Some`). Folders themselves are never indexed — only the
/// files beneath them. Deduplicates files reachable from more than one selected folder.
pub async fn enumerate_shared(
    token_key: &str,
    drive_id: &str,
    folders: Option<&[String]>,
) -> Result<Vec<DriveFile>> {
    match folders {
        None => {
            let mut out = Vec::new();
            let mut page: Option<String> = None;
            for _ in 0..MAX_PAGES {
                let url = shared_files_url(
                    drive_id,
                    "trashed = false",
                    FILE_FIELDS,
                    "modifiedTime desc",
                    page.as_deref(),
                )?;
                let v = google::authorized_get(token_key, &url).await?;
                let (files, next) = parse_files(&v);
                out.extend(files);
                match next {
                    Some(t) => page = Some(t),
                    None => return Ok(out),
                }
            }
            eprintln!("drive: shared whole-drive enumerate hit the page guard");
            Ok(out)
        }
        Some(roots) => {
            walk_folders(token_key, roots, |q, page| {
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
async fn walk_folders(
    token_key: &str,
    roots: &[String],
    url_for: impl Fn(&str, Option<&str>) -> Result<String>,
) -> Result<Vec<DriveFile>> {
    use std::collections::HashSet;
    let mut out: Vec<DriveFile> = Vec::new();
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
            eprintln!("drive: folder walk hit the node guard at {MAX_PAGES} folders");
            break;
        }
        let q = format!("'{folder}' in parents and trashed = false");
        let mut page: Option<String> = None;
        loop {
            let url = url_for(&q, page.as_deref())?;
            let v = google::authorized_get(token_key, &url).await?;
            let (children, next) = parse_files(&v);
            for child in children {
                if child.mime_type == FOLDER_MIME {
                    queue.push(child.id);
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
    Ok(out)
}

/// Enumerate the files under the selected **My Drive** folders (recursively, deduped) — the personal
/// counterpart to `enumerate_shared`'s folder branch, for folder-scoped My Drive.
pub async fn enumerate_my_folders(token_key: &str, folders: &[String]) -> Result<Vec<DriveFile>> {
    walk_folders(token_key, folders, my_files_url).await
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

async fn list_changes_for(
    token_key: &str,
    drive_id: Option<&str>,
    cursor: &str,
) -> Result<(Vec<DriveChange>, String)> {
    let mut all = Vec::new();
    let mut page = cursor.to_string();
    for _ in 0..MAX_PAGES {
        let v = google::authorized_get(token_key, &changes_url(&page, drive_id)?).await?;
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

/// Pull every **My Drive** change since `cursor` + the next baseline cursor (`newStartPageToken`). On
/// an expired cursor Google returns HTTP 410 — surfaced as an error the caller detects
/// ([`is_cursor_expired`]) to fall back to a full re-list.
pub async fn list_changes(token_key: &str, cursor: &str) -> Result<(Vec<DriveChange>, String)> {
    list_changes_for(token_key, None, cursor).await
}

/// Pull every change since `cursor` for one **shared drive's** own change feed + its next cursor.
pub async fn list_shared_changes(
    token_key: &str,
    drive_id: &str,
    cursor: &str,
) -> Result<(Vec<DriveChange>, String)> {
    list_changes_for(token_key, Some(drive_id), cursor).await
}

/// Fetch one file's metadata (for body-on-demand, where we hold only the stored pointer).
/// `supportsAllDrives` so a shared-drive file resolves too (harmless for My Drive files).
pub async fn fetch_file(token_key: &str, file_id: &str) -> Result<DriveFile> {
    let url = format!("{DRIVE_API}/files/{file_id}?fields={FILE_FIELDS}&supportsAllDrives=true");
    let v = google::authorized_get(token_key, &url).await?;
    parse_file(&v).ok_or_else(|| Error::Other("Drive returned no file for that id.".into()))
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
/// A file PM tried to index but couldn't, surfaced in the post-sync report so the user knows what was
/// left out (e.g. an unsupported file type MarkItDown can't read, or a fetch error). Not a fatal
/// error — the sync carries on; these are just reported.
#[derive(Clone, Serialize, Default)]
pub struct DriveSyncIssue {
    pub name: String,
    pub reason: String,
}

/// The outcome of a sync pass: how many items were indexed/updated/removed, the list of files that
/// couldn't be indexed (capped), and whether the user stopped it early. Shown in Settings after a
/// sync and stashed in the live snapshot so a user returning after it finished still sees the result.
#[derive(Clone, Serialize, Default)]
pub struct DriveSyncReport {
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    /// The user pressed Stop — already-indexed files are kept; the rest were left for next time.
    pub cancelled: bool,
    /// Files attempted but not indexed (unsupported/empty, or a fetch error), capped for memory.
    pub issues: Vec<DriveSyncIssue>,
    /// True when more files couldn't be indexed than the capped `issues` list holds.
    pub issues_truncated: bool,
}

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
    /// The run is done; `report` carries the breakdown + the not-indexed list (+ a `cancelled` flag).
    Finished { report: DriveSyncReport },
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
