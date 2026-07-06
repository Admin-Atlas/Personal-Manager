// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Google Drive and OneDrive sync engines, unified behind one [`CloudDriver`]. Each is a detached,
//! single-flight, crash-resumable pass that gathers a connected account's work off the DB lock (phase
//! 1: a whole-drive delta cursor or a folder-scoped reconcile), then processes it item by item (phase
//! 2: `index_only::react` -> fetch a body only when needed -> apply off the lock). Phase 2 — identical
//! between the two providers except for the pointer's parent-folder tagging (Drive only) and the
//! provider labels in messages — is written **once** in [`run_cloud_pass`]; the genuinely
//! provider-specific parts (phase-1 gathering, the fetch/pointer/finalize seams, the `AppState`
//! snapshot + event name) live behind the [`CloudDriver`] trait, implemented by [`DriveDriver`] and
//! [`OneDriveDriver`]. This replaces the two ~350-line `run_*_sync` engines that mirrored each other
//! byte-for-byte (audit X-D1). The shared single-flight + crash-resume-marker lifecycle and the
//! blocking index-only apply live in [`crate::connector_sync`]; the local-folder connector keeps its
//! own engine in [`crate::localfolder`] (it has no cloud sibling to share a driver with).
//!
//! The driver methods that touch the network are declared `-> impl Future<…> + Send` (native RPITIT,
//! no `async-trait`): [`crate::commands::resume_drive_sync`] spawns the core onto the async runtime, so
//! the whole generic pass future must be `Send`, and the explicit bound makes that provable through the
//! generic `C`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{Error, Result};
use crate::{connector_sync, db, drive, index_only, onedrive, AppState};

// --- unified report / progress types (shared by both cloud connectors) ---------------------------

/// A file PM tried to index but couldn't, surfaced in the post-sync report so the user knows what was
/// left out (e.g. an unsupported file type MarkItDown can't read, or a fetch error). Not a fatal
/// error — the sync carries on; these are just reported. Shared by Drive + OneDrive (their reports
/// were byte-identical before this unification); the local-folder connector keeps its own.
#[derive(Clone, Serialize, Default)]
pub struct CloudSyncIssue {
    pub name: String,
    pub reason: String,
}

/// The outcome of a cloud sync pass: how many items were indexed/updated/removed, the list of files
/// that couldn't be indexed (capped), and whether the user stopped it early. Shown in Settings after a
/// sync and stashed in the live snapshot so a user returning after it finished still sees the result.
#[derive(Clone, Serialize, Default)]
pub struct CloudSyncReport {
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    /// The user pressed Stop — already-indexed files are kept; the rest were left for next time.
    pub cancelled: bool,
    /// Files attempted but not indexed (unsupported/empty, or a fetch error), capped for memory.
    pub issues: Vec<CloudSyncIssue>,
    /// True when more files couldn't be indexed than the capped `issues` list holds.
    pub issues_truncated: bool,
}

/// Progress for a running cloud sync, broadcast globally on the driver's `EVENT_NAME` (`drive://sync`
/// / `onedrive://sync`) and mirrored into the shared snapshot. The frontend maps `processed`/`total`
/// onto the shared `IngestProgress` bar and shows `report` when finished. One event type for both
/// providers — the JSON shape is unchanged from the former `DriveSyncEvent`/`OneDriveSyncEvent`, so
/// the existing per-provider listeners keep working.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CloudSyncEvent {
    /// The total number of files/changes this run will work through (sent once, before the items).
    Counted { total: usize },
    /// One item processed (1-based `processed` of `total`).
    Item {
        processed: usize,
        total: usize,
        name: String,
    },
    /// The run is done; `report` carries the breakdown + the not-indexed list (+ a `cancelled` flag).
    Finished { report: CloudSyncReport },
}

/// Marker key for a Drive sync started but not cleanly finished (crash-resume). Set when a sync begins
/// and removed when it ends (completed or stopped); a value surviving a restart means the app was
/// closed/crashed mid-index, and [`crate::commands::resume_drive_sync`] picks it back up.
pub(crate) const DRIVE_SYNC_PENDING_KEY: &str = "drive_sync_pending";
/// Marker key for a OneDrive sync started but not cleanly finished (crash-resume); the Drive sibling.
pub(crate) const ONEDRIVE_SYNC_PENDING_KEY: &str = "onedrive_sync_pending";

// --- the driver seam -----------------------------------------------------------------------------

/// All of one account's gathered work for a sync pass, generic over the connector's phase-1 work item
/// (`DriveItem` / `OneDriveItem`). Built off the DB lock in phase 1, drained in phase 2.
struct AccountWork<Item> {
    email: String,
    token_key: String,
    items: Vec<Item>,
    /// The advanced whole-drive delta cursor — set only when the whole drive synced this pass.
    new_cursor: Option<String>,
    /// Additional advanced delta cursors to persist — Drive's per-shared-drive `(driveId, cursor)`
    /// pairs (whole-drive shared selections). OneDrive has no such concept and always leaves this
    /// empty; its `finalize_or_flag` ignores it.
    extra_cursors: Vec<(String, String)>,
    auth_failed: bool,
    /// Set when any of this account's listings hit its page/folder guard (audit F-30). For a truncated
    /// *enumeration* (a full re-list / folder reconcile) the gather also withheld the cursor advance and
    /// dropped inferred deletions — absence proves nothing on a partial list. A truncated *delta feed*
    /// instead advances its resumable cursor (nothing withheld: its removes are explicit, so it just
    /// resumes next pass). Either way this flag makes phase 2 surface a coverage-incomplete note so the
    /// partial pass isn't mistaken for a clean one.
    coverage_incomplete: bool,
    /// Set when phase-1 gather hit a soft (non-auth) error — a transient delta/list/reconcile failure
    /// that left the item set possibly incomplete (F-29). Auth failures take the `auth_failed` path
    /// instead. Phase 2 seeds `account_failed` from this so the account finalizes 'error' with its
    /// cursor left unadvanced (retry next pass), rather than stamping a misleading 'ok' + advancing
    /// the cursor past changes a failed gather never saw. Distinct from `coverage_incomplete`, which
    /// is a *successful* but truncated pass.
    gather_failed: bool,
}

/// The phase-2 resolution of one gathered work item: either a no-op to skip (a changes-feed entry that
/// maps to nothing) or a concrete `(fetchable file, source id, change event)` to process. The file
/// borrows from the work item, so it stays available for the body fetch + pointer.
enum Resolved<'a, F> {
    /// The change maps to a no-op (e.g. a trashed-then-gone id we never indexed) — count it skipped.
    Skip,
    Process {
        file: Option<&'a F>,
        source_id: String,
        event: index_only::ChangeEvent,
    },
}

/// What [`run_cloud_pass`] needs from a cloud connector: the provider-specific phase-1 gather, the
/// per-item fetch/pointer/finalize seams, and the `AppState` snapshot + wire-event identity. Everything
/// the two engines shared byte-for-byte (the phase-2 loop, progress emission, issue recording,
/// single-flight lifecycle) lives generically outside the trait.
///
/// `Send`/`Sync` on the trait and its associated types make the monomorphised pass future `Send`, which
/// [`crate::commands::resume_drive_sync`] requires (it spawns the core onto the async runtime).
///
/// Module-private: the two `*_sync_core` entry points below hide it entirely (they don't name it), so
/// nothing outside this file needs to see the driver seam.
trait CloudDriver: Send + Sync {
    /// The fetchable file (`drive::DriveFile` / `onedrive::DriveItem`) — carries the name + builds the
    /// pointer + is passed to `fetch_body`.
    type File: Send + Sync;
    /// The phase-1 work item enum (`DriveItem` / `OneDriveItem`).
    type Item: Send;
    /// Per-pass state threaded into `make_pointer` — Drive's parent-folder-name memo (id → name),
    /// resolved once per folder across a whole pass; `()` for OneDrive, which doesn't tag folders.
    type FolderCache: Default + Send;

    /// The crash-resume marker key ([`DRIVE_SYNC_PENDING_KEY`] / [`ONEDRIVE_SYNC_PENDING_KEY`]).
    const PENDING_KEY: &'static str;
    /// The global progress event name (`drive://sync` / `onedrive://sync`).
    const EVENT_NAME: &'static str;
    /// The source-id namespace prefix (`gdrive` / `onedrive`) — used in the whole-account
    /// `SourceFailure` source string and the apply-panic message.
    const SOURCE_KIND: &'static str;
    /// Human label for the user-facing issue/error text (`Drive` / `OneDrive`).
    const PROVIDER_LABEL: &'static str;

    /// This connector's live-sync snapshot field on [`AppState`] (`drive_sync` / `onedrive_sync`).
    fn snapshot(state: &AppState) -> &Mutex<crate::CloudSyncState>;
    /// This connector's cooperative stop flag on [`AppState`].
    fn cancel_flag(state: &AppState) -> &AtomicBool;

    /// Every connected account's email for an all-accounts pass.
    fn account_emails(conn: &Connection) -> Result<Vec<String>>;
    /// The stored item state for a source id (drives the reducer + the `known` change mapping).
    fn read_item_state(conn: &Connection, source_id: &str)
        -> Result<Option<index_only::ItemState>>;
    /// Flag an account `error` (whole-account auth failure — its cursor is left unadvanced).
    fn set_error_state(conn: &Connection, email: &str) -> Result<()>;
    /// Persist the pass for one account: commit (cursor advance + `ok`) when clean, or flag `error`
    /// with the cursor left unadvanced when any item failed. Drive also persists `extra_cursors`.
    fn finalize_or_flag(
        conn: &Connection,
        work: &AccountWork<Self::Item>,
        account_failed: bool,
    ) -> Result<()>;
    /// The file's display name for the report/progress (falls back to the source id when absent).
    fn file_name(file: &Self::File) -> String;

    /// Gather one account's work off the DB lock (phase 1): the whole-drive delta cursor and/or a
    /// folder-scoped reconcile, plus Drive's opted-in shared drives. Returns the [`AccountWork`] and
    /// any soft (per-account) error to fold into the pass's `last_err` — hard errors (a poisoned
    /// lock, a failed scope read) propagate via `?`.
    fn gather_account(
        &self,
        app: &AppHandle,
        email: String,
    ) -> impl std::future::Future<Output = Result<(AccountWork<Self::Item>, Option<Error>)>> + Send;

    /// Resolve one gathered item into its `(file, source id, event)` for phase 2, or `Skip`. Reads the
    /// store for the incremental (`Changed`/`Delta`) case's `known` check.
    fn resolve_item<'a>(
        &self,
        app: &AppHandle,
        email: &str,
        item: &'a Self::Item,
    ) -> Result<Resolved<'a, Self::File>>;

    /// Fetch a file's body live (only called when the reducer needs one — a fresh ingest or re-embed).
    fn fetch_body(
        state: &AppState,
        token_key: &str,
        file: &Self::File,
    ) -> impl std::future::Future<Output = Result<Option<String>>> + Send;

    /// Build the foundation pointer for a freshly-fetched body. Drive resolves the parent-folder name
    /// (memoised in `cache`) as sorting-review context; OneDrive builds it directly (no folder tag).
    fn make_pointer(
        &self,
        token_key: &str,
        file: &Self::File,
        source_id: String,
        body: String,
        cache: &mut Self::FolderCache,
    ) -> impl std::future::Future<Output = index_only::PointerInput> + Send;
}

// --- generic engine ------------------------------------------------------------------------------

/// Apply `f` to this connector's live-sync snapshot, best-effort (a poisoned lock is skipped). Binding
/// the lock guard to a named local first sidesteps the `if let` temporary-lifetime pitfall.
fn with_cloud_snap<C: CloudDriver>(app: &AppHandle, f: impl FnOnce(&mut crate::CloudSyncState)) {
    let state = app.state::<AppState>();
    let guard = C::snapshot(state.inner()).lock();
    if let Ok(mut snap) = guard {
        f(&mut snap);
    }
}

/// Update the connector's snapshot and broadcast its progress event globally. The snapshot lets the UI
/// restore an in-flight sync after navigating away; the global event (vs a per-call Channel) means
/// progress reaches whatever component is mounted, not just the starter.
fn emit_progress<C: CloudDriver>(app: &AppHandle, ev: CloudSyncEvent) {
    with_cloud_snap::<C>(app, |snap| match &ev {
        CloudSyncEvent::Counted { total } => {
            snap.total = Some(*total);
            snap.processed = 0;
        }
        CloudSyncEvent::Item {
            processed, total, ..
        } => {
            snap.processed = *processed;
            snap.total = Some(*total);
        }
        // Keep the last result in the snapshot too, so a user returning to Settings after the sync
        // finished still sees the summary (the live event only reaches a mounted listener).
        CloudSyncEvent::Finished { report } => {
            snap.last_report = Some(report.clone());
        }
    });
    let _ = app.emit(C::EVENT_NAME, ev);
}

/// True if the running sync has been asked to stop.
fn sync_cancelled<C: CloudDriver>(app: &AppHandle) -> bool {
    C::cancel_flag(app.state::<AppState>().inner()).load(Ordering::SeqCst)
}

/// Record a file that couldn't be indexed, up to the cap (after which we just flag truncation).
fn record_issue(issues: &mut Vec<CloudSyncIssue>, truncated: &mut bool, name: &str, reason: &str) {
    if issues.len() < connector_sync::MAX_REPORT_ISSUES {
        issues.push(CloudSyncIssue {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    } else {
        *truncated = true;
    }
}

/// Drive both cloud sync engines: the detached, single-flight, crash-resumable lifecycle around one
/// [`run_cloud_pass`]. **My Drive / the whole drive** (on by default) uses the efficient delta cursor —
/// the first sync enumerates everything (the slow one the UI warns about), later syncs apply only the
/// changes feed. **Folder-scoped** accounts (and Drive's opted-in shared drives) are re-enumerated and
/// reconciled each pass. Every item is index-only: a pointer + embedding, the body fetched live. Never
/// holds the DB lock across a network/embed call (rule #4).
///
/// **Runs detached**: progress is broadcast via the global `EVENT_NAME` event and mirrored into the
/// shared snapshot, so the sync keeps running — and the UI keeps reflecting it — even if the user
/// leaves Settings. **Single-flight**: a request arriving mid-sync is folded into one follow-up
/// all-accounts pass rather than starting a second, racy sync. **Durable**: a crash-resume marker is
/// persisted while running and cleared on a clean exit, so an interrupted run resumes on next launch.
/// Already-indexed files survive (each is committed as it goes). Returns items touched by the last pass.
async fn run_cloud_sync<C: CloudDriver>(
    app: &AppHandle,
    driver: C,
    account: Option<String>,
) -> Result<usize> {
    let st: &AppState = app.state::<AppState>().inner();
    connector_sync::run_detached_sync(
        st,
        C::snapshot(st),
        C::cancel_flag(st),
        C::PENDING_KEY,
        account,
        |target| run_cloud_pass(app, &driver, target),
    )
    .await
}

/// One sync pass: gather each account's work off the lock (phase 1), then process it item by item
/// (phase 2). Split out so [`run_cloud_sync`]'s single-flight wrapper can run it more than once (the
/// follow-up sweep) and own the running/marker lifecycle.
async fn run_cloud_pass<C: CloudDriver>(
    app: &AppHandle,
    driver: &C,
    account: Option<String>,
) -> Result<usize> {
    // The engine is needed for index-only embedding + binary conversion — ensure it once up front.
    // `ensure_installed` is blocking (a first run installs the Python venv + deps), so keep it off
    // the async runtime — run it on the blocking pool so a first-sync-after-install can't pin a
    // tokio worker (F-41). The cloned handle reaches AppState inside the closure.
    {
        let app = app.clone();
        tokio::task::spawn_blocking(move || app.state::<AppState>().sidecar.ensure_installed())
            .await
            .map_err(|e| Error::Other(format!("sidecar install task panicked: {e}")))??;
    }

    let emails: Vec<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        match account {
            Some(e) => vec![e],
            None => C::account_emails(&conn)?,
        }
    };

    let mut work: Vec<AccountWork<C::Item>> = Vec::new();
    let mut last_err: Option<Error> = None;

    // Phase 1 — gather each account's work off the lock. The driver owns the provider-specific shape
    // (My Drive + shared drives, or a single OneDrive) and reports any soft, per-account error to fold
    // into `last_err`; hard errors propagate via `?`.
    for email in emails {
        let (w, soft_err) = driver.gather_account(app, email).await?;
        if let Some(e) = soft_err {
            last_err = Some(e);
        }
        work.push(w);
    }

    let total: usize = work.iter().map(|w| w.items.len()).sum();
    emit_progress::<C>(app, CloudSyncEvent::Counted { total });

    // Phase 2 — process each item: react → fetch body only when needed → apply (embed off the lock).
    let (mut indexed, mut updated, mut removed, mut skipped, mut failed) = (0, 0, 0, 0, 0usize);
    let mut processed = 0usize;
    // Files we attempted but couldn't index (unsupported/empty, or a fetch error), for the report.
    let mut issues: Vec<CloudSyncIssue> = Vec::new();
    let mut issues_truncated = false;
    // Set if the user pressed Stop. Already-applied items stay committed; we stop early and skip the
    // interrupted account's cursor advance, so the next sync re-checks it.
    let mut cancelled = false;
    // Per-pass driver state (Drive's parent-folder-name memo; `()` for OneDrive).
    let mut folder_cache = C::FolderCache::default();

    'accounts: for w in &work {
        // Stop requested (before this account, or after finishing the previous one)? Halt — keeping
        // everything indexed so far. The interrupted account's cursor is left unadvanced below.
        if sync_cancelled::<C>(app) {
            cancelled = true;
            break 'accounts;
        }

        // Any item in this account failing (a body fetch or an apply) blocks the clean finalize below:
        // the account is stamped 'error' with its cursor left unadvanced instead of a misleading 'ok'.
        // Reset per account so one bad account doesn't taint the next (the global `failed` counter is
        // cross-account and can't gate this per-account decision). Mirrors the calendar sync's "check
        // failures first" rule (F-29). Seeded from the phase-1 gather outcome: a transient gather error
        // already means the pass is incomplete, so the same 'hold the cursor, flag error' applies even
        // if every item that *was* gathered then applies cleanly.
        let mut account_failed = w.gather_failed;

        // A whole-account auth failure fans every item out to `unreachable` (never mass deletion).
        if w.auth_failed {
            let actions = index_only::react(
                index_only::ChangeEvent::SourceFailure {
                    source: format!("{}:{}", C::SOURCE_KIND, w.email),
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
            C::set_error_state(&conn, &w.email)?;
            continue;
        }

        // A gather that hit its page/folder guard already withheld the enumeration's cursor advance and
        // skipped inferred deletions; surface it so the partial pass isn't read as a clean one (F-30).
        // Pushed directly, NOT through the per-file `record_issue` (which is capped): there's at most one
        // of these per account and it's the only report-side signal a pass was partial, so it must never
        // be starved by a full per-file issues list.
        if w.coverage_incomplete {
            issues.push(CloudSyncIssue {
                name: w.email.clone(),
                reason: "Only part of this account could be listed this sync (too many items to page \
                         through at once). Nothing was removed; the rest is picked up on the next sync."
                    .to_string(),
            });
        }

        for item in &w.items {
            // Stop requested mid-account? Halt after the current file — already-indexed files stay.
            if sync_cancelled::<C>(app) {
                cancelled = true;
                break 'accounts;
            }
            let (file, source_id, event) = match driver.resolve_item(app, &w.email, item)? {
                Resolved::Skip => {
                    processed += 1;
                    skipped += 1;
                    continue;
                }
                Resolved::Process {
                    file,
                    source_id,
                    event,
                } => (file, source_id, event),
            };

            let name = file.map(C::file_name).unwrap_or_else(|| source_id.clone());
            let current = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                C::read_item_state(&conn, &source_id)?
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
                        C::fetch_body(state.inner(), &w.token_key, f).await
                    }
                    None => Ok(None),
                };
                match body {
                    // Build the pointer only now a body is in hand — Drive's parent-folder lookup
                    // (memoised in `folder_cache`) rides along here rather than on every attempt.
                    Ok(Some(text)) => match file {
                        Some(f) => Some(
                            driver
                                .make_pointer(
                                    &w.token_key,
                                    f,
                                    source_id.clone(),
                                    text,
                                    &mut folder_cache,
                                )
                                .await,
                        ),
                        None => None,
                    },
                    Ok(None) => {
                        processed += 1;
                        skipped += 1;
                        record_issue(
                            &mut issues,
                            &mut issues_truncated,
                            &name,
                            "No extractable text (unsupported file type or empty)",
                        );
                        emit_progress::<C>(
                            app,
                            CloudSyncEvent::Item {
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
                            &format!("Couldn't fetch from {}: {e}", C::PROVIDER_LABEL),
                        );
                        last_err = Some(e);
                        account_failed = true;
                        emit_progress::<C>(
                            app,
                            CloudSyncEvent::Item {
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
            // Lower-cased label ("drive" / "onedrive") reproduces the pre-unification per-engine panic
            // text exactly; only hit on a spawn_blocking JoinError (an apply-task panic), never normally.
            .map_err(|e| {
                Error::Other(format!(
                    "{} apply task panicked: {e}",
                    C::PROVIDER_LABEL.to_lowercase()
                ))
            })?;
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
            emit_progress::<C>(
                app,
                CloudSyncEvent::Item {
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
        // next sync. Auth-failed accounts already `continue`d above. (F-29)
        {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            C::finalize_or_flag(&conn, w, account_failed)?;
        }
    }

    let report = CloudSyncReport {
        indexed,
        updated,
        removed,
        skipped,
        failed,
        cancelled,
        issues,
        issues_truncated,
    };
    emit_progress::<C>(app, CloudSyncEvent::Finished { report });

    // A deliberate stop isn't an error. Otherwise surface a failure (auth/expired) even when some
    // items succeeded — the good ones are already committed.
    if !cancelled {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(indexed + updated + removed)
}

// --- Google Drive driver -------------------------------------------------------------------------

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

/// One unit of sync work for a Drive account, gathered off the lock in phase 1.
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
) -> Result<(Vec<DriveItem>, Option<(String, String)>, bool)> {
    match sel.folders.as_deref() {
        Some(folders) => {
            let (items, truncated) =
                gather_shared_folders(app, token_key, &sel.drive_id, folders).await?;
            Ok((items, None, truncated))
        }
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
) -> Result<(Vec<DriveItem>, bool)> {
    let (files, truncated) = drive::enumerate_shared(token_key, drive_id, Some(folders)).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_shared_source_ids(&conn, drive_id)?
            .into_iter()
            .collect()
    };
    // A truncated enumeration (`complete = false`) must not infer deletions — a still-present file we
    // didn't reach would otherwise be soft-deleted (F-30).
    let items = index_only::reconcile_enumeration(files, known, !truncated, |file_id| {
        drive::shared_source_id(drive_id, file_id)
    })
    .into_iter()
    .map(drive_reconciled)
    .collect();
    Ok((items, truncated))
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
) -> Result<(Vec<DriveItem>, bool)> {
    let (files, truncated) = drive::enumerate_my_folders(token_key, folders).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_my_drive_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    // A truncated enumeration (`complete = false`) must not infer deletions (F-30).
    let items = index_only::reconcile_enumeration(files, known, !truncated, |file_id| {
        drive::source_id_for(email, file_id)
    })
    .into_iter()
    .map(drive_reconciled)
    .collect();
    Ok((items, truncated))
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
) -> Result<(Vec<DriveItem>, Option<(String, String)>, bool)> {
    let cursor = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::get_shared_cursor(&conn, email, drive_id)?
    };

    // First sync / 410 reset: the whole drive enumerated as Adds + a fresh baseline cursor. Also
    // reports whether the enumeration was truncated (⇒ don't baseline the cursor yet — retry next pass).
    async fn baseline(token_key: &str, drive_id: &str) -> Result<(Vec<DriveItem>, String, bool)> {
        let (files, truncated) = drive::enumerate_shared(token_key, drive_id, None).await?;
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
        Ok((items, new_cursor, truncated))
    }

    // A truncated *baseline enumeration* indexes what it gathered but withholds the cursor (a partial
    // re-list can't be baselined), so the next sync re-enumerates. The delta branch below, by contrast,
    // advances even when truncated — its page token is resumable (F-30).
    let baseline_cursor =
        |truncated: bool, c: String| (!truncated).then(|| (drive_id.to_string(), c));

    let cursor = match cursor {
        None => {
            let (items, c, truncated) = baseline(token_key, drive_id).await?;
            return Ok((items, baseline_cursor(truncated, c), truncated));
        }
        Some(c) => c,
    };

    let (changes, new_cursor, truncated) =
        match drive::list_shared_changes(token_key, drive_id, &cursor).await {
            Ok(v) => v,
            Err(e) if drive::is_cursor_expired(&e) => {
                let (items, c, truncated) = baseline(token_key, drive_id).await?;
                return Ok((items, baseline_cursor(truncated, c), truncated));
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
    // Delta feed: advance the (possibly intermediate) cursor even when truncated — resumable checkpoint,
    // explicit removes only, so it drains a big backlog across passes instead of re-fetching the head.
    Ok((items, Some((drive_id.to_string(), new_cursor)), truncated))
}

/// The Google Drive connector's [`CloudDriver`] — see [`crate::commands::sync_drive`] /
/// [`crate::commands::resume_drive_sync`]. Honours each account's scope: **My Drive** (on by default)
/// uses the efficient delta cursor (first sync enumerates everything, later syncs the changes feed);
/// **shared drives** the account opted into are re-enumerated + reconciled each pass (whole drive, or
/// selected folders), de-duplicated across accounts via `claim_or_skip_shared_drive`.
struct DriveDriver;

impl CloudDriver for DriveDriver {
    type File = drive::DriveFile;
    type Item = DriveItem;
    type FolderCache = std::collections::HashMap<String, Option<String>>;

    const PENDING_KEY: &'static str = DRIVE_SYNC_PENDING_KEY;
    const EVENT_NAME: &'static str = "drive://sync";
    const SOURCE_KIND: &'static str = "gdrive";
    const PROVIDER_LABEL: &'static str = "Drive";

    fn snapshot(state: &AppState) -> &Mutex<crate::CloudSyncState> {
        &state.drive_sync
    }
    fn cancel_flag(state: &AppState) -> &AtomicBool {
        &state.drive_sync_cancel
    }

    fn account_emails(conn: &Connection) -> Result<Vec<String>> {
        Ok(drive::list_accounts(conn)?
            .into_iter()
            .map(|a| a.email)
            .collect())
    }
    fn read_item_state(
        conn: &Connection,
        source_id: &str,
    ) -> Result<Option<index_only::ItemState>> {
        drive::read_item_state(conn, source_id)
    }
    fn set_error_state(conn: &Connection, email: &str) -> Result<()> {
        drive::set_state(conn, email, "error")
    }
    fn finalize_or_flag(
        conn: &Connection,
        work: &AccountWork<DriveItem>,
        account_failed: bool,
    ) -> Result<()> {
        drive::finalize_or_flag(
            conn,
            &work.email,
            account_failed,
            work.new_cursor.as_deref(),
            &work.extra_cursors,
        )
    }
    fn file_name(file: &drive::DriveFile) -> String {
        file.name.clone()
    }

    async fn gather_account(
        &self,
        app: &AppHandle,
        email: String,
    ) -> Result<(AccountWork<DriveItem>, Option<Error>)> {
        let token_key = drive::account_token_key(&email);
        let scope = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::get_scope(&conn, &email)?
        };

        let mut items: Vec<DriveItem> = Vec::new();
        let mut new_cursor: Option<String> = None;
        let mut extra_cursors: Vec<(String, String)> = Vec::new();
        let mut auth_failed = false;
        let mut coverage_incomplete = false;
        let mut last_err: Option<Error> = None;

        // --- My Drive. Whole-drive uses the cheap delta cursor; folder-scoped re-enumerates +
        // reconciles (like a folder-scoped shared drive), advancing no cursor. ---
        if scope.my_drive {
            match scope.my_drive_folders.as_deref() {
                Some(folders) => {
                    match gather_my_drive_folders(app, &token_key, &email, folders).await {
                        Ok((mut recon, truncated)) => {
                            items.append(&mut recon);
                            coverage_incomplete |= truncated;
                        }
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
                    // A full enumerate + fresh baseline cursor (first sync, or a 410 cursor reset). A
                    // truncated enumeration returns `None` for the cursor to persist — a partial
                    // re-list can't be baselined, so the next sync re-enumerates from scratch (F-30).
                    let full_relist = |token_key: String| async move {
                        let (files, truncated) = drive::enumerate_drive(&token_key).await?;
                        let cursor = drive::start_page_token(&token_key, None).await?;
                        Ok::<_, Error>((files, (!truncated).then_some(cursor), truncated))
                    };
                    // The 2nd element is the cursor to PERSIST: `None` = withhold, `Some` = advance. The
                    // changes feed advances even when truncated — its page token is a resumable
                    // checkpoint and the feed has no absence-inferred deletes, so it drains a big backlog
                    // across passes instead of re-fetching the same head forever.
                    let outcome: Result<(Vec<DriveItem>, Option<String>, bool)> = if cursor
                        .is_none()
                    {
                        full_relist(token_key.clone())
                            .await
                            .map(|(files, persist, truncated)| {
                                (
                                    files.into_iter().map(DriveItem::Enumerated).collect(),
                                    persist,
                                    truncated,
                                )
                            })
                    } else {
                        match drive::list_changes(&token_key, cursor.as_deref().unwrap_or("")).await
                        {
                            Ok((changes, c, truncated)) => Ok((
                                changes.into_iter().map(DriveItem::Changed).collect(),
                                Some(c),
                                truncated,
                            )),
                            Err(e) if drive::is_cursor_expired(&e) => {
                                full_relist(token_key.clone()).await.map(
                                    |(files, persist, truncated)| {
                                        (
                                            files.into_iter().map(DriveItem::Enumerated).collect(),
                                            persist,
                                            truncated,
                                        )
                                    },
                                )
                            }
                            Err(e) => Err(e),
                        }
                    };
                    match outcome {
                        Ok((mut my_items, persist_cursor, truncated)) => {
                            items.append(&mut my_items);
                            new_cursor = persist_cursor;
                            coverage_incomplete |= truncated;
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
                    Ok((mut recon, cursor, truncated)) => {
                        items.append(&mut recon);
                        coverage_incomplete |= truncated;
                        // A truncated shared drive returns no cursor, so nothing is pushed here — it
                        // retries next pass rather than baselining past unlisted files (F-30).
                        if let Some(advanced) = cursor {
                            extra_cursors.push(advanced);
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

        // A soft (non-auth) gather error means the delta/reconcile may be incomplete — flag it so
        // phase 2 holds this account's cursor (F-29). Auth failures are carried separately.
        let gather_failed = last_err.is_some() && !auth_failed;
        Ok((
            AccountWork {
                email,
                token_key,
                items,
                new_cursor,
                extra_cursors,
                auth_failed,
                coverage_incomplete,
                gather_failed,
            },
            last_err,
        ))
    }

    fn resolve_item<'a>(
        &self,
        app: &AppHandle,
        email: &str,
        item: &'a DriveItem,
    ) -> Result<Resolved<'a, drive::DriveFile>> {
        Ok(match item {
            DriveItem::Enumerated(file) => {
                let sid = drive::source_id_for(email, &file.id);
                let ev = index_only::ChangeEvent::Add {
                    source_id: sid.clone(),
                    modified_at: file.modified_time.clone(),
                };
                Resolved::Process {
                    file: Some(file),
                    source_id: sid,
                    event: ev,
                }
            }
            DriveItem::Changed(change) => {
                let sid = drive::source_id_for(email, &change.file_id);
                let known = {
                    let state = app.state::<AppState>();
                    let conn = state.conn()?;
                    drive::read_item_state(&conn, &sid)?.is_some()
                };
                match drive::map_change(change, email, known) {
                    Some(ev) => Resolved::Process {
                        file: change.file.as_ref(),
                        source_id: sid,
                        event: ev,
                    },
                    None => Resolved::Skip,
                }
            }
            // Shared-drive reconcile: the event (Add/Update/Delete) is already built.
            DriveItem::Reconciled {
                source_id,
                event,
                file,
            } => Resolved::Process {
                file: file.as_ref(),
                source_id: source_id.clone(),
                event: event.clone(),
            },
        })
    }

    async fn fetch_body(
        state: &AppState,
        token_key: &str,
        file: &drive::DriveFile,
    ) -> Result<Option<String>> {
        drive::fetch_body(state, token_key, file).await
    }

    async fn make_pointer(
        &self,
        token_key: &str,
        file: &drive::DriveFile,
        source_id: String,
        body: String,
        cache: &mut std::collections::HashMap<String, Option<String>>,
    ) -> index_only::PointerInput {
        // Snapshot the parent-folder name (cached per pass) alongside the body — plain review context
        // for the sorting proposal, resolved off the file's first `parents` entry.
        let folder_name = match file.parent_id.as_deref() {
            Some(pid) => resolve_folder_name(token_key, pid, cache).await,
            None => None,
        };
        file.pointer(source_id, body, folder_name)
    }
}

// --- OneDrive driver -----------------------------------------------------------------------------

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

/// Folder-scoped reconcile for OneDrive: enumerate the selected folders live and diff against the
/// account's currently-healthy items. Present+known → `Update` (catches edits); present+new/missing →
/// `Add` (ingests, or reactivates a folder removed then re-added); known-but-absent → `Delete`. Reads
/// the known set under a brief lock; the enumeration itself is off the lock.
async fn gather_onedrive_folders(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    folders: &[String],
) -> Result<(Vec<OneDriveItem>, bool)> {
    let (items, truncated) = onedrive::enumerate_folders(token_key, folders).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        onedrive::known_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    // A truncated enumeration (`complete = false`) must not infer deletions (F-30).
    let work = index_only::reconcile_enumeration(items, known, !truncated, |id| {
        onedrive::source_id_for(email, id)
    })
    .into_iter()
    .map(|r| OneDriveItem::Reconciled {
        source_id: r.source_id,
        event: r.event,
        item: r.payload,
    })
    .collect();
    Ok((work, truncated))
}

/// The OneDrive connector's [`CloudDriver`] (the Microsoft sibling of [`DriveDriver`]). The whole drive
/// uses the efficient Graph delta cursor (first sync enumerates everything, later syncs apply only
/// changes); a folder-scoped account is re-enumerated + reconciled each pass. No shared-drive concept,
/// so `extra_cursors` stays empty.
struct OneDriveDriver;

impl CloudDriver for OneDriveDriver {
    type File = onedrive::DriveItem;
    type Item = OneDriveItem;
    type FolderCache = ();

    const PENDING_KEY: &'static str = ONEDRIVE_SYNC_PENDING_KEY;
    const EVENT_NAME: &'static str = "onedrive://sync";
    const SOURCE_KIND: &'static str = "onedrive";
    const PROVIDER_LABEL: &'static str = "OneDrive";

    fn snapshot(state: &AppState) -> &Mutex<crate::CloudSyncState> {
        &state.onedrive_sync
    }
    fn cancel_flag(state: &AppState) -> &AtomicBool {
        &state.onedrive_sync_cancel
    }

    fn account_emails(conn: &Connection) -> Result<Vec<String>> {
        Ok(onedrive::list_accounts(conn)?
            .into_iter()
            .map(|a| a.email)
            .collect())
    }
    fn read_item_state(
        conn: &Connection,
        source_id: &str,
    ) -> Result<Option<index_only::ItemState>> {
        onedrive::read_item_state(conn, source_id)
    }
    fn set_error_state(conn: &Connection, email: &str) -> Result<()> {
        onedrive::set_state(conn, email, "error")
    }
    fn finalize_or_flag(
        conn: &Connection,
        work: &AccountWork<OneDriveItem>,
        account_failed: bool,
    ) -> Result<()> {
        onedrive::finalize_or_flag(
            conn,
            &work.email,
            account_failed,
            work.new_cursor.as_deref(),
        )
    }
    fn file_name(file: &onedrive::DriveItem) -> String {
        file.name.clone()
    }

    async fn gather_account(
        &self,
        app: &AppHandle,
        email: String,
    ) -> Result<(AccountWork<OneDriveItem>, Option<Error>)> {
        let token_key = onedrive::account_token_key(&email);
        let scope = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            onedrive::get_scope(&conn, &email)?
        };

        let mut items: Vec<OneDriveItem> = Vec::new();
        let mut new_cursor: Option<String> = None;
        let mut auth_failed = false;
        let mut coverage_incomplete = false;
        let mut last_err: Option<Error> = None;

        match scope.folders.as_deref() {
            // Folder-scoped: re-enumerate selected folders + reconcile (no cursor).
            Some(folders) => {
                match gather_onedrive_folders(app, &token_key, &email, folders).await {
                    Ok((mut recon, truncated)) => {
                        items.append(&mut recon);
                        coverage_incomplete |= truncated;
                    }
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
                let outcome: Result<(Vec<OneDriveItem>, String, bool)> = match &cursor {
                    None => onedrive::list_delta(&token_key, None).await.map(
                        |(deltas, link, truncated)| {
                            (
                                deltas
                                    .into_iter()
                                    .filter_map(|d| d.file)
                                    .map(OneDriveItem::Enumerated)
                                    .collect(),
                                link,
                                truncated,
                            )
                        },
                    ),
                    Some(link) => match onedrive::list_delta(&token_key, Some(link)).await {
                        Ok((deltas, link, truncated)) => Ok((
                            deltas.into_iter().map(OneDriveItem::Delta).collect(),
                            link,
                            truncated,
                        )),
                        Err(e) if onedrive::is_cursor_expired(&e) => {
                            onedrive::list_delta(&token_key, None).await.map(
                                |(deltas, link, truncated)| {
                                    (
                                        deltas
                                            .into_iter()
                                            .filter_map(|d| d.file)
                                            .map(OneDriveItem::Enumerated)
                                            .collect(),
                                        link,
                                        truncated,
                                    )
                                },
                            )
                        }
                        Err(e) => Err(e),
                    },
                };
                match outcome {
                    Ok((mut its, link, truncated)) => {
                        items.append(&mut its);
                        // The delta cursor is always advanced — even a truncated walk hands back a
                        // resumable `@odata.nextLink`, so the next sync continues rather than re-fetching
                        // the same head forever. The feed's removes are explicit, so there's no
                        // absence-inferred deletion to guard against; a truncated pass is just flagged
                        // as still catching up (F-30 withholds only for enumerations).
                        new_cursor = Some(link);
                        coverage_incomplete |= truncated;
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

        // A soft (non-auth) gather error means the delta/reconcile may be incomplete — flag it so
        // phase 2 holds this account's cursor (F-29). Auth failures are carried separately.
        let gather_failed = last_err.is_some() && !auth_failed;
        Ok((
            AccountWork {
                email,
                token_key,
                items,
                new_cursor,
                extra_cursors: Vec::new(),
                auth_failed,
                coverage_incomplete,
                gather_failed,
            },
            last_err,
        ))
    }

    fn resolve_item<'a>(
        &self,
        app: &AppHandle,
        email: &str,
        item: &'a OneDriveItem,
    ) -> Result<Resolved<'a, onedrive::DriveItem>> {
        Ok(match item {
            OneDriveItem::Enumerated(it) => {
                let sid = onedrive::source_id_for(email, &it.id);
                let ev = index_only::ChangeEvent::Add {
                    source_id: sid.clone(),
                    modified_at: it.modified_time.clone(),
                };
                Resolved::Process {
                    file: Some(it),
                    source_id: sid,
                    event: ev,
                }
            }
            OneDriveItem::Delta(delta) => {
                let sid = onedrive::source_id_for(email, &delta.item_id);
                let known = {
                    let state = app.state::<AppState>();
                    let conn = state.conn()?;
                    onedrive::read_item_state(&conn, &sid)?.is_some()
                };
                match onedrive::map_change(delta, email, known) {
                    Some(ev) => Resolved::Process {
                        file: delta.file.as_ref(),
                        source_id: sid,
                        event: ev,
                    },
                    None => Resolved::Skip,
                }
            }
            OneDriveItem::Reconciled {
                source_id,
                event,
                item,
            } => Resolved::Process {
                file: item.as_ref(),
                source_id: source_id.clone(),
                event: event.clone(),
            },
        })
    }

    async fn fetch_body(
        state: &AppState,
        token_key: &str,
        file: &onedrive::DriveItem,
    ) -> Result<Option<String>> {
        onedrive::fetch_body(state, token_key, file).await
    }

    async fn make_pointer(
        &self,
        _token_key: &str,
        file: &onedrive::DriveItem,
        source_id: String,
        body: String,
        _cache: &mut (),
    ) -> index_only::PointerInput {
        file.pointer(source_id, body)
    }
}

// --- thin entry points the IPC-layer command wrappers call ---------------------------------------

/// The sync engine behind [`crate::commands::sync_drive`] / [`crate::commands::resume_drive_sync`].
pub(crate) async fn drive_sync_core(app: &AppHandle, account: Option<String>) -> Result<usize> {
    run_cloud_sync(app, DriveDriver, account).await
}

/// The sync engine behind [`crate::commands::sync_onedrive`] / [`crate::commands::resume_onedrive_sync`].
pub(crate) async fn onedrive_sync_core(app: &AppHandle, account: Option<String>) -> Result<usize> {
    run_cloud_sync(app, OneDriveDriver, account).await
}
