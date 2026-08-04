// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The index-only cloud connectors — Google Drive, OneDrive and local folders — plus the
//! shared fetch/promote path that turns an indexed stub into a full body.

use rusqlite::params;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, State};

use crate::blocking::spawn_blocking_result;
use crate::error::{Error, Result};
use crate::google;
use crate::ingest::{self, Document};
use crate::{
    cloud_sync, db, drive, index_only, localfolder, microsoft, onedrive, pathguard, secrets,
    AppState,
};

use super::archivist::refuse_if_rebuilding;
use super::shared::own_client;
use super::vaults::require_vault_owner;

// --- Google Drive (index-only connector, board card 4A) ---

/// The Drive connector's state for Settings: whether the shared Google client is configured, plus
/// every connected account (each independent — its own token, sync, and items).
#[derive(Serialize)]
pub struct DriveStatus {
    pub oauth_client_configured: bool,
    pub accounts: Vec<drive::DriveAccount>,
}

#[tauri::command]
pub fn drive_status(state: State<'_, AppState>) -> Result<DriveStatus> {
    let conn = state.conn()?;
    Ok(DriveStatus {
        oauth_client_configured: google::has_client()?,
        accounts: drive::list_accounts(&conn)?,
    })
}

/// Connect a Google Drive account (read-only): run the consent flow, learn which account it granted
/// (Drive `about`), store that account's token under its own keychain key, and register it. Returns
/// the connected account. Normally uses the shared BYO Google client; if `client_id`/`client_secret`
/// are supplied, this account signs in with its OWN Cloud project (the Advanced-Protection path) and
/// that client is remembered for the account so later token refreshes reuse it.
#[tauri::command]
pub async fn connect_drive(
    app: AppHandle,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<drive::DriveAccount> {
    require_vault_owner(&app)?;
    let own = own_client(client_id, client_secret)?;
    // Request read-only Drive AND read-only Sheets together (space-joined per OAuth), so the account
    // grants both in one consent. Sheets powers the metadata-only Google Sheets index; an account that
    // last consented before Sheets existed keeps working for Drive and re-grants Sheets on reconnect
    // (`include_granted_scopes=true` unions it). Reconnecting an existing account runs this same flow.
    let scopes = format!("{} {}", google::DRIVE_SCOPE, google::SHEETS_SCOPE);
    let token = match &own {
        Some((id, secret)) => {
            google::run_consent_with_client(&scopes, "Google Drive", id.clone(), secret.clone())
                .await?
        }
        None => google::run_consent(&scopes, "Google Drive").await?,
    };
    let (email, name) = drive::about_user(&token).await?;
    if let Some((id, secret)) = &own {
        secrets::set_google_client_for_account(&email, id, secret)?;
    }
    google::save_token(&drive::account_token_key(&email), &token)?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    drive::upsert_account(&conn, &email, &name)?;
    drive::list_accounts(&conn)?
        .into_iter()
        .find(|a| a.email == email)
        .ok_or_else(|| Error::Other("the account registration could not be read back".into()))
}

/// Disconnect one Drive account: forget its token and registry row, and soft-flag its indexed items
/// `unreachable` (kept findable — never a hard delete).
#[tauri::command]
pub async fn disconnect_drive(state: State<'_, AppState>, email: String) -> Result<()> {
    // The backup destination reuses this account's token key, so revoking here would sever a grant
    // the user has not asked to give up — and a re-granted `drive.file` is a NEW grant, which cannot
    // write the archives the old one uploaded. That silently breaks backup retention with a 403 the
    // next time it runs. Only revoke when nothing else is using the account.
    let used_for_backup = {
        let conn = state.conn()?;
        crate::db::get_setting(&conn, crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY)?
            .is_some_and(|a| a == email)
    };
    // L-3: sever the grant at Google's end BEFORE forgetting the local token — best-effort, exactly
    // like "Remove PM data" (wipe.rs). Revoking the refresh token drops PM from the account's
    // Connected-apps list; without it the grant lingers at Google until the token expires naturally.
    if !used_for_backup {
        if let Ok(Some(blob)) = secrets::get_google_token_for(&drive::account_token_key(&email)) {
            let _ = google::revoke(blob.expose()).await;
        }
    }
    {
        let conn = state.conn()?;
        drive::forget_account(
            &conn,
            &email,
            if used_for_backup {
                drive::Credentials::Keep
            } else {
                drive::Credentials::Forget
            },
        )?;
    }
    state.sync_index_only();
    Ok(())
}

/// The shared drives one connected account can see (`drives.list`) — for the "add shared drives"
/// picker. Read-only enumeration over the account's own token; no DB and no sidecar needed.
#[tauri::command]
pub async fn list_drive_shared_drives(email: String) -> Result<Vec<drive::SharedDrive>> {
    drive::list_shared_drives(&drive::account_token_key(&email)).await
}

/// Shared drives already indexed by a DIFFERENT connected account → `driveId → owner email`. The
/// scope picker greys those out ("synced by <owner>") since shared drives are de-duplicated — only the
/// owner indexes a drive, so the user needn't (and can't usefully) re-index it under this account.
#[tauri::command]
pub fn drive_shared_owners(
    state: State<'_, AppState>,
    email: String,
) -> Result<std::collections::HashMap<String, String>> {
    let conn = state.conn()?;
    drive::shared_drive_owners_elsewhere(&conn, &email)
}

/// The immediate subfolders of `parent_id` inside a shared drive — one lazy level of the folder
/// picker. Pass the shared drive's id as `parent_id` for the top level.
#[tauri::command]
pub async fn list_drive_folders(
    email: String,
    drive_id: String,
    parent_id: String,
) -> Result<Vec<drive::DriveFolder>> {
    drive::list_folders(&drive::account_token_key(&email), &drive_id, &parent_id).await
}

/// The account's "Shared with me" ROOTS — the top-level files/folders others granted it directly, for
/// the shared-with-me picker. Both files and folders are selectable (unlike My/shared drives, which
/// expose only folders). Read-only enumeration over the account's own token; no DB, no sidecar.
#[tauri::command]
pub async fn list_drive_shared_with_me_roots(email: String) -> Result<Vec<drive::SwmRoot>> {
    drive::list_swm_root_choices(&drive::account_token_key(&email)).await
}

/// Shared-with-me roots already indexed by a DIFFERENT connected account → `rootId → owner email`. The
/// picker greys those out ("synced by <owner>"), since a shared-with-me root is de-duplicated like a
/// shared drive — only its owner indexes it, so this account needn't (and can't usefully) re-index it.
#[tauri::command]
pub fn drive_swm_root_owners(
    state: State<'_, AppState>,
    email: String,
) -> Result<std::collections::HashMap<String, String>> {
    let conn = state.conn()?;
    drive::swm_root_owners_elsewhere(&conn, &email)
}

/// One account's indexing scope (My Drive on/off + opted-in shared drives and their folders).
#[tauri::command]
pub fn get_drive_scope(state: State<'_, AppState>, email: String) -> Result<drive::DriveScope> {
    let conn = state.conn()?;
    drive::get_scope(&conn, &email)
}

/// Persist one account's indexing scope. The UI follows this with a `sync_drive` to apply it (index
/// newly-in-scope files, soft-remove files that fell out of scope).
#[tauri::command]
pub fn set_drive_scope(
    state: State<'_, AppState>,
    email: String,
    scope: drive::DriveScope,
) -> Result<()> {
    let conn = state.conn()?;
    drive::set_scope(&conn, &email, &scope)
}

/// Clone a sync-state snapshot out of its mutex (`what` names the sync in the poisoned-lock error).
/// Shared by the three `*_sync_status` commands.
fn sync_snapshot<T: Clone>(state: &std::sync::Mutex<T>, what: &str) -> Result<T> {
    state
        .lock()
        .map(|s| s.clone())
        .map_err(|_| Error::Other(format!("{what} sync state poisoned")))
}

/// Whether the snapshot should report "Stopping…" — a Stop is a request against the pass that is
/// RUNNING, so an idle connector never reports one however the flag happens to be sitting (it is
/// cleared at the next sync start, not on finish). Derived here rather than stored on the state, so
/// the flag stays the single owner of the fact and the button can't disagree with it (#699).
fn stop_requested(running: bool, cancel: &AtomicBool) -> bool {
    running && cancel.load(Ordering::SeqCst)
}

/// Project a run's owed sweeps onto the wire pair the UI reads (`queued`, `queued_all`).
///
/// The rules — including the `running` gate — live on [`crate::connector_sync::SyncQueue::owed`], so
/// they stay beside the merge rules they belong to and are tested there. What this seam adds is the
/// one thing a caller can still get wrong: the projection is taken from the clone [`sync_snapshot`]
/// already holds, **never by re-locking the slot**. A second acquisition straddles
/// `pass_complete`'s atomic take-and-retarget, so it can return the old account beside an
/// already-drained queue — the row goes blank in exactly the window this exists to cover.
fn queued_sweeps(running: bool, queue: &crate::connector_sync::SyncQueue) -> (Vec<String>, bool) {
    queue.owed(running)
}

/// Shared engine behind the three `resume_*_sync` commands: read the connector's pending-sync
/// marker, bail when there's nothing to resume or a sync is already running this session (don't
/// stack), then hand the marker's parsed target (account/folder; `None` = all) to `spawn`.
/// Returns whether a resume was kicked off.
fn resume_pending_sync(
    app: AppHandle,
    pending_key: &str,
    is_running: impl FnOnce(&AppState) -> bool,
    spawn: impl FnOnce(AppHandle, Option<String>),
) -> Result<bool> {
    let marker: Option<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, pending_key)?
    };
    let Some(marker) = marker else {
        return Ok(false);
    };
    if is_running(&app.state::<AppState>()) {
        return Ok(false);
    }
    let target: Option<String> = serde_json::from_str(&marker).unwrap_or(None);
    spawn(app, target);
    Ok(true)
}

/// The currently-running Drive sync snapshot (empty / `running:false` when idle), so the Settings UI
/// can resume showing progress after the user leaves and returns.
#[tauri::command]
pub fn drive_sync_status(state: State<'_, AppState>) -> Result<crate::CloudSyncState> {
    let mut snap = sync_snapshot(&state.drive_sync, "drive")?;
    snap.stopping = stop_requested(snap.running, &state.drive_sync_cancel);
    (snap.queued, snap.queued_all) = queued_sweeps(snap.running, &snap.queue);
    Ok(snap)
}

/// The message a full re-index is refused with while a sync is in flight. Named once because both
/// providers raise it and the UI's disabled state must agree with it word for word.
const REINDEX_BUSY: &str =
    "A sync is already running. Wait for it to finish, then re-index — starting one now would \
     re-establish the cursor this is meant to discard.";

/// Re-index one Drive account from scratch: forget its delta cursors, then run an ordinary sync,
/// which now has no cursor to ride and so re-enumerates everything in scope (#727).
///
/// **Refused while a sync is running, and that guard is load-bearing rather than defensive.** The
/// running pass ends in `finalize_sync`, which writes a FRESH cursor. Clearing the map underneath it
/// would therefore be undone moments later by a pass that never re-enumerated — leaving the user with
/// a full walk they asked for, waited through, and did not get. The UI also disables the control, but
/// a frontend boolean is not a store guarantee (the webview is untrusted), so the refusal lives here.
///
/// Deliberately per-ACCOUNT and never "all". A full walk is the most expensive thing this connector
/// does, and the point of the split control is to make the expensive path chosen rather than reached
/// by default.
#[tauri::command]
pub async fn reindex_drive(app: AppHandle, account: String) -> Result<usize> {
    {
        let state = app.state::<AppState>();
        if sync_snapshot(&state.drive_sync, "drive")?.running {
            return Err(Error::Other(REINDEX_BUSY.into()));
        }
        let conn = state.conn()?;
        drive::clear_cursors(&conn, &account)?;
    }
    sync_drive(app, Some(account), Some(true)).await
}

/// Re-index one OneDrive account from scratch — the sibling of [`reindex_drive`]; same guard, same
/// reason.
#[tauri::command]
pub async fn reindex_onedrive(app: AppHandle, account: String) -> Result<usize> {
    {
        let state = app.state::<AppState>();
        if sync_snapshot(&state.onedrive_sync, "onedrive")?.running {
            return Err(Error::Other(REINDEX_BUSY.into()));
        }
        let conn = state.conn()?;
        onedrive::clear_cursor(&conn, &account)?;
    }
    sync_onedrive(app, Some(account)).await
}

/// Sync one Drive account (or every account when `account` is `None`) into the index-only store. See
/// [`cloud_sync::drive_sync_core`] for the behaviour; this is the command the UI's "Sync now" calls.
///
/// `includeSharedWithMe` defaults to TRUE when omitted, so every existing caller — and any future
/// one that forgets the argument — keeps syncing the full corpus. Only the background poller's
/// frequent passes opt out, because that corpus has no delta cursor and must be re-walked in full.
#[tauri::command]
pub async fn sync_drive(
    app: AppHandle,
    account: Option<String>,
    include_shared_with_me: Option<bool>,
) -> Result<usize> {
    refuse_if_rebuilding(&app, "a sync would be indexing into a moving target")?;
    cloud_sync::drive_sync_core(&app, account, include_shared_with_me.unwrap_or(true)).await
}

/// Ask the running sync to stop after the current file. Already-indexed files are kept; the rest are
/// left for the next sync. A no-op when nothing is running (the flag resets at the next sync start).
#[tauri::command]
pub fn stop_drive_sync(state: State<'_, AppState>) -> Result<()> {
    state.drive_sync_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Resume a sync a previous app session started but didn't finish (the app was closed/crashed
/// mid-index). Called once on launch. Returns whether a resume was kicked off. Already-indexed files
/// were persisted as they went, so the resumed pass re-checks the source and only does the work that
/// was left — it never re-embeds what's already there. No marker → nothing to resume.
#[tauri::command]
pub fn resume_drive_sync(app: AppHandle) -> Result<bool> {
    resume_pending_sync(
        app,
        cloud_sync::DRIVE_SYNC_PENDING_KEY,
        |st| st.drive_sync.lock().map(|s| s.running).unwrap_or(false),
        |app, account| {
            tauri::async_runtime::spawn(async move {
                // A resume finishes an interrupted pass, which may have been mid-shared-with-me.
                let _ = cloud_sync::drive_sync_core(&app, account, true).await;
            });
        },
    )
}

/// Register a local folder to index (the path comes from the frontend's native folder picker). Returns
/// the folder's stable key; the UI then triggers a sync. Idempotent — re-adding reactivates the row.
#[tauri::command]
pub fn add_local_folder(state: State<'_, AppState>, path: String) -> Result<String> {
    // L-5: the path is a webview string (from the native picker, but a compromised webview could
    // supply any path). Require a real, absolute, well-formed location before we register a root
    // whose whole subtree we then walk and read.
    pathguard::sanitize_source(&path)?;
    let root = std::path::PathBuf::from(&path);
    if !root.is_dir() {
        return Err(Error::Other("That path isn't a folder we can read.".into()));
    }
    let conn = state.conn()?;
    localfolder::add_folder(&conn, &root)
}

/// Stop tracking a local folder: its items stay findable (flagged `unreachable`), the registry row drops.
#[tauri::command]
pub fn remove_local_folder(state: State<'_, AppState>, key: String) -> Result<()> {
    let conn = state.conn()?;
    localfolder::remove_folder(&conn, &key)
}

/// Every tracked local folder (path, state, indexed count, present?, excludes), for the Settings list.
#[tauri::command]
pub fn list_local_folders(state: State<'_, AppState>) -> Result<Vec<localfolder::LocalFolder>> {
    let conn = state.conn()?;
    localfolder::list_folders(&conn)
}

/// The immediate child subfolders of `rel` (root-relative, `/`-joined; `None`/empty = the folder root)
/// inside a tracked folder — one lazy level of the local folder picker.
#[tauri::command]
pub fn list_local_subfolders(
    state: State<'_, AppState>,
    key: String,
    rel: Option<String>,
) -> Result<Vec<localfolder::LocalSubfolder>> {
    // The block is load-bearing, not style: it drops the DB guard before `list_subfolders` walks
    // the filesystem, so a slow or unreachable disk never holds the connection lock.
    let root = {
        let conn = state.conn()?;
        localfolder::folder_root(&conn, &key)?
    };
    let Some(root) = root else {
        return Err(Error::Other("That folder isn't tracked.".into()));
    };
    localfolder::list_subfolders(&root, rel.as_deref().unwrap_or(""))
}

/// Persist a tracked folder's excluded subfolders (root-relative paths). The UI follows this with a
/// `sync_local` to apply it (soft-remove now-excluded files, re-index any un-excluded ones).
#[tauri::command]
pub fn set_local_excludes(
    state: State<'_, AppState>,
    key: String,
    exclude: Vec<String>,
) -> Result<()> {
    let conn = state.conn()?;
    localfolder::set_excludes(&conn, &key, &exclude)
}

/// The currently-running local-folder sync snapshot, so the UI resumes progress after navigating away.
#[tauri::command]
pub fn local_folder_sync_status(state: State<'_, AppState>) -> Result<crate::LocalFolderSyncState> {
    let mut snap = sync_snapshot(&state.local_sync, "local")?;
    snap.stopping = stop_requested(snap.running, &state.local_sync_cancel);
    (snap.queued, snap.queued_all) = queued_sweeps(snap.running, &snap.queue);
    Ok(snap)
}

/// Ask the running local-folder sync to stop after the current file (already-indexed files are kept).
#[tauri::command]
pub fn stop_local_folder_sync(state: State<'_, AppState>) -> Result<()> {
    state.local_sync_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Sync one tracked folder (or every folder when `folder` is `None`) — the "Sync now" command.
#[tauri::command]
pub async fn sync_local_folder(app: AppHandle, folder: Option<String>) -> Result<usize> {
    refuse_if_rebuilding(&app, "a sync would be indexing into a moving target")?;
    localfolder::local_sync_core(&app, folder).await
}

/// Resume a local-folder sync a previous session started but didn't finish (closed/crashed mid-index).
/// Called once on launch; returns whether a resume was kicked off. Already-indexed files were persisted
/// as they went, so a resumed pass only does the work that was left.
#[tauri::command]
pub fn resume_local_folder_sync(app: AppHandle) -> Result<bool> {
    resume_pending_sync(
        app,
        localfolder::LOCAL_SYNC_PENDING_KEY,
        |st| st.local_sync.lock().map(|s| s.running).unwrap_or(false),
        |app, folder| {
            tauri::async_runtime::spawn(async move {
                let _ = localfolder::local_sync_core(&app, folder).await;
            });
        },
    )
}

/// Fetch one index-only item's current body live from its source, converted to the same indexable
/// text the ingest path produces and **trimmed identically** (`input.body.trim()`, index_only.rs), so
/// its bytes match the string the stored chunk offsets were computed against. Shared by the reader
/// (`fetch_index_only_body`) and the on-demand re-index (`reindex_index_only`). Never persists the body.
async fn fetch_index_only_text(app: &AppHandle, doc_id: i64) -> Result<String> {
    // The LIVE location, not the identity anchor (#710). A document reachable at two places opens
    // from whichever one is still there — which is the point of the model: the anchor is an identity,
    // and a file deleted from the account that happened to index it first must not take the copy in
    // a tracked folder down with it. `fetchable` prefers the anchor and falls through.
    let (source_type, location) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let source_type: String = conn.query_row(
            "SELECT source_type FROM documents WHERE id = ?1",
            params![doc_id],
            |r| r.get(0),
        )?;
        (source_type, crate::locations::fetchable(&conn, doc_id)?)
    };
    if source_type != ingest::SOURCE_TYPE_INDEX_ONLY {
        return Err(Error::Other(
            "This document is stored locally — open it directly.".into(),
        ));
    }
    let missing = || {
        Error::Other(
            "This file was removed at the source; only its saved summary is available.".into(),
        )
    };
    let location = location.ok_or_else(missing)?;
    if location.state == index_only::SourceState::SourceMissing {
        return Err(missing());
    }
    let source_id = location.source_id;
    let external_ref = location.external_ref;
    let state = app.state::<AppState>();
    // `ensure_installed` is blocking (first run installs the venv + deps) — run it on the blocking
    // pool so it never pins a tokio worker (F-41). The cloned handle reaches AppState in the closure.
    {
        let app = app.clone();
        spawn_blocking_result("sidecar install", move || {
            app.state::<AppState>().sidecar.ensure_installed()
        })
        .await?;
    }
    let no_text = || Error::Other("This file has no extractable text to show.".into());
    // Fetch the body live and convert it exactly like a fresh index. Dispatch on the source-id
    // provider prefix; the trailing segment after the last `:` is the provider's file id (Drive
    // fileIds and Graph itemIds carry no `:`). Every branch yields a String, trimmed uniformly below.
    let raw = if source_id.starts_with("local:") {
        // Local folder: the body is on disk at the stored path (its `external_ref`).
        let path = external_ref
            .ok_or_else(|| Error::Other("This indexed file has no stored path.".into()))?;
        let path = std::path::PathBuf::from(&path);
        if !path.is_file() {
            return Err(Error::Other(
                "This file is no longer at its saved location.".into(),
            ));
        }
        let app2 = app.clone();
        let (markdown, _title) = spawn_blocking_result("local convert", move || {
            app2.state::<AppState>().sidecar.convert(&path)
        })
        .await?;
        markdown
    } else {
        let item_id = source_id
            .rsplit_once(':')
            .map(|(_, id)| id.to_string())
            .ok_or_else(|| Error::Other("Malformed source id.".into()))?;
        // Drive: a My Drive id names its account directly; a shared-drive id is account-independent,
        // so resolve an account that can reach it (owner first). Read off the lock before the fetch.
        let drive_token_key = {
            let conn = state.conn()?;
            drive::token_key_for_source(&conn, &source_id)?
        };
        if let Some(token_key) = drive_token_key {
            let file = drive::fetch_file(&token_key, &item_id).await?;
            drive::fetch_body(state.inner(), &token_key, &file)
                .await?
                .ok_or_else(no_text)?
        } else if let Some(email) = onedrive::account_of(&source_id) {
            let token_key = onedrive::account_token_key(&email);
            let item = onedrive::fetch_item(&token_key, &item_id).await?;
            onedrive::fetch_body(state.inner(), &token_key, &item)
                .await?
                .ok_or_else(no_text)?
        } else {
            return Err(Error::Other("Unrecognised index-only source.".into()));
        }
    };
    // Trim on EVERY branch, not just local: the chunk offsets index `input.body.trim()`, so the
    // cloud branches used to return an un-trimmed body that shifted the whole overlay.
    let body = raw.trim().to_string();
    if body.is_empty() {
        return Err(no_text());
    }
    Ok(body)
}

/// The reader's live fetch of an index-only body plus whether the stored chunk offsets still index it
/// EXACTLY (a `content_hash` identity match, not a length heuristic) — so the overlay is drawn only
/// when its byte offsets would land in the right places, and offers Re-index otherwise.
#[derive(Serialize)]
pub struct IndexOnlyFetch {
    pub body: String,
    pub aligned: bool,
}

/// Fetch an index-only document's full body live from its source, for the reader. The body is never
/// stored — only the short summary lives offline. Also reports whether the stored chunk offsets still
/// index this exact body, so the chunk overlay can decide between drawing and offering a Re-index.
#[tauri::command]
pub async fn fetch_index_only_body(app: AppHandle, doc_id: i64) -> Result<IndexOnlyFetch> {
    let body = fetch_index_only_text(&app, doc_id).await?;
    let state = app.state::<AppState>();
    let (source_id, stored_hash): (Option<String>, Option<String>) = {
        let conn = state.conn()?;
        conn.query_row(
            "SELECT source_id, content_hash FROM documents WHERE id = ?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
    };
    // `documents.content_hash` for an index-only item IS pointer_content_hash(source_id, indexed
    // trimmed body). Recompute it over the freshly fetched (trimmed) body: equal ⇒ the offsets index
    // this exact string, so the overlay is safe to draw; unequal ⇒ the map is stale (offer Re-index).
    let aligned = match (source_id, stored_hash) {
        (Some(sid), Some(stored)) => index_only::pointer_content_hash(&sid, &body) == stored,
        _ => false,
    };
    Ok(IndexOnlyFetch { body, aligned })
}

/// Re-fetch one index-only item's live body and rebuild its stored chunk map + summary against it,
/// reusing [`index_only::reindex_pointer`] (which preserves the item's classification —
/// project/tags/importance/reviewed/entity — replacing only chunks/summary/title), then push the change
/// to the encrypted manifest so a reconcile-on-open can't revert it. Returns the exact body it embedded.
/// The shared core of the reader's on-demand "Re-index this item" and the Rebuild-time bulk upgrade.
pub(super) async fn reindex_index_only_core(app: &AppHandle, doc_id: i64) -> Result<String> {
    let body = fetch_index_only_text(app, doc_id).await?;
    let app2 = app.clone();
    let embedded = body.clone();
    spawn_blocking_result("reindex", move || -> Result<()> {
        let state = app2.state::<AppState>();
        state.sidecar.ensure_installed()?;
        let (source_id, external_ref, title, source_modified_at, source_content_hash): (
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        ) = {
            let conn = state.conn()?;
            conn.query_row(
                "SELECT source_id, external_ref, title, source_modified_at, source_content_hash \
                 FROM documents WHERE id = ?1 AND source_type = 'index_only'",
                params![doc_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                    ))
                },
            )?
        };
        if source_id.is_empty() {
            return Err(Error::Other(
                "This item has no source pointer to re-index.".into(),
            ));
        }
        let input = index_only::PointerInput {
            source_id,
            title,
            external_ref,
            source_modified_at,
            source_content_hash,
            body: embedded,
            // Not used by the re-embed (it rewrites only the chunk map + summary + title); the DB's
            // existing folder and source-metadata columns are left untouched.
            source_parent_folder_id: None,
            source_parent_folder_name: None,
            source_author: None,
            source_last_modified_by: None,
            source_created_at: None,
            source_size_bytes: None,
        };
        let gateway = {
            let conn = state.conn()?;
            state.gateway_for_write(&conn)?
        };
        index_only::reindex_pointer(&state, &gateway, &input)?;
        // The re-embed rewrote the DB row (chunk map + source_state='ok' + summary); push those to the
        // encrypted manifest (the source of truth) so a reconcile-on-open can't revert them — every
        // other index-only write path syncs the manifest, and this must too.
        let (vault_root, manifest_cipher) = state.manifest_io()?;
        let conn = state.conn()?;
        // The re-embed already committed, so a failed push leaves the file behind the mirror — record
        // it, or the next boot applies the older file back over what we just wrote.
        if let Err(e) = index_only::write_synced(&conn, &vault_root, &manifest_cipher) {
            index_only::mark_manifest_stale(&conn);
            return Err(e);
        }
        Ok(())
    })
    .await?;
    Ok(body)
}

/// Re-index one index-only item on demand (the reader's "Re-index this item"): re-fetch its current
/// live body and rebuild the stored chunk map + summary against it, so a stale overlay (e.g. offsets
/// left indexing the ~500-char summary after a rebuild-from-manifest) lines up again. Returns the exact
/// body it embedded (so the reader redraws the overlay against it with no second live fetch).
#[tauri::command]
pub async fn reindex_index_only(app: AppHandle, doc_id: i64) -> Result<IndexOnlyFetch> {
    let body = reindex_index_only_core(&app, doc_id).await?;
    // The overlay now indexes the exact body we just embedded — confirm against the freshly written
    // content_hash and hand the body back so the reader needn't fetch a second time.
    let aligned = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let (source_id, stored): (Option<String>, String) = conn.query_row(
            "SELECT source_id, content_hash FROM documents WHERE id = ?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        source_id.is_some_and(|sid| index_only::pointer_content_hash(&sid, &body) == stored)
    };
    Ok(IndexOnlyFetch { body, aligned })
}

/// Promote an index-only Google Sheet to a **full local spreadsheet import** — the "import fully"
/// action. Fetches the Sheet's FULL grid (exported as an `.xlsx` workbook, every tab preserved), routes
/// it through the local spreadsheet processor, and transforms the document IN PLACE (same id, keeps its
/// classification): `source_type` flips `index_only` → `spreadsheet`, the synthetic sheet body becomes
/// vault-stored Markdown, and the source is stripped from the index-only manifest so it can't be
/// resurrected (see [`ingest::promote_spreadsheet`]). Only Google Sheets are promotable today — other
/// index-only sources (Docs, PDFs) have no grid to import this way. Returns the updated document.
#[tauri::command]
pub async fn promote_index_only(app: AppHandle, doc_id: i64) -> Result<Document> {
    let (source_type, source_id, source_state): (String, Option<String>, String) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        conn.query_row(
            "SELECT source_type, source_id, source_state FROM documents WHERE id = ?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    };
    if source_type != ingest::SOURCE_TYPE_INDEX_ONLY {
        return Err(Error::Other(
            "This document is already imported locally.".into(),
        ));
    }
    if source_state == "source_missing" {
        return Err(Error::Other(
            "This file was removed at the source, so it can't be imported.".into(),
        ));
    }
    let source_id = source_id
        .ok_or_else(|| Error::Other("This indexed item has no source pointer to import.".into()))?;

    let state = app.state::<AppState>();
    // `ensure_installed` is blocking (first run installs the venv + deps) — run it on the blocking
    // pool so it never pins a tokio worker (F-41). The cloned handle reaches AppState in the closure.
    {
        let app = app.clone();
        spawn_blocking_result("sidecar install", move || {
            app.state::<AppState>().sidecar.ensure_installed()
        })
        .await?;
    }
    // The provider file id is the segment after the last `:` (Drive/Graph ids carry none), mirroring
    // `fetch_index_only_body`.
    let item_id = source_id
        .rsplit_once(':')
        .map(|(_, id)| id.to_string())
        .ok_or_else(|| Error::Other("Malformed source id.".into()))?;

    // Only Google Drive Sheets are promotable today. Resolve an account that can reach the file (My
    // Drive names its account; a shared-drive id resolves an owner) off the lock before the fetch.
    let token_key = {
        let conn = state.conn()?;
        drive::token_key_for_source(&conn, &source_id)?
    }
    .ok_or_else(|| {
        Error::Other("Importing fully is only supported for Google Drive sources right now.".into())
    })?;

    let file = drive::fetch_file(&token_key, &item_id).await?;
    if !drive::is_sheet(&file.mime_type) {
        return Err(Error::Other(
            "Only Google Sheets can be imported fully right now.".into(),
        ));
    }
    // Pull the FULL grid as an `.xlsx` workbook to a temp file — the ONE place the whole grid is
    // fetched. Then hand off to the blocking ingest transform, cleaning the temp file up after.
    let path = drive::export_sheet_xlsx(&token_key, &file).await?;
    let app2 = app.clone();
    spawn_blocking_result("import", move || {
        let state = app2.state::<AppState>();
        let build = || -> Result<Document> {
            let (vault, cipher) = state.markdown_io()?;
            let (vault_root, manifest_cipher) = state.manifest_io()?;
            let gateway = {
                let conn = state.conn()?;
                state.gateway_for_write(&conn)?
            };
            ingest::promote_spreadsheet(
                state.inner(),
                &gateway,
                &vault,
                &cipher,
                &vault_root,
                &manifest_cipher,
                doc_id,
                &path,
                Some("xlsx"),
            )
        };
        let out = build();
        let _ = std::fs::remove_file(&path);
        out
    })
    .await
}

// --- Microsoft OneDrive (index-only connector, board card 4B) ---
//
// A near-mirror of the Google Drive block above, for OneDrive via Microsoft Graph. The differences
// are mechanical: a public client (no secret), the Graph delta query (one endpoint does first-sync
// AND incremental), and a single personal-drive corpus (no shared drives) that is either whole-drive
// (delta cursor) or folder-scoped (re-enumerate + reconcile). It reuses the index-only foundation,
// the gentle-mode pacing, and `connector_sync::apply_connector_actions` / `action_category` unchanged.

/// The OneDrive connector's state for Settings: whether the BYO Microsoft client id is configured,
/// plus every connected account (each independent — its own token, sync, and items).
#[derive(Serialize)]
pub struct OneDriveStatus {
    pub oauth_client_configured: bool,
    pub accounts: Vec<onedrive::OneDriveAccount>,
}

#[tauri::command]
pub fn onedrive_status(state: State<'_, AppState>) -> Result<OneDriveStatus> {
    let conn = state.conn()?;
    Ok(OneDriveStatus {
        oauth_client_configured: microsoft::has_client()?,
        accounts: onedrive::list_accounts(&conn)?,
    })
}

/// Save the user's BYO Microsoft client id (public client — no secret). Keychain-only; provider-level
/// (shared by every OneDrive account). Setting it connects nothing on its own.
#[tauri::command]
pub fn set_microsoft_client(app: AppHandle, client_id: String) -> Result<()> {
    require_vault_owner(&app)?;
    // The last of the blank-string-secret class (`set_openrouter_key`, `set_google_client` and the
    // secrets getters all already guard it). A stored "" passes `.is_some()`, so `has_client()`
    // reported CONFIGURED and every OAuth attempt then failed opaquely somewhere deep in the flow,
    // instead of saying "no client set" at the one place that knows.
    let id = client_id.trim();
    if id.is_empty() {
        return Err(Error::Other("Client ID is empty".into()));
    }
    secrets::set_microsoft_client(id)
}

/// Clear the Microsoft client id and sign out every OneDrive account (they all depend on it). Indexed
/// items are kept but flagged unreachable (never deleted), matching the Google-client clear.
#[tauri::command]
pub fn clear_microsoft_client(state: State<'_, AppState>) -> Result<()> {
    {
        let conn = state.conn()?;
        onedrive::forget_all_accounts(&conn)?;
    }
    secrets::clear_microsoft_client()?;
    state.sync_index_only();
    Ok(())
}

/// Connect a Microsoft OneDrive account (read-only): run the consent flow, learn which account it
/// granted (Graph `/me`), store that account's token under its own keychain key, and register it.
/// Returns the connected account. The BYO Microsoft client id must already be configured.
#[tauri::command]
pub async fn connect_onedrive(app: AppHandle) -> Result<onedrive::OneDriveAccount> {
    require_vault_owner(&app)?;
    let token = microsoft::run_consent(microsoft::ONEDRIVE_SCOPE, "OneDrive").await?;
    let (email, name) = onedrive::me_account(&token).await?;
    microsoft::save_token(&onedrive::account_token_key(&email), &token)?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    onedrive::upsert_account(&conn, &email, &name)?;
    onedrive::list_accounts(&conn)?
        .into_iter()
        .find(|a| a.email == email)
        .ok_or_else(|| Error::Other("the account registration could not be read back".into()))
}

/// Disconnect one OneDrive account: forget its token and registry row, and soft-flag its indexed
/// items `unreachable` (kept findable — never a hard delete).
#[tauri::command]
pub fn disconnect_onedrive(state: State<'_, AppState>, email: String) -> Result<()> {
    {
        let conn = state.conn()?;
        onedrive::forget_account(&conn, &email)?;
    }
    state.sync_index_only();
    Ok(())
}

/// The immediate subfolders of `parent_id` (or the drive root when `parent_id` is `None`) — one lazy
/// level of the OneDrive folder picker.
#[tauri::command]
pub async fn list_onedrive_folders(
    email: String,
    parent_id: Option<String>,
) -> Result<Vec<onedrive::OneDriveFolder>> {
    onedrive::list_folders(&onedrive::account_token_key(&email), parent_id.as_deref()).await
}

/// One account's indexing scope (whole drive, or the chosen folders).
#[tauri::command]
pub fn get_onedrive_scope(
    state: State<'_, AppState>,
    email: String,
) -> Result<onedrive::OneDriveScope> {
    let conn = state.conn()?;
    onedrive::get_scope(&conn, &email)
}

/// Persist one account's indexing scope. The UI follows this with a `sync_onedrive` to apply it.
#[tauri::command]
pub fn set_onedrive_scope(
    state: State<'_, AppState>,
    email: String,
    scope: onedrive::OneDriveScope,
) -> Result<()> {
    let conn = state.conn()?;
    onedrive::set_scope(&conn, &email, &scope)
}

/// The currently-running OneDrive sync snapshot, so the Settings UI can resume showing progress.
#[tauri::command]
pub fn onedrive_sync_status(state: State<'_, AppState>) -> Result<crate::CloudSyncState> {
    let mut snap = sync_snapshot(&state.onedrive_sync, "onedrive")?;
    snap.stopping = stop_requested(snap.running, &state.onedrive_sync_cancel);
    (snap.queued, snap.queued_all) = queued_sweeps(snap.running, &snap.queue);
    Ok(snap)
}

/// Sync one OneDrive account (or every account when `account` is `None`). The command the UI's
/// "Sync now" calls; see [`cloud_sync::onedrive_sync_core`] for the behaviour.
#[tauri::command]
pub async fn sync_onedrive(app: AppHandle, account: Option<String>) -> Result<usize> {
    refuse_if_rebuilding(&app, "a sync would be indexing into a moving target")?;
    cloud_sync::onedrive_sync_core(&app, account).await
}

/// Ask the running OneDrive sync to stop after the current file (kept-so-far stays indexed).
#[tauri::command]
pub fn stop_onedrive_sync(state: State<'_, AppState>) -> Result<()> {
    state.onedrive_sync_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Resume a OneDrive sync a previous app session started but didn't finish. Called once on launch.
#[tauri::command]
pub fn resume_onedrive_sync(app: AppHandle) -> Result<bool> {
    resume_pending_sync(
        app,
        cloud_sync::ONEDRIVE_SYNC_PENDING_KEY,
        |st| st.onedrive_sync.lock().map(|s| s.running).unwrap_or(false),
        |app, account| {
            tauri::async_runtime::spawn(async move {
                let _ = cloud_sync::onedrive_sync_core(&app, account).await;
            });
        },
    )
}
