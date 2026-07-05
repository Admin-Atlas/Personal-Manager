// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Google Drive and OneDrive sync engines — each a detached, single-flight, crash-resumable pass
//! that gathers a connected account's work off the DB lock (phase 1: My Drive / whole-drive delta
//! cursor or a folder-scoped reconcile), then processes it item by item (phase 2: `index_only::react`
//! -> fetch a body only when needed -> apply off the lock). Lifted verbatim out of [`crate::commands`]
//! so the IPC layer keeps only the thin `#[tauri::command]` wrappers that call in here; the behaviour
//! is unchanged. The two engines still mirror each other closely — unifying them behind one driver is
//! the next step. The shared single-flight + crash-resume-marker lifecycle and the blocking index-only
//! apply live in [`crate::connector_sync`].

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager};

use crate::error::{Error, Result};
use crate::{connector_sync, db, drive, index_only, onedrive, AppState};

/// Resolve a Drive folder id to its display name for sorting-review context, memoising by id across a
/// whole sync pass (folder ids are globally unique in Drive) so tagging many files in one folder costs
/// a single lookup. Best-effort: an unreachable folder yields `None` and the file still indexes,
/// untagged. This is the only folder-name lookup path — `list_drive_folders` is a lazy picker tree,
/// not an id→name cache, so there is nothing to reuse.
async fn resolve_folder_name(
    token_key: &str,
    folder_id: &str,
    cache: &mut std::collections::HashMap<String, Option<String>>,
) -> Option<String> {
    if let Some(hit) = cache.get(folder_id) {
        return hit.clone();
    }
    let name = drive::fetch_folder_name(token_key, folder_id).await;
    cache.insert(folder_id.to_string(), name.clone());
    name
}

/// One unit of sync work for an account, gathered off the lock in phase 1.
enum DriveItem {
    /// A My-Drive first-sync file → `Add`.
    Enumerated(drive::DriveFile),
    /// A My-Drive changes-feed entry → mapped via `map_change`.
    Changed(drive::DriveChange),
    /// A shared-drive reconcile result with its event pre-built: `Add` for a new/reactivating file
    /// (reducer: unknown→ingest, missing/unreachable→reachable), `Update` for a present healthy file
    /// (same-hash→noop, changed→re-embed), or `Delete` for a file that vanished from the enumeration.
    Reconciled {
        source_id: String,
        event: index_only::ChangeEvent,
        file: Option<drive::DriveFile>,
    },
}

/// Map a shared reconcile-plan entry ([`index_only::reconcile_enumeration`]) onto this connector's
/// work item — the enumerated file rides along as `file` so phase 2 can fetch a body on `Add`/`Update`.
fn drive_reconciled(r: index_only::ReconcileItem<drive::DriveFile>) -> DriveItem {
    DriveItem::Reconciled {
        source_id: r.source_id,
        event: r.event,
        file: r.payload,
    }
}

/// All of one account's gathered work for a sync pass (My Drive + shared drives together).
struct AccountWork {
    email: String,
    token_key: String,
    items: Vec<DriveItem>,
    /// The advanced My-Drive delta cursor — set only when My Drive was synced this pass.
    new_cursor: Option<String>,
    /// Advanced delta cursors for whole-drive shared selections — `(driveId, newCursor)`, one per
    /// whole-drive shared drive that synced cleanly this pass. Folder-scoped selections add nothing.
    shared_new_cursors: Vec<(String, String)>,
    auth_failed: bool,
}

/// Gather one opted-in shared drive's work, dispatching on how much of it is in scope:
/// - **Folder-scoped** (`folders = Some`) → [`gather_shared_folders`] (re-enumerate + reconcile; the
///   changes feed can't be scoped to folders). No cursor.
/// - **Whole drive** (`folders = None`) → [`gather_shared_whole`] (the same efficient delta-cursor
///   path My Drive uses). Returns the advanced `(driveId, cursor)` to persist on a clean pass.
async fn gather_shared(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    sel: &drive::SharedSelection,
) -> Result<(Vec<DriveItem>, Option<(String, String)>)> {
    match sel.folders.as_deref() {
        Some(folders) => Ok((
            gather_shared_folders(app, token_key, &sel.drive_id, folders).await?,
            None,
        )),
        None => gather_shared_whole(app, token_key, email, &sel.drive_id).await,
    }
}

/// Folder-scoped reconcile: enumerate the selected folders live and diff against the currently-healthy
/// known set. A present file already healthy → `Update` (catches edits); a present file new or
/// previously missing/unreachable → `Add` (ingests, or reactivates a folder the user removed and
/// re-added); a known file no longer present → `Delete`. Reads the known set under a brief lock; the
/// enumeration itself is off the lock.
async fn gather_shared_folders(
    app: &AppHandle,
    token_key: &str,
    drive_id: &str,
    folders: &[String],
) -> Result<Vec<DriveItem>> {
    let files = drive::enumerate_shared(token_key, drive_id, Some(folders)).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_shared_source_ids(&conn, drive_id)?
            .into_iter()
            .collect()
    };
    Ok(index_only::reconcile_enumeration(files, known, |file_id| {
        drive::shared_source_id(drive_id, file_id)
    })
    .into_iter()
    .map(drive_reconciled)
    .collect())
}

/// Folder-scoped reconcile for **My Drive**: the personal counterpart to [`gather_shared_folders`].
/// Enumerates the selected My-Drive folders live and diffs against the account's currently-healthy
/// My-Drive items (shared-drive items excluded — they reconcile on their own). Same event semantics:
/// present+known → `Update`, present+new/missing → `Add`, known-but-absent → `Delete`. No cursor.
async fn gather_my_drive_folders(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    folders: &[String],
) -> Result<Vec<DriveItem>> {
    let files = drive::enumerate_my_folders(token_key, folders).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_my_drive_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    Ok(index_only::reconcile_enumeration(files, known, |file_id| {
        drive::source_id_for(email, file_id)
    })
    .into_iter()
    .map(drive_reconciled)
    .collect())
}

/// Whole-drive sync via a **per-drive delta cursor** — the same cheap path My Drive uses, so a large
/// shared drive isn't fully re-listed every sync. First pass (no stored cursor, or a 410 reset)
/// enumerates the drive as `Add`s and baselines a fresh start token; later passes pull only the
/// drive's own changes feed and advance the cursor. Returns the items plus `(driveId, newCursor)` to
/// persist after the pass commits.
async fn gather_shared_whole(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    drive_id: &str,
) -> Result<(Vec<DriveItem>, Option<(String, String)>)> {
    let cursor = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::get_shared_cursor(&conn, email, drive_id)?
    };

    // First sync / 410 reset: the whole drive enumerated as Adds + a fresh baseline cursor.
    async fn baseline(token_key: &str, drive_id: &str) -> Result<(Vec<DriveItem>, String)> {
        let files = drive::enumerate_shared(token_key, drive_id, None).await?;
        let new_cursor = drive::start_page_token(token_key, Some(drive_id)).await?;
        let items = files
            .into_iter()
            .map(|f| {
                let source_id = drive::shared_source_id(drive_id, &f.id);
                let event = index_only::ChangeEvent::Add {
                    source_id: source_id.clone(),
                    modified_at: f.modified_time.clone(),
                };
                DriveItem::Reconciled {
                    source_id,
                    event,
                    file: Some(f),
                }
            })
            .collect();
        Ok((items, new_cursor))
    }

    let cursor = match cursor {
        None => {
            let (items, c) = baseline(token_key, drive_id).await?;
            return Ok((items, Some((drive_id.to_string(), c))));
        }
        Some(c) => c,
    };

    let (changes, new_cursor) = match drive::list_shared_changes(token_key, drive_id, &cursor).await
    {
        Ok(v) => v,
        Err(e) if drive::is_cursor_expired(&e) => {
            let (items, c) = baseline(token_key, drive_id).await?;
            return Ok((items, Some((drive_id.to_string(), c))));
        }
        Err(e) => return Err(e),
    };

    let mut items: Vec<DriveItem> = Vec::with_capacity(changes.len());
    for change in &changes {
        let source_id = drive::shared_source_id(drive_id, &change.file_id);
        let known = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::read_item_state(&conn, &source_id)?.is_some()
        };
        if let Some(event) = drive::map_shared_change(change, drive_id, known) {
            items.push(DriveItem::Reconciled {
                source_id,
                event,
                file: change.file.clone(),
            });
        }
    }
    Ok((items, Some((drive_id.to_string(), new_cursor))))
}

/// Apply `f` to the shared drive-sync snapshot, best-effort (a poisoned lock is skipped). Binding the
/// lock guard to a named local first sidesteps the `if let` temporary-lifetime pitfall.
fn with_drive_snap(app: &AppHandle, f: impl FnOnce(&mut crate::DriveSyncState)) {
    let state = app.state::<AppState>();
    let guard = state.drive_sync.lock();
    if let Ok(mut snap) = guard {
        f(&mut snap);
    }
}

/// Update the shared drive-sync snapshot and broadcast a `drive://sync` progress event globally. The
/// snapshot lets the UI restore an in-flight sync after navigating away; the global event (vs a
/// per-call Channel) means progress reaches whatever component is mounted, not just the starter.
fn emit_drive_progress(app: &AppHandle, ev: drive::DriveSyncEvent) {
    with_drive_snap(app, |snap| match &ev {
        drive::DriveSyncEvent::Counted { total } => {
            snap.total = Some(*total);
            snap.processed = 0;
        }
        drive::DriveSyncEvent::Item {
            processed, total, ..
        } => {
            snap.processed = *processed;
            snap.total = Some(*total);
        }
        // Keep the last result in the snapshot too, so a user returning to Settings after the sync
        // finished still sees the summary (the live event only reaches a mounted listener).
        drive::DriveSyncEvent::Finished { report } => {
            snap.last_report = Some(report.clone());
        }
    });
    let _ = app.emit("drive://sync", ev);
}

/// Key in the `settings` table marking a sync that was started but not cleanly finished. Set when a
/// sync begins and removed when it ends (completed or stopped); a value surviving across a restart
/// therefore means the app was closed/crashed mid-index, and [`crate::commands::resume_drive_sync`]
/// picks it back up.
pub(crate) const DRIVE_SYNC_PENDING_KEY: &str = "drive_sync_pending";

/// True if the running sync has been asked to stop.
fn sync_cancelled(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .drive_sync_cancel
        .load(Ordering::SeqCst)
}

/// Record a file that couldn't be indexed, up to the cap (after which we just flag truncation).
fn record_issue(
    issues: &mut Vec<drive::DriveSyncIssue>,
    truncated: &mut bool,
    name: &str,
    reason: &str,
) {
    if issues.len() < connector_sync::MAX_REPORT_ISSUES {
        issues.push(drive::DriveSyncIssue {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    } else {
        *truncated = true;
    }
}

/// The sync engine behind both [`crate::commands::sync_drive`] and
/// [`crate::commands::resume_drive_sync`], honouring each account's
/// scope. **My Drive** (on by default) uses the efficient delta cursor — the first sync enumerates
/// everything (the slow one the UI warns about), later syncs apply only the changes feed. **Shared
/// drives** the account opted into are re-enumerated and reconciled each pass (whole drive, or just
/// the selected folders). Every item is index-only: a pointer + embedding, the body fetched live.
/// Never holds the DB lock across a network/embed call (rule #4).
///
/// **Runs detached**: progress is broadcast via the global `drive://sync` event (not a per-call
/// Channel) and mirrored into [`AppState::drive_sync`], so the sync keeps running — and the UI keeps
/// reflecting it — even if the user leaves the Settings page.
///
/// **Single-flight**: a request arriving while a sync is already running (e.g. the user connected
/// another account mid-index, or a stray button press) is folded into one follow-up all-accounts
/// pass rather than starting a second, racy sync — backend-enforced, so the UI can't break it. The
/// follow-up cheaply re-checks already-synced accounts (delta) and fully indexes any new one.
///
/// **Durable**: a crash-resume marker is persisted while running and cleared on a clean exit, so an
/// interrupted run is resumed on next launch. Already-indexed files survive (each is committed as it
/// goes), so a stop or crash never loses work. Returns the number of items touched by the last pass.
pub(crate) async fn drive_sync_core(app: &AppHandle, account: Option<String>) -> Result<usize> {
    let st: &AppState = app.state::<AppState>().inner();
    connector_sync::run_detached_sync(
        st,
        &st.drive_sync,
        &st.drive_sync_cancel,
        DRIVE_SYNC_PENDING_KEY,
        account,
        |target| run_drive_sync(app, target),
    )
    .await
}

/// One sync pass: gather each account's work, then process it. Split out so [`drive_sync_core`] can
/// run it more than once (the follow-up sweep) and so the wrapper owns the running/marker lifecycle.
async fn run_drive_sync(app: &AppHandle, account: Option<String>) -> Result<usize> {
    // The engine is needed for index-only embedding + binary conversion — ensure it once up front.
    {
        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
    }

    let emails: Vec<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        match account {
            Some(e) => vec![e],
            None => drive::list_accounts(&conn)?
                .into_iter()
                .map(|a| a.email)
                .collect(),
        }
    };

    let mut work: Vec<AccountWork> = Vec::new();
    let mut last_err: Option<Error> = None;

    // Phase 1 — gather each account's work off the lock: My Drive via its delta cursor, then each
    // opted-in shared drive via a reconcile. One AccountWork per account carries both.
    for email in emails {
        let token_key = drive::account_token_key(&email);
        let scope = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::get_scope(&conn, &email)?
        };

        let mut items: Vec<DriveItem> = Vec::new();
        let mut new_cursor: Option<String> = None;
        let mut shared_new_cursors: Vec<(String, String)> = Vec::new();
        let mut auth_failed = false;

        // --- My Drive. Whole-drive uses the cheap delta cursor; folder-scoped re-enumerates +
        // reconciles (like a folder-scoped shared drive), advancing no cursor. ---
        if scope.my_drive {
            match scope.my_drive_folders.as_deref() {
                Some(folders) => {
                    match gather_my_drive_folders(app, &token_key, &email, folders).await {
                        Ok(mut recon) => items.append(&mut recon),
                        Err(e) => {
                            if drive::is_auth_failure(&e) {
                                auth_failed = true;
                            }
                            last_err = Some(e);
                        }
                    }
                }
                None => {
                    let cursor = {
                        let state = app.state::<AppState>();
                        let conn = state.conn()?;
                        drive::get_cursor(&conn, &email)?
                    };
                    // A full enumerate + fresh baseline cursor (first sync, or a 410 cursor reset).
                    let full_relist = |token_key: String| async move {
                        let files = drive::enumerate_drive(&token_key).await?;
                        let cursor = drive::start_page_token(&token_key, None).await?;
                        Ok::<_, Error>((files, cursor))
                    };
                    let outcome: Result<(Vec<DriveItem>, String)> = if cursor.is_none() {
                        full_relist(token_key.clone()).await.map(|(files, c)| {
                            (files.into_iter().map(DriveItem::Enumerated).collect(), c)
                        })
                    } else {
                        match drive::list_changes(&token_key, cursor.as_deref().unwrap_or("")).await
                        {
                            Ok((changes, c)) => {
                                Ok((changes.into_iter().map(DriveItem::Changed).collect(), c))
                            }
                            Err(e) if drive::is_cursor_expired(&e) => {
                                full_relist(token_key.clone()).await.map(|(files, c)| {
                                    (files.into_iter().map(DriveItem::Enumerated).collect(), c)
                                })
                            }
                            Err(e) => Err(e),
                        }
                    };
                    match outcome {
                        Ok((mut my_items, c)) => {
                            items.append(&mut my_items);
                            new_cursor = Some(c);
                        }
                        Err(e) => {
                            if drive::is_auth_failure(&e) {
                                auth_failed = true;
                            }
                            last_err = Some(e);
                        }
                    }
                }
            }
        }

        // --- shared drives. Skip if the account already auth-failed (same token). Shared drives are
        // de-duplicated across accounts: `claim_or_skip_shared_drive` records this account's access and
        // tells us whether it OWNS the drive (the first account to sync it does). A drive owned by
        // another account is skipped here — that owner already indexes it (the scope UI greys it out).
        // Whole-drive selections return an advanced cursor to persist; folder-scoped ones return None. ---
        if !auth_failed {
            for sel in &scope.shared {
                let owns = {
                    let state = app.state::<AppState>();
                    let conn = state.conn()?;
                    drive::claim_or_skip_shared_drive(&conn, &email, &sel.drive_id, &sel.name)?
                };
                if !owns {
                    continue;
                }
                match gather_shared(app, &token_key, &email, sel).await {
                    Ok((mut recon, cursor)) => {
                        items.append(&mut recon);
                        if let Some(advanced) = cursor {
                            shared_new_cursors.push(advanced);
                        }
                    }
                    Err(e) => {
                        if drive::is_auth_failure(&e) {
                            auth_failed = true;
                        }
                        last_err = Some(e);
                        if auth_failed {
                            break;
                        }
                    }
                }
            }
        }

        work.push(AccountWork {
            email,
            token_key,
            items,
            new_cursor,
            shared_new_cursors,
            auth_failed,
        });
    }

    let total: usize = work.iter().map(|w| w.items.len()).sum();
    emit_drive_progress(app, drive::DriveSyncEvent::Counted { total });

    // Phase 2 — process each item: react → fetch body only when needed → apply (embed off the lock).
    let (mut indexed, mut updated, mut removed, mut skipped, mut failed) = (0, 0, 0, 0, 0usize);
    let mut processed = 0usize;
    // Files we attempted but couldn't index (unsupported/empty, or a fetch error), for the report.
    let mut issues: Vec<drive::DriveSyncIssue> = Vec::new();
    let mut issues_truncated = false;
    // Set if the user pressed Stop. Already-applied items stay committed; we stop early and skip the
    // interrupted account's cursor advance, so the next sync re-checks it.
    let mut cancelled = false;
    // Parent-folder names resolved once per unique folder id for the whole pass (see
    // [`resolve_folder_name`]) — snapshotted onto each newly-synced document as review context.
    let mut folder_names: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();

    'accounts: for w in &work {
        // Stop requested (before this account, or after finishing the previous one)? Halt — keeping
        // everything indexed so far. The interrupted account's cursor is left unadvanced below.
        if sync_cancelled(app) {
            cancelled = true;
            break 'accounts;
        }

        // Any item in this account failing (a body fetch or an apply) blocks the clean finalize below:
        // the account is stamped 'error' with its cursor left unadvanced instead of a misleading 'ok'.
        // Reset per account so one bad account doesn't taint the next (the global `failed` counter is
        // cross-account and can't gate this per-account decision). Mirrors the calendar sync's "check
        // failures first" rule (F-29).
        let mut account_failed = false;

        // A whole-account auth failure fans every item out to `unreachable` (never mass deletion).
        if w.auth_failed {
            let actions = index_only::react(
                index_only::ChangeEvent::SourceFailure {
                    source: format!("gdrive:{}", w.email),
                },
                None,
            );
            let app2 = app.clone();
            let _ = tokio::task::spawn_blocking(move || {
                connector_sync::apply_connector_actions(&app2, &actions, None)
            })
            .await;
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::set_state(&conn, &w.email, "error")?;
            continue;
        }

        for item in &w.items {
            // Stop requested mid-account? Halt after the current file — already-indexed files stay.
            if sync_cancelled(app) {
                cancelled = true;
                break 'accounts;
            }
            let (file, source_id, event): (Option<&drive::DriveFile>, String, _) = match item {
                DriveItem::Enumerated(file) => {
                    let sid = drive::source_id_for(&w.email, &file.id);
                    let ev = index_only::ChangeEvent::Add {
                        source_id: sid.clone(),
                        modified_at: file.modified_time.clone(),
                    };
                    (Some(file), sid, ev)
                }
                DriveItem::Changed(change) => {
                    let sid = drive::source_id_for(&w.email, &change.file_id);
                    let known = {
                        let state = app.state::<AppState>();
                        let conn = state.conn()?;
                        drive::read_item_state(&conn, &sid)?.is_some()
                    };
                    match drive::map_change(change, &w.email, known) {
                        Some(ev) => (change.file.as_ref(), sid, ev),
                        None => {
                            processed += 1;
                            skipped += 1;
                            continue;
                        }
                    }
                }
                // Shared-drive reconcile: the event (Update/Delete) is already built.
                DriveItem::Reconciled {
                    source_id,
                    event,
                    file,
                } => (file.as_ref(), source_id.clone(), event.clone()),
            };

            let name = file
                .map(|f| f.name.clone())
                .unwrap_or_else(|| source_id.clone());
            let current = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                drive::read_item_state(&conn, &source_id)?
            };
            let actions = index_only::react(event, current.as_ref());
            let category = connector_sync::action_category(&actions);

            let needs_body = actions.iter().any(|a| {
                matches!(
                    a,
                    index_only::Action::IngestNew { .. } | index_only::Action::ReEmbed { .. }
                )
            });
            let fetched = if needs_body {
                let body = match file {
                    Some(f) => {
                        let state = app.state::<AppState>();
                        drive::fetch_body(state.inner(), &w.token_key, f).await
                    }
                    None => Ok(None),
                };
                // Snapshot the parent-folder name (cached per pass) alongside the body — plain review
                // context for the sorting proposal, resolved off the file's first `parents` entry.
                let folder_name = match file.and_then(|f| f.parent_id.as_deref()) {
                    Some(pid) => resolve_folder_name(&w.token_key, pid, &mut folder_names).await,
                    None => None,
                };
                match body {
                    Ok(Some(text)) => file.map(|f| f.pointer(source_id.clone(), text, folder_name)),
                    Ok(None) => {
                        processed += 1;
                        skipped += 1;
                        record_issue(
                            &mut issues,
                            &mut issues_truncated,
                            &name,
                            "No extractable text (unsupported file type or empty)",
                        );
                        emit_drive_progress(
                            app,
                            drive::DriveSyncEvent::Item {
                                processed,
                                total,
                                name,
                            },
                        );
                        continue;
                    }
                    Err(e) => {
                        processed += 1;
                        failed += 1;
                        record_issue(
                            &mut issues,
                            &mut issues_truncated,
                            &name,
                            &format!("Couldn't fetch from Drive: {e}"),
                        );
                        last_err = Some(e);
                        account_failed = true;
                        emit_drive_progress(
                            app,
                            drive::DriveSyncEvent::Item {
                                processed,
                                total,
                                name,
                            },
                        );
                        continue;
                    }
                }
            } else {
                None
            };

            let app2 = app.clone();
            let apply = tokio::task::spawn_blocking(move || {
                connector_sync::apply_connector_actions(&app2, &actions, fetched)
            })
            .await
            .map_err(|e| Error::Other(format!("drive apply task panicked: {e}")))?;
            match apply {
                Ok(()) => match category {
                    connector_sync::ActionKind::Indexed => indexed += 1,
                    connector_sync::ActionKind::Updated => updated += 1,
                    connector_sync::ActionKind::Removed => removed += 1,
                    connector_sync::ActionKind::Other => skipped += 1,
                },
                Err(e) => {
                    failed += 1;
                    record_issue(
                        &mut issues,
                        &mut issues_truncated,
                        &name,
                        &format!("Indexing failed: {e}"),
                    );
                    last_err = Some(e);
                    account_failed = true;
                }
            }
            processed += 1;
            emit_drive_progress(
                app,
                drive::DriveSyncEvent::Item {
                    processed,
                    total,
                    name,
                },
            );
            // Gentle mode: breathe between files so indexing doesn't pin the CPU continuously. Re-read
            // the setting each item (a cheap read, dropped before the await per rule #4) so flipping
            // Fast/Gentle mid-sync takes effect on the very next file, not only on the next run. Only
            // items that reached here did real work (the cheap no-op/skip paths `continue` above).
            let pause_ms = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                db::indexing_pause_ms(&conn)
            };
            if pause_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await;
            }
        }

        // Persist the pass, honoring whether any item failed: a clean account commits (cursor advance,
        // time, state 'ok'); an account with ANY failed item is flagged 'error' with its cursor left
        // unadvanced, so the failure isn't hidden behind a misleading 'ok' and the failed items retry
        // next sync. See `drive::finalize_or_flag`. Auth-failed accounts already `continue`d above. (F-29)
        {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::finalize_or_flag(
                &conn,
                &w.email,
                account_failed,
                w.new_cursor.as_deref(),
                &w.shared_new_cursors,
            )?;
        }
    }

    let report = drive::DriveSyncReport {
        indexed,
        updated,
        removed,
        skipped,
        failed,
        cancelled,
        issues,
        issues_truncated,
    };
    emit_drive_progress(app, drive::DriveSyncEvent::Finished { report });

    // A deliberate stop isn't an error. Otherwise surface a failure (auth/expired) even when some
    // items succeeded — the good ones are already committed.
    if !cancelled {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(indexed + updated + removed)
}

/// One unit of OneDrive sync work for an account, gathered off the lock in phase 1.
enum OneDriveItem {
    /// A whole-drive first-sync / re-baseline file → `Add` (reactivates a previously-missing item).
    Enumerated(onedrive::DriveItem),
    /// A whole-drive incremental delta entry → mapped via `map_change` in phase 2.
    Delta(onedrive::DriveDelta),
    /// A folder-scoped reconcile result with its event pre-built (`Add`/`Update`/`Delete`).
    Reconciled {
        source_id: String,
        event: index_only::ChangeEvent,
        item: Option<onedrive::DriveItem>,
    },
}

/// All of one account's gathered work for a sync pass.
struct OneDriveAccountWork {
    email: String,
    token_key: String,
    items: Vec<OneDriveItem>,
    /// The advanced whole-drive delta link — set only when the whole drive synced this pass.
    new_cursor: Option<String>,
    auth_failed: bool,
}

/// Folder-scoped reconcile for OneDrive: enumerate the selected folders live and diff against the
/// account's currently-healthy items. Present+known → `Update` (catches edits); present+new/missing →
/// `Add` (ingests, or reactivates a folder removed then re-added); known-but-absent → `Delete`. Reads
/// the known set under a brief lock; the enumeration itself is off the lock.
async fn gather_onedrive_folders(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    folders: &[String],
) -> Result<Vec<OneDriveItem>> {
    let items = onedrive::enumerate_folders(token_key, folders).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        onedrive::known_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    Ok(
        index_only::reconcile_enumeration(items, known, |id| onedrive::source_id_for(email, id))
            .into_iter()
            .map(|r| OneDriveItem::Reconciled {
                source_id: r.source_id,
                event: r.event,
                item: r.payload,
            })
            .collect(),
    )
}

/// Apply `f` to the shared OneDrive-sync snapshot, best-effort (a poisoned lock is skipped).
fn with_onedrive_snap(app: &AppHandle, f: impl FnOnce(&mut crate::OneDriveSyncState)) {
    let state = app.state::<AppState>();
    let guard = state.onedrive_sync.lock();
    if let Ok(mut snap) = guard {
        f(&mut snap);
    }
}

/// Update the OneDrive-sync snapshot and broadcast a `onedrive://sync` progress event globally (so
/// progress reaches whatever component is mounted, and the UI can restore an in-flight sync).
fn emit_onedrive_progress(app: &AppHandle, ev: onedrive::OneDriveSyncEvent) {
    with_onedrive_snap(app, |snap| match &ev {
        onedrive::OneDriveSyncEvent::Counted { total } => {
            snap.total = Some(*total);
            snap.processed = 0;
        }
        onedrive::OneDriveSyncEvent::Item {
            processed, total, ..
        } => {
            snap.processed = *processed;
            snap.total = Some(*total);
        }
        onedrive::OneDriveSyncEvent::Finished { report } => {
            snap.last_report = Some(report.clone());
        }
    });
    let _ = app.emit("onedrive://sync", ev);
}

/// Marker key for a OneDrive sync started but not cleanly finished (crash-resume); see the Drive
/// equivalent.
pub(crate) const ONEDRIVE_SYNC_PENDING_KEY: &str = "onedrive_sync_pending";

/// True if the running OneDrive sync has been asked to stop.
fn onedrive_sync_cancelled(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .onedrive_sync_cancel
        .load(Ordering::SeqCst)
}

/// Record a file that couldn't be indexed, up to the cap (after which we just flag truncation).
fn record_onedrive_issue(
    issues: &mut Vec<onedrive::OneDriveSyncIssue>,
    truncated: &mut bool,
    name: &str,
    reason: &str,
) {
    if issues.len() < connector_sync::MAX_REPORT_ISSUES {
        issues.push(onedrive::OneDriveSyncIssue {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    } else {
        *truncated = true;
    }
}

/// The OneDrive sync engine (mirrors [`drive_sync_core`]): detached, single-flight, durable. The
/// whole drive uses the efficient Graph delta cursor (first sync enumerates everything, later syncs
/// apply only changes); a folder-scoped account is re-enumerated + reconciled each pass. Every item is
/// index-only: a pointer + embedding, the body fetched live. Never holds the DB lock across a
/// network/embed call (rule #4).
pub(crate) async fn onedrive_sync_core(app: &AppHandle, account: Option<String>) -> Result<usize> {
    let st: &AppState = app.state::<AppState>().inner();
    connector_sync::run_detached_sync(
        st,
        &st.onedrive_sync,
        &st.onedrive_sync_cancel,
        ONEDRIVE_SYNC_PENDING_KEY,
        account,
        |target| run_onedrive_sync(app, target),
    )
    .await
}

/// One OneDrive sync pass: gather each account's work, then process it.
async fn run_onedrive_sync(app: &AppHandle, account: Option<String>) -> Result<usize> {
    {
        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
    }

    let emails: Vec<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        match account {
            Some(e) => vec![e],
            None => onedrive::list_accounts(&conn)?
                .into_iter()
                .map(|a| a.email)
                .collect(),
        }
    };

    let mut work: Vec<OneDriveAccountWork> = Vec::new();
    let mut last_err: Option<Error> = None;

    // Phase 1 — gather each account's work off the lock.
    for email in emails {
        let token_key = onedrive::account_token_key(&email);
        let scope = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            onedrive::get_scope(&conn, &email)?
        };

        let mut items: Vec<OneDriveItem> = Vec::new();
        let mut new_cursor: Option<String> = None;
        let mut auth_failed = false;

        match scope.folders.as_deref() {
            // Folder-scoped: re-enumerate selected folders + reconcile (no cursor).
            Some(folders) => {
                match gather_onedrive_folders(app, &token_key, &email, folders).await {
                    Ok(mut recon) => items.append(&mut recon),
                    Err(e) => {
                        if onedrive::is_auth_failure(&e) {
                            auth_failed = true;
                        }
                        last_err = Some(e);
                    }
                }
            }
            // Whole drive: the Graph delta cursor. No cursor (or a 410 reset) re-baselines via the
            // no-token delta (every file → Enumerated/Add); otherwise pull the incremental delta.
            None => {
                let cursor = {
                    let state = app.state::<AppState>();
                    let conn = state.conn()?;
                    onedrive::get_cursor(&conn, &email)?
                };
                let outcome: Result<(Vec<OneDriveItem>, String)> = match &cursor {
                    None => onedrive::list_delta(&token_key, None)
                        .await
                        .map(|(deltas, link)| {
                            (
                                deltas
                                    .into_iter()
                                    .filter_map(|d| d.file)
                                    .map(OneDriveItem::Enumerated)
                                    .collect(),
                                link,
                            )
                        }),
                    Some(link) => match onedrive::list_delta(&token_key, Some(link)).await {
                        Ok((deltas, link)) => {
                            Ok((deltas.into_iter().map(OneDriveItem::Delta).collect(), link))
                        }
                        Err(e) if onedrive::is_cursor_expired(&e) => {
                            onedrive::list_delta(&token_key, None)
                                .await
                                .map(|(deltas, link)| {
                                    (
                                        deltas
                                            .into_iter()
                                            .filter_map(|d| d.file)
                                            .map(OneDriveItem::Enumerated)
                                            .collect(),
                                        link,
                                    )
                                })
                        }
                        Err(e) => Err(e),
                    },
                };
                match outcome {
                    Ok((mut its, link)) => {
                        items.append(&mut its);
                        new_cursor = Some(link);
                    }
                    Err(e) => {
                        if onedrive::is_auth_failure(&e) {
                            auth_failed = true;
                        }
                        last_err = Some(e);
                    }
                }
            }
        }

        work.push(OneDriveAccountWork {
            email,
            token_key,
            items,
            new_cursor,
            auth_failed,
        });
    }

    let total: usize = work.iter().map(|w| w.items.len()).sum();
    emit_onedrive_progress(app, onedrive::OneDriveSyncEvent::Counted { total });

    // Phase 2 — process each item: react → fetch body only when needed → apply (embed off the lock).
    let (mut indexed, mut updated, mut removed, mut skipped, mut failed) = (0, 0, 0, 0, 0usize);
    let mut processed = 0usize;
    let mut issues: Vec<onedrive::OneDriveSyncIssue> = Vec::new();
    let mut issues_truncated = false;
    let mut cancelled = false;

    'accounts: for w in &work {
        if onedrive_sync_cancelled(app) {
            cancelled = true;
            break 'accounts;
        }

        // Any item in this account failing (a body fetch or an apply) blocks the clean finalize below:
        // the account is stamped 'error' with its cursor left unadvanced instead of a misleading 'ok'.
        // Reset per account so one bad account doesn't taint the next (the global `failed` counter is
        // cross-account and can't gate this per-account decision). Mirrors the calendar sync's "check
        // failures first" rule (F-29).
        let mut account_failed = false;

        // A whole-account auth failure fans every item out to `unreachable` (never mass deletion).
        if w.auth_failed {
            let actions = index_only::react(
                index_only::ChangeEvent::SourceFailure {
                    source: format!("onedrive:{}", w.email),
                },
                None,
            );
            let app2 = app.clone();
            let _ = tokio::task::spawn_blocking(move || {
                connector_sync::apply_connector_actions(&app2, &actions, None)
            })
            .await;
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            onedrive::set_state(&conn, &w.email, "error")?;
            continue;
        }

        for item in &w.items {
            if onedrive_sync_cancelled(app) {
                cancelled = true;
                break 'accounts;
            }
            let (file, source_id, event): (Option<&onedrive::DriveItem>, String, _) = match item {
                OneDriveItem::Enumerated(it) => {
                    let sid = onedrive::source_id_for(&w.email, &it.id);
                    let ev = index_only::ChangeEvent::Add {
                        source_id: sid.clone(),
                        modified_at: it.modified_time.clone(),
                    };
                    (Some(it), sid, ev)
                }
                OneDriveItem::Delta(delta) => {
                    let sid = onedrive::source_id_for(&w.email, &delta.item_id);
                    let known = {
                        let state = app.state::<AppState>();
                        let conn = state.conn()?;
                        onedrive::read_item_state(&conn, &sid)?.is_some()
                    };
                    match onedrive::map_change(delta, &w.email, known) {
                        Some(ev) => (delta.file.as_ref(), sid, ev),
                        None => {
                            processed += 1;
                            skipped += 1;
                            continue;
                        }
                    }
                }
                OneDriveItem::Reconciled {
                    source_id,
                    event,
                    item,
                } => (item.as_ref(), source_id.clone(), event.clone()),
            };

            let name = file
                .map(|f| f.name.clone())
                .unwrap_or_else(|| source_id.clone());
            let current = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                onedrive::read_item_state(&conn, &source_id)?
            };
            let actions = index_only::react(event, current.as_ref());
            let category = connector_sync::action_category(&actions);

            let needs_body = actions.iter().any(|a| {
                matches!(
                    a,
                    index_only::Action::IngestNew { .. } | index_only::Action::ReEmbed { .. }
                )
            });
            let fetched = if needs_body {
                let body = match file {
                    Some(f) => {
                        let state = app.state::<AppState>();
                        onedrive::fetch_body(state.inner(), &w.token_key, f).await
                    }
                    None => Ok(None),
                };
                match body {
                    Ok(Some(text)) => file.map(|f| f.pointer(source_id.clone(), text)),
                    Ok(None) => {
                        processed += 1;
                        skipped += 1;
                        record_onedrive_issue(
                            &mut issues,
                            &mut issues_truncated,
                            &name,
                            "No extractable text (unsupported file type or empty)",
                        );
                        emit_onedrive_progress(
                            app,
                            onedrive::OneDriveSyncEvent::Item {
                                processed,
                                total,
                                name,
                            },
                        );
                        continue;
                    }
                    Err(e) => {
                        processed += 1;
                        failed += 1;
                        record_onedrive_issue(
                            &mut issues,
                            &mut issues_truncated,
                            &name,
                            &format!("Couldn't fetch from OneDrive: {e}"),
                        );
                        last_err = Some(e);
                        account_failed = true;
                        emit_onedrive_progress(
                            app,
                            onedrive::OneDriveSyncEvent::Item {
                                processed,
                                total,
                                name,
                            },
                        );
                        continue;
                    }
                }
            } else {
                None
            };

            let app2 = app.clone();
            let apply = tokio::task::spawn_blocking(move || {
                connector_sync::apply_connector_actions(&app2, &actions, fetched)
            })
            .await
            .map_err(|e| Error::Other(format!("onedrive apply task panicked: {e}")))?;
            match apply {
                Ok(()) => match category {
                    connector_sync::ActionKind::Indexed => indexed += 1,
                    connector_sync::ActionKind::Updated => updated += 1,
                    connector_sync::ActionKind::Removed => removed += 1,
                    connector_sync::ActionKind::Other => skipped += 1,
                },
                Err(e) => {
                    failed += 1;
                    record_onedrive_issue(
                        &mut issues,
                        &mut issues_truncated,
                        &name,
                        &format!("Indexing failed: {e}"),
                    );
                    last_err = Some(e);
                    account_failed = true;
                }
            }
            processed += 1;
            emit_onedrive_progress(
                app,
                onedrive::OneDriveSyncEvent::Item {
                    processed,
                    total,
                    name,
                },
            );
            let pause_ms = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                db::indexing_pause_ms(&conn)
            };
            if pause_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await;
            }
        }

        // Persist the pass, honoring whether any item failed: a clean account commits (cursor advance,
        // time, state 'ok'); an account with ANY failed item is flagged 'error' with its cursor left
        // unadvanced, so the failure isn't hidden behind a misleading 'ok' and the failed items retry
        // next sync. See `onedrive::finalize_or_flag`. Auth-failed accounts already `continue`d above. (F-29)
        {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            onedrive::finalize_or_flag(&conn, &w.email, account_failed, w.new_cursor.as_deref())?;
        }
    }

    let report = onedrive::OneDriveSyncReport {
        indexed,
        updated,
        removed,
        skipped,
        failed,
        cancelled,
        issues,
        issues_truncated,
    };
    emit_onedrive_progress(app, onedrive::OneDriveSyncEvent::Finished { report });

    if !cancelled {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(indexed + updated + removed)
}
