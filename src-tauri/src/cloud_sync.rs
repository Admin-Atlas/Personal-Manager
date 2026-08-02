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

use std::path::{Path, PathBuf};
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
    /// The total number of files/changes this PASS will work through (sent once, before its items),
    /// and which account it is for (`None` = every account). A run can span several passes — each
    /// request folded in mid-run gets its own — so this is also how the UI learns that the queued
    /// account it is showing as "Queued" has come up: see `useDetachedSync`.
    Counted {
        total: usize,
        target: Option<String>,
    },
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
trait CloudDriver: Send + Sync + Clone {
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

    /// The driver for ONE pass of a run. A run spans several passes whenever requests arrive mid-sync
    /// (each queued target gets its own sweep), and those requests are not recorded individually — so
    /// `rerun` is all a pass knows about why it exists. A connector that varies what a pass covers
    /// decides here; the default keeps the run's driver unchanged, and only Drive overrides it.
    fn for_pass(&self, _rerun: bool) -> Self {
        self.clone()
    }

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

    /// Whether a body-fetch error is a per-ITEM access failure (the item was revoked/removed/locked
    /// for this user — a "Shared with me" grant that was pulled, a malware-flagged or Personal-Vault
    /// item) rather than an account-wide or transient failure. Lets phase 2 skip that one file (record
    /// an issue, keep the account healthy) instead of failing the whole account and holding its cursor
    /// forever (M2). Both providers now override it; the `false` default is the safe fallback for a
    /// connector that hasn't classified its errors yet, not the expected answer.
    fn is_item_gone(_err: &Error) -> bool {
        false
    }

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
        CloudSyncEvent::Counted { total, .. } => {
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

/// Fold one pass's report into the run's running total, without emitting anything.
///
/// A run is one or more passes (a request arriving mid-sync is folded into a follow-up sweep), and
/// the user thinks in runs. Reporting only the final pass would be actively wrong: the sweep that
/// follows a 50-document pass typically finds nothing new, so the run would end by announcing "0
/// indexed". `cancelled` takes the latest pass's value — whether the run ended by stopping is a
/// property of how it ended, not a sum.
fn accumulate_pass_report<C: CloudDriver>(app: &AppHandle, report: CloudSyncReport) {
    with_cloud_snap::<C>(app, |snap| match snap.last_report.as_mut() {
        None => snap.last_report = Some(report),
        Some(run) => merge_pass_into_run(run, report),
    });
}

/// Add one pass's counts to the run's. Pure, so the run-total arithmetic is testable without an app
/// handle: `cancelled` takes the latest pass's value (how the run ENDED, not a sum), and the issue
/// list stays bounded by the same cap one pass uses, with truncation sticky once either side hit it.
fn merge_pass_into_run(run: &mut CloudSyncReport, pass: CloudSyncReport) {
    run.indexed += pass.indexed;
    run.updated += pass.updated;
    run.removed += pass.removed;
    run.skipped += pass.skipped;
    run.failed += pass.failed;
    run.cancelled = pass.cancelled;
    let room = connector_sync::MAX_REPORT_ISSUES.saturating_sub(run.issues.len());
    if pass.issues.len() > room {
        run.issues_truncated = true;
    }
    run.issues.extend(pass.issues.into_iter().take(room));
    run.issues_truncated |= pass.issues_truncated;
}

/// Emit the run's single terminal event, from the totals accumulated across its passes. Called once
/// by [`connector_sync::run_detached_sync`] after the final pass, on every exit path.
fn emit_run_finished<C: CloudDriver>(app: &AppHandle) {
    let report = {
        let state = app.state::<AppState>();
        let guard = C::snapshot(state.inner()).lock();
        guard.ok().and_then(|snap| snap.last_report.clone())
    };
    // A run that never completed a pass (an early bail) has nothing accumulated; still terminate the
    // UI's run rather than leaving a bar up forever.
    let _ = app.emit(
        C::EVENT_NAME,
        CloudSyncEvent::Finished {
            report: report.unwrap_or_default(),
        },
    );
}

/// True if the running sync has been asked to stop.
fn sync_cancelled<C: CloudDriver>(app: &AppHandle) -> bool {
    C::cancel_flag(app.state::<AppState>().inner()).load(Ordering::SeqCst)
}

/// How one item's phase-2 processing ended.
///
/// Every arm is a decision the engine already made inline; naming them is what lets the accounting
/// be checked without an `AppHandle`, a network round-trip, or a store — the three reasons the
/// shared engine had no test of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemOutcome {
    /// The change mapped to nothing we track (a trashed-then-gone id that was never indexed).
    Unmapped,
    /// A body was needed and the fetch produced no extractable text.
    NoText,
    /// The body fetch failed. `permanent` marks a failure that will recur identically on every future
    /// pass — a per-ITEM revoke/removal/lock, or a body this build can never index (over a cap, or one
    /// the document engine refuses) — as opposed to a transient, auth, or network failure. Named for
    /// the CONSEQUENCE rather than the cause ("gone" is a lie for an over-cap file), because the whole
    /// decision it drives is "replay this next pass, or step past it".
    FetchFailed { permanent: bool },
    /// The reducer's actions applied cleanly, landing in this category.
    Applied(connector_sync::ActionKind),
    /// The apply itself failed.
    ApplyFailed,
}

/// What an outcome does to the pass: which counter it lands in, whether it records a user-visible
/// issue, and whether it fails the account.
///
/// `fails_account` is the consequential one. An account with any failed item is finalized `error`
/// with its delta cursor left UNADVANCED, so the failure retries next pass instead of hiding behind
/// a misleading `ok` that has already stepped past the change it never applied (F-29). Getting that
/// wrong cuts both ways: M2 fixed one revoked shared file failing the whole account, and the same
/// arm's `permanent` flag is what stops a file that can NEVER be fetched holding the cursor forever.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ItemEffect {
    indexed: usize,
    updated: usize,
    removed: usize,
    skipped: usize,
    failed: usize,
    records_issue: bool,
    fails_account: bool,
}

/// The pure accounting rule for one item. Every outcome lands in exactly one counter.
fn item_effect(outcome: ItemOutcome) -> ItemEffect {
    match outcome {
        // Nothing to do and nothing to tell the user about.
        ItemOutcome::Unmapped => ItemEffect {
            skipped: 1,
            ..ItemEffect::default()
        },
        // We reached the file and it held nothing indexable — worth saying, not worth failing over.
        ItemOutcome::NoText => ItemEffect {
            skipped: 1,
            records_issue: true,
            ..ItemEffect::default()
        },
        // The account survives a file that can never be fetched (gone for this user, or unindexable
        // by this build) — retrying it forever is what pins the account, not what heals it. Anything
        // else holds the cursor so the pass retries rather than stepping past what it never saw.
        ItemOutcome::FetchFailed { permanent } => ItemEffect {
            skipped: usize::from(permanent),
            failed: usize::from(!permanent),
            records_issue: true,
            fails_account: !permanent,
            ..ItemEffect::default()
        },
        ItemOutcome::Applied(kind) => ItemEffect {
            indexed: usize::from(kind == connector_sync::ActionKind::Indexed),
            updated: usize::from(kind == connector_sync::ActionKind::Updated),
            removed: usize::from(kind == connector_sync::ActionKind::Removed),
            skipped: usize::from(kind == connector_sync::ActionKind::Other),
            ..ItemEffect::default()
        },
        ItemOutcome::ApplyFailed => ItemEffect {
            failed: 1,
            records_issue: true,
            fails_account: true,
            ..ItemEffect::default()
        },
    }
}

/// The pass's running totals, so [`item_effect`]'s verdict is applied in one place rather than
/// re-spelled at each of the five sites an item can end at.
#[derive(Default)]
struct PassCounts {
    indexed: usize,
    updated: usize,
    removed: usize,
    skipped: usize,
    failed: usize,
}

impl PassCounts {
    fn add(&mut self, eff: ItemEffect) {
        self.indexed += eff.indexed;
        self.updated += eff.updated;
        self.removed += eff.removed;
        self.skipped += eff.skipped;
        self.failed += eff.failed;
    }
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

// --- shared fetch-body helpers (the connectors' byte-identical tails, hoisted like the types above) --

/// The trimmed text, or `None` when it's empty — both connectors' "did the fetch/convert yield
/// anything indexable?" filter.
pub(crate) fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Stage downloaded bytes to a uniquely-named temp file tagged with the source's extension, so the
/// sidecar (MarkItDown) picks the right converter; removed by the caller after use. `prefix` keeps
/// each connector's temp files recognisable (`pm-drive-` / `pm-onedrive-`).
///
/// The name is **random per call** (it used to be content-addressed): a deterministic name let a
/// temp left behind by a crashed pass — or one Windows Defender was still scanning right after the
/// write — collide with the sidecar's `open(path, "rb")`, surfacing as `PermissionError [Errno 13]`
/// and failing that one document (#403). A fresh name can't collide, and the write handle is closed
/// before the path is handed on, so our own handle never blocks the reader either.
pub(crate) fn stage_temp(prefix: &str, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 8)
        .unwrap_or("bin");
    let mut file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(&format!(".{ext}"))
        .tempfile()
        .map_err(|e| Error::Other(format!("staging temp file: {e}")))?;
    std::io::Write::write_all(&mut file, bytes)?;
    std::io::Write::flush(&mut file)?;
    // Close our writer handle but keep the file on disk (the caller removes it after conversion);
    // on Windows an open writer would deny the sidecar's read `open`.
    file.into_temp_path()
        .keep()
        .map_err(|e| Error::Other(format!("staging temp file: {e}")))
}

/// The shared tail of both connectors' `FetchPlan::DownloadBinary` arms: take the download's
/// outcome, stage the bytes to a temp file, convert via the sidecar, remove the temp, and return
/// the non-empty markdown. Never holds the DB lock.
pub(crate) fn convert_downloaded_binary(
    state: &AppState,
    temp_prefix: &str,
    name: &str,
    downloaded: Result<Vec<u8>>,
) -> Result<Option<String>> {
    let bytes = match downloaded {
        Ok(b) => b,
        // An over-cap download is a skip (kept findable via its title), not a hard error.
        Err(e) if e.to_string().contains("too large") => return Ok(None),
        Err(e) => return Err(e),
    };
    let tmp = stage_temp(temp_prefix, name, &bytes)?;
    let converted = convert_staged(state, &tmp);
    let _ = std::fs::remove_file(&tmp);
    let (markdown, _title) = converted?;
    Ok(non_empty(&markdown))
}

/// Convert a staged temp file via the sidecar, retrying briefly on a transient Windows file lock.
/// Once the file is written, antivirus (Defender) opens it to scan, and during that window the
/// sidecar's `open(path, "rb")` is denied with `PermissionError [Errno 13]` (#403) — a lock that
/// clears in well under a second. A genuine conversion failure (unsupported/corrupt input) doesn't
/// match the lock signature and surfaces on the first try. Runs off the DB lock, like its caller.
fn convert_staged(state: &AppState, path: &Path) -> Result<(String, String)> {
    const MAX_ATTEMPTS: u32 = 4;
    let mut attempt = 1u32;
    loop {
        match state.sidecar.convert(path) {
            Ok(out) => return Ok(out),
            Err(e) if attempt < MAX_ATTEMPTS && is_file_lock_error(&e) => {
                // 160 / 320 / 640 ms — bounded (~1.1s total) so a genuinely stuck lock still fails fast.
                std::thread::sleep(std::time::Duration::from_millis(80 * (1u64 << attempt)));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Whether an error looks like a transient file lock worth retrying (the #403 Windows failure:
/// antivirus or another handle briefly holding the just-written temp). Matched on the sidecar's
/// surfaced Python/OS text — `PermissionError`/`Errno 13` (Python) and `os error 32`/"being used by
/// another process" (the Win32 sharing violation).
fn is_file_lock_error(e: &Error) -> bool {
    let s = e.to_string();
    s.contains("PermissionError")
        || s.contains("[Errno 13]")
        || s.contains("Permission denied")
        || s.contains("os error 32")
        || s.contains("being used by another process")
}

/// Whether a body fetch failed in a way that will fail identically on every future pass, so the item
/// must be SKIPPED rather than replayed forever. Provider-independent, and the reason it exists is the
/// cursor: a failure that fails the account holds its delta cursor at the last-good value (F-29), so a
/// permanently-unindexable file re-fails on every subsequent pass and the account never reads as
/// synced again. F-29 exists to prevent silent gaps; a permanent per-item failure is that same failure
/// inverted, so the rule is narrowed here rather than widened.
///
/// Three causes, all terminal: PM's own download cap (`google.rs`/`microsoft.rs` `read_capped*`), the
/// sidecar's input cap (`sidecar::check_input_size`), and the document engine ANSWERING and refusing
/// this file.
///
/// **Fail-closed on the third.** This used to match the bare `sidecar convert failed:` prefix, which
/// every convert error wears — including one raised by the engine being BROKEN. `do_convert` imports
/// markitdown lazily, so a half-installed venv answers `sidecar convert failed: No module named
/// 'markitdown'` for every file; that classified as permanently-unindexable, so each file was skipped,
/// the account stayed `ok`, and the delta cursor committed past changes PM had never indexed. The
/// changes feed never re-offers them, so the only recovery was disconnect-and-re-add. The prefix could
/// not distinguish the two cases because Python reports `str(exc)`, which carries no type: measured,
/// `str(UnsupportedFormatException("cannot handle .xyz"))` is exactly `cannot handle .xyz`.
///
/// So the verdict is now explicit: the sidecar raises `Unconvertible` for its own caps and for
/// markitdown's `UnsupportedFormatException` / `FileConversionException` only, the main loop tags the
/// reply `error_kind: "unconvertible"`, and `SidecarManager::request` renders that as
/// `sidecar convert failed [unconvertible]:`. Anything untagged — an ImportError, a
/// `MissingDependencyException` (the engine is incomplete; a repair fixes it), an OS error,
/// `sidecar convert IO error:`, "not installed" — is the engine being broken and stays account-fatal,
/// holding the cursor so the files are re-offered once it is repaired. A transient antivirus file lock
/// is still checked FIRST: [`convert_staged`] has already retried it, and it arrives untagged anyway.
fn is_permanently_unindexable(err: &Error) -> bool {
    if is_file_lock_error(err) {
        return false;
    }
    let s = err.to_string();
    s.contains("too large to index")
        || s.contains("too large to process")
        || s.contains("sidecar convert failed [unconvertible]:")
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
        |req: connector_sync::PassRequest| {
            // The driver is rebuilt per PASS, not per run: a follow-up sweep answers a request that
            // was folded into this run without being recorded, so what it should cover can differ
            // from what the run's first pass covered (see `DriveDriver::for_pass`).
            let pass_driver = driver.for_pass(req.rerun);
            async move { run_cloud_pass(app, &pass_driver, req.target).await }
        },
        || emit_run_finished::<C>(app),
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
        match &account {
            Some(e) => vec![e.clone()],
            None => C::account_emails(&conn)?,
        }
    };

    let mut work: Vec<AccountWork<C::Item>> = Vec::new();
    let mut last_err: Option<Error> = None;

    // Phase 1 — gather each account's work off the lock. The driver owns the provider-specific shape
    // (My Drive + shared drives, or a single OneDrive) and reports any soft, per-account error to fold
    // into `last_err`; hard errors propagate via `?`.
    for email in emails {
        // Stop requested before this account's gather? Halt phase 1 — phase 2's first check reads the
        // same flag and reports the pass cancelled, and an ungathered account is simply left untouched
        // (cursor unadvanced, no inferred deletions). A walk ALREADY in flight also stops now rather
        // than running the account to completion: each driver hands the same flag down its listings as
        // a [`connector_sync::Cancel`] probe (#699), where a trip returns what was gathered flagged
        // truncated — which is what keeps a half-listed account from being read as a deletion.
        if sync_cancelled::<C>(app) {
            break;
        }
        let (w, soft_err) = driver.gather_account(app, email).await?;
        if let Some(e) = soft_err {
            last_err = Some(e);
        }
        work.push(w);
    }

    let total: usize = work.iter().map(|w| w.items.len()).sum();
    emit_progress::<C>(
        app,
        CloudSyncEvent::Counted {
            total,
            target: account,
        },
    );

    // Phase 2 — process each item: react → fetch body only when needed → apply (embed off the lock).
    let mut counts = PassCounts::default();
    let mut processed = 0usize;
    // Files we attempted but couldn't index (unsupported/empty, or a fetch error), for the report.
    let mut issues: Vec<CloudSyncIssue> = Vec::new();
    let mut issues_truncated = false;
    // Set if the user pressed Stop. Already-applied items stay committed; we stop early and skip the
    // interrupted account's cursor advance, so the next sync re-checks it.
    let mut cancelled = false;
    // Per-pass driver state (Drive's parent-folder-name memo; `()` for OneDrive).
    let mut folder_cache = C::FolderCache::default();
    // Batch the encrypted manifest rewrite across the pass: `apply_connector_actions` now only commits
    // DB rows, and we flush the manifest every MANIFEST_FLUSH_EVERY items + once after the loop, instead
    // of once per item (which was O(n²) over a pass). A mid-pass bail is caught by the flusher's Drop.
    let mut manifest_flush = connector_sync::ManifestFlusher::new(app);

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
            if let Ok(Ok(dirtied)) = tokio::task::spawn_blocking(move || {
                connector_sync::apply_connector_actions(&app2, &actions, None)
            })
            .await
            {
                manifest_flush.note(dirtied)?;
            }
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            C::set_error_state(&conn, &w.email)?;
            continue;
        }

        // A gather that couldn't see the whole account — a page/folder guard, or a subtree it wasn't
        // allowed to open — already withheld the enumeration's cursor advance and skipped inferred
        // deletions; surface it so the partial pass isn't read as a clean one (F-30). The note stays
        // cause-agnostic because the user's next step is the same for all of them: wait a sync.
        // Pushed directly, NOT through the per-file `record_issue` (which is capped): there's at most one
        // of these per account and it's the only report-side signal a pass was partial, so it must never
        // be starved by a full per-file issues list.
        if w.coverage_incomplete {
            issues.push(CloudSyncIssue {
                name: w.email.clone(),
                reason: "Only part of this account could be listed this sync. Nothing was \
                         removed; the rest is picked up on the next sync."
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
                    counts.add(item_effect(ItemOutcome::Unmapped));
                    continue;
                }
                Resolved::Process {
                    file,
                    source_id,
                    event,
                } => (file, source_id, event),
            };

            let name = file.map(C::file_name).unwrap_or_else(|| source_id.clone());
            // Read the item's state HERE, not during phase 1. Gathers run in sequence and can re-key
            // rows out from under each other (`drive::adopt_legacy_swm_row`), so a plan built by an
            // earlier gather is only judged against the store as it stands now — see
            // [`baseline_my_drive`]'s note on legacy shared-with-me rows.
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
                        counts.add(item_effect(ItemOutcome::NoText));
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
                        record_issue(
                            &mut issues,
                            &mut issues_truncated,
                            &name,
                            &format!("Couldn't fetch from {}: {e}", C::PROVIDER_LABEL),
                        );
                        // A failure this file will hit again on every future pass — a per-item
                        // revoke/removal/lock, or a body PM can never index — skips just this file
                        // (recorded as an issue, account stays healthy) instead of failing the whole
                        // account. Holding the cursor for it would replay the same poison item every
                        // pass and the account would never read as synced again. Any other error
                        // (transient/auth/network) still fails the account so its cursor is held and it
                        // retries next pass (F-29, M2).
                        let eff = item_effect(ItemOutcome::FetchFailed {
                            permanent: C::is_item_gone(&e) || is_permanently_unindexable(&e),
                        });
                        counts.add(eff);
                        if eff.fails_account {
                            account_failed = true;
                            last_err = Some(e);
                        }
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
                Ok(dirtied) => {
                    manifest_flush.note(dirtied)?;
                    counts.add(item_effect(ItemOutcome::Applied(category)));
                }
                Err(e) => {
                    counts.add(item_effect(ItemOutcome::ApplyFailed));
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

    // Persist the tail of the batched manifest — reached on a normal finish AND on a Stop that broke the
    // loop. A hard `?` bail earlier is covered by the flusher's Drop (bounded to < MANIFEST_FLUSH_EVERY).
    manifest_flush.flush()?;

    let report = CloudSyncReport {
        indexed: counts.indexed,
        updated: counts.updated,
        removed: counts.removed,
        skipped: counts.skipped,
        failed: counts.failed,
        cancelled,
        issues,
        issues_truncated,
    };
    // Accumulate, don't announce: this is the end of a PASS, and a run may still have a folded-in
    // sweep to go. `run_detached_sync` emits the one terminal event once the run is actually over.
    accumulate_pass_report::<C>(app, report);

    // A deliberate stop isn't an error. Otherwise surface a failure (auth/expired) even when some
    // items succeeded — the good ones are already committed.
    if !cancelled {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(counts.indexed + counts.updated + counts.removed)
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
    /// A My-Drive changes-feed entry → mapped via `map_change`.
    Changed(drive::DriveChange),
    /// An enumeration reconciled against what is already indexed, with its event pre-built: `Add` for a
    /// new/reactivating file (reducer: unknown→ingest, missing/unreachable→reachable), `Update` for a
    /// present healthy file (same-hash→noop, changed→re-embed), or `Delete` for a file that vanished
    /// from the enumeration. Every listing that returns a whole corpus lands here — folder-scoped, a
    /// shared-with-me root, and both whole-drive re-baselines (see [`drive_baseline_items`]).
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

/// Plan one Drive enumeration against what is already indexed under the same namespace, and map the
/// plan onto this connector's work items. **Every** Drive listing that hands back a whole corpus goes
/// through here — a folder-scoped reconcile, a shared-with-me root, and (since this) the two
/// whole-drive re-baselines — so "absent means deleted, unless the listing was cut short" has one
/// implementation rather than one per call site.
///
/// The re-baselines used to hand-build an `Add` per enumerated file instead, which silently lost both
/// halves of the gap the re-baseline exists to close: `react(Add, Some(ok))` is a `Noop`, so a file
/// deleted while the cursor was dead stayed `source_state = 'ok'` forever (I-09.2), and an *edit*
/// during the same gap was never compared either (an `Add` carries no content hash).
///
/// `truncated` is the listing's own report that it did not see everything; it is inverted into
/// [`index_only::reconcile_enumeration`]'s `complete`, which withholds the whole deletion pass — a
/// file we simply didn't reach must never be read as absent (F-30).
fn drive_baseline_items(
    files: Vec<drive::DriveFile>,
    known: std::collections::HashSet<String>,
    truncated: bool,
    source_id_of: impl Fn(&str) -> String,
) -> Vec<DriveItem> {
    index_only::reconcile_enumeration(files, known, !truncated, source_id_of)
        .into_iter()
        .map(drive_reconciled)
        .collect()
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
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<DriveItem>, Option<(String, String)>, bool)> {
    match sel.folders.as_deref() {
        Some(folders) => {
            let (items, truncated) = gather_shared_folders(
                app,
                token_key,
                &sel.drive_id,
                folders,
                &sel.exclude,
                sel.include_root_files,
                cancel,
            )
            .await?;
            Ok((items, None, truncated))
        }
        None => gather_shared_whole(app, token_key, email, &sel.drive_id, cancel).await,
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
    exclude: &[String],
    include_root_files: bool,
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<DriveItem>, bool)> {
    let (mut files, mut truncated) =
        drive::enumerate_shared(token_key, drive_id, Some(folders), exclude, cancel).await?;
    // Fix 4: also index files loose in the drive's root when opted in — a file has one parent, so a
    // root file never overlaps a folder-walked file (no dedup needed). Reconciled against the whole
    // drive's known set below, so toggling this off soft-removes the root files like unselecting a folder.
    if include_root_files {
        let (root_files, rt) = drive::enumerate_root_files(token_key, drive_id, cancel).await?;
        files.extend(root_files);
        truncated |= rt;
    }
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_shared_source_ids(&conn, drive_id)?
            .into_iter()
            .collect()
    };
    let items = drive_baseline_items(files, known, truncated, |file_id| {
        drive::shared_source_id(drive_id, file_id)
    });
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
    exclude: &[String],
    include_root_files: bool,
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<DriveItem>, bool)> {
    let (mut files, mut truncated) =
        drive::enumerate_my_folders(token_key, folders, exclude, cancel).await?;
    // Fix 4: also index files loose in My Drive's root when opted in (see [`gather_shared_folders`]).
    if include_root_files {
        let (root_files, rt) =
            drive::enumerate_root_files(token_key, drive::MY_DRIVE_ROOT, cancel).await?;
        files.extend(root_files);
        truncated |= rt;
    }
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_my_drive_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    let items = drive_baseline_items(files, known, truncated, |file_id| {
        drive::source_id_for(email, file_id)
    });
    Ok((items, truncated))
}

/// Re-baseline **the whole of My Drive**: enumerate every file, take a fresh changes-feed start token,
/// and reconcile the enumeration against the account's currently-healthy My-Drive items. Reached on a
/// first sync, on a 410 cursor reset, and whenever `drive::set_scope` pruned the cursor on the way back
/// from folder-scoped to whole-drive — the last two run against a fully-populated known set, which is
/// exactly when a file that vanished (or was edited) while the cursor was dead has to be noticed.
///
/// The returned cursor is the one to PERSIST: withheld (`None`) when the enumeration was truncated,
/// because a partial re-list can't be baselined — the next sync re-enumerates from scratch (F-30).
///
/// Shared-with-me files are deliberately out of both sides of the diff: `enumerate_drive` filters
/// `sharedWithMe = false` and `known_my_drive_source_ids` matches only the `gdrive:<email>:` prefix,
/// while that corpus lives under `gdrive:swm:<rootId>:` and reconciles per root in
/// [`gather_shared_with_me`]. Do NOT union the two known sets.
///
/// One residue that leaves: a LEGACY shared-with-me row still keyed `gdrive:<email>:<fileId>` (indexed
/// before the swm namespace landed) is in the known set but can never be in this enumeration, so it
/// plans as a `Delete`. That is absorbed rather than acted on, because `gather_shared_with_me` runs
/// later in the SAME phase-1 gather and `drive::adopt_legacy_swm_row` re-keys the row before phase 2
/// evaluates the plan — at which point phase 2's per-item `read_item_state(old_id)` is `None` and the
/// reducer no-ops. That ordering is therefore load-bearing: pre-resolving item state during phase 1, or
/// gathering shared-with-me before My Drive, would turn it into a real (soft, self-healing) flag.
async fn baseline_my_drive(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<DriveItem>, Option<String>, bool)> {
    let (files, truncated) = drive::enumerate_drive(token_key, cancel).await?;
    let cursor = drive::start_page_token(token_key, None).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::known_my_drive_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    let items = drive_baseline_items(files, known, truncated, |file_id| {
        drive::source_id_for(email, file_id)
    });
    Ok((items, (!truncated).then_some(cursor), truncated))
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
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<DriveItem>, Option<(String, String)>, bool)> {
    let cursor = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        drive::get_shared_cursor(&conn, email, drive_id)?
    };

    // First sync / 410 reset: the whole drive re-enumerated and reconciled against what is already
    // indexed for it + a fresh baseline cursor. Reconciled, not hand-built `Add`s — on a 410 reset the
    // known set is fully populated, so files deleted (or edited) while the cursor was dead are only
    // seen by diffing. Also reports whether the enumeration was truncated (⇒ don't baseline the cursor
    // yet — retry next pass; and `drive_baseline_items` withholds the deletions too).
    async fn baseline(
        app: &AppHandle,
        token_key: &str,
        drive_id: &str,
        cancel: connector_sync::Cancel<'_>,
    ) -> Result<(Vec<DriveItem>, String, bool)> {
        let (files, truncated) =
            drive::enumerate_shared(token_key, drive_id, None, &[], cancel).await?;
        let new_cursor = drive::start_page_token(token_key, Some(drive_id)).await?;
        let known: std::collections::HashSet<String> = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::known_shared_source_ids(&conn, drive_id)?
                .into_iter()
                .collect()
        };
        let items = drive_baseline_items(files, known, truncated, |file_id| {
            drive::shared_source_id(drive_id, file_id)
        });
        Ok((items, new_cursor, truncated))
    }

    // A truncated *baseline enumeration* indexes what it gathered but withholds the cursor (a partial
    // re-list can't be baselined), so the next sync re-enumerates. The delta branch below, by contrast,
    // advances even when truncated — its page token is resumable (F-30).
    let baseline_cursor =
        |truncated: bool, c: String| (!truncated).then(|| (drive_id.to_string(), c));

    let cursor = match cursor {
        None => {
            let (items, c, truncated) = baseline(app, token_key, drive_id, cancel).await?;
            return Ok((items, baseline_cursor(truncated, c), truncated));
        }
        Some(c) => c,
    };

    let (changes, new_cursor, truncated) =
        match drive::list_shared_changes(token_key, drive_id, &cursor, cancel).await {
            Ok(v) => v,
            Err(e) if drive::is_cursor_expired(&e) => {
                let (items, c, truncated) = baseline(app, token_key, drive_id, cancel).await?;
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

/// Gather the account's **Shared with me** work. Lists the picked shared roots (or all, when the scope
/// carries no explicit list), and for each root this account OWNS (first-come via
/// `claim_or_skip_swm_root`, so a root shared with two connected accounts is indexed once) enumerates
/// its files — a folder root walked recursively, a shortcut resolved to its target, a single file
/// indexed on its own — and reconciles them under the root's own account-independent
/// `gdrive:swm:<rootId>:` namespace (no cursor). Legacy leaked rows are adopted in place first
/// ([`drive::adopt_legacy_swm_row`]). Returns the items plus whether any enumeration was incomplete
/// (⇒ no reconcile deletions for the affected root).
async fn gather_shared_with_me(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    picked: Option<&[String]>,
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<DriveItem>, bool)> {
    let (roots, mut truncated) = drive::list_swm_roots(token_key, cancel).await?;
    let picked_set: Option<std::collections::HashSet<&str>> =
        picked.map(|ids| ids.iter().map(String::as_str).collect());

    let mut items: Vec<DriveItem> = Vec::new();
    for root in &roots {
        // Honour the pick list (`None` = every shared root).
        if picked_set
            .as_ref()
            .is_some_and(|s| !s.contains(root.id.as_str()))
        {
            continue;
        }
        // Claim ownership; a root already owned by another connected account is indexed there — skip.
        let owns = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            drive::claim_or_skip_swm_root(&conn, email, &root.id, &root.name)?
        };
        if !owns {
            continue;
        }
        let (files, root_truncated) = drive::enumerate_swm_root(token_key, root, cancel).await?;
        truncated |= root_truncated;
        // Adopt any legacy My-Drive-namespaced rows for these files, THEN read the root's known set —
        // so an adopted row is already in that set and reconciles as an Update/no-op, not a re-ingest.
        let (known, adopted): (std::collections::HashSet<String>, Vec<(String, String)>) = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            let mut adopted = Vec::new();
            for f in &files {
                if let Some(pair) = drive::adopt_legacy_swm_row(&conn, email, &root.id, &f.id)? {
                    adopted.push(pair);
                }
            }
            (
                drive::known_swm_source_ids(&conn, &root.id)?
                    .into_iter()
                    .collect(),
                adopted,
            )
        };
        // Carry each adoption into the encrypted manifest, off the DB guard. Without this the old
        // My-Drive-namespaced id survives in the portable truth (the mirror-∪-file union never drops
        // it), and the next Rebuild restores it as a SECOND document beside the re-keyed one — a
        // duplicate the user cannot tell from a real file. Best-effort: an adoption that already
        // committed to the DB must not fail the sync, and the next sync retries it.
        if !adopted.is_empty() {
            let state = app.state::<AppState>();
            match state.manifest_io() {
                Ok((vault_root, cipher)) => {
                    if let Err(e) = index_only::rekey_sources(&vault_root, &cipher, &adopted) {
                        eprintln!("drive: shared-with-me manifest re-key skipped ({e})");
                    }
                }
                Err(e) => eprintln!("drive: shared-with-me manifest re-key skipped ({e})"),
            }
        }
        let root_id = root.id.clone();
        let mut recon = drive_baseline_items(files, known, root_truncated, |file_id| {
            drive::swm_source_id(&root_id, file_id)
        });
        items.append(&mut recon);
    }

    // Heal roots whose share was REVOKED. A revoked root simply vanishes from `list_swm_roots`, so
    // the loop above never visits it, no reconcile runs for it, and its documents sat at
    // `source_state = 'ok'` with stale content forever — a later body fetch would just 403. Diff what
    // this account owns against what the listing actually returned and release the difference;
    // `release_swm_root` already drops the access row and soft-flags the items `unreachable` when no
    // other connected account can still reach them. Soft only: nothing is deleted, and re-sharing
    // restores it on the next sync.
    //
    // Gated on `!truncated` — the F-30 rule. A partial listing (a page guard trip, a tolerated
    // 403/404 mid-walk) is NOT evidence that the missing roots are gone, and acting on one would
    // mass-flag a perfectly healthy corpus.
    if !truncated {
        let live: std::collections::HashSet<&str> = roots.iter().map(|r| r.id.as_str()).collect();
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        for owned in drive::owned_swm_roots(&conn, email)? {
            // A root the user simply unpicked is handled by `set_scope`; only act on ones this
            // account still believes it indexes but Drive no longer offers at all.
            if !live.contains(owned.as_str()) {
                drive::release_swm_root(&conn, email, &owned)?;
            }
        }
    }

    Ok((items, truncated))
}

/// The Google Drive connector's [`CloudDriver`] — see [`crate::commands::sync_drive`] /
/// [`crate::commands::resume_drive_sync`]. Honours each account's scope: **My Drive** (on by default)
/// uses the efficient delta cursor (first sync enumerates everything, later syncs the changes feed);
/// **shared drives** the account opted into are re-enumerated + reconciled each pass (whole drive, or
/// selected folders), de-duplicated across accounts via `claim_or_skip_shared_drive`.
#[derive(Clone, Copy)]
struct DriveDriver {
    /// Whether this pass also re-walks "Shared with me".
    ///
    /// Every other Drive corpus rides a `changes.list` delta cursor: one cheap call that returns an
    /// empty page when nothing moved, so polling it often costs almost nothing. Shared-with-me has
    /// no cursor — Google exposes no delta for it — so each pass RE-ENUMERATES every picked root and
    /// reconciles it. That is fine on the sync button and too heavy to repeat every few minutes, so
    /// the background poller runs it on a much longer cadence and asks for it explicitly.
    ///
    /// A manual sync always passes true: someone who pressed the button wants everything looked at.
    include_shared_with_me: bool,
}

impl CloudDriver for DriveDriver {
    type File = drive::DriveFile;
    type Item = DriveItem;
    type FolderCache = std::collections::HashMap<String, Option<String>>;

    /// A follow-up sweep always re-walks Shared with me.
    ///
    /// Only the background poller ever opts out, and the requests folded into a running run are not
    /// recorded with the options they were made under — so a user's "Sync now" landing mid-poll would
    /// otherwise be answered by a sweep that silently skips the corpus they pressed the button for,
    /// with no sign anything was left out. Widening is a superset, so the poller's own folded-in
    /// request loses nothing by being answered more thoroughly, and this only costs the extra
    /// enumeration when a pass outlives the poll interval — which is exactly when it is warranted.
    fn for_pass(&self, rerun: bool) -> Self {
        Self {
            include_shared_with_me: self.include_shared_with_me || rerun,
        }
    }

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
    fn is_item_gone(err: &Error) -> bool {
        drive::is_item_forbidden_or_missing(err)
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
        // The same flag phase 1 and phase 2 read between accounts, handed DOWN into every listing so a
        // Stop lands inside the walk (#699). A trip returns what was gathered flagged truncated, which
        // withholds this account's cursor and its reconcile deletions — see [`drive_baseline_items`].
        let cancel = || sync_cancelled::<Self>(app);

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
                    match gather_my_drive_folders(
                        app,
                        &token_key,
                        &email,
                        folders,
                        &scope.my_drive_exclude,
                        scope.my_drive_include_root_files,
                        &cancel,
                    )
                    .await
                    {
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
                    // The 2nd element is the cursor to PERSIST: `None` = withhold, `Some` = advance. The
                    // changes feed advances even when truncated — its page token is a resumable
                    // checkpoint and the feed has no absence-inferred deletes, so it drains a big backlog
                    // across passes instead of re-fetching the same head forever. A re-baseline
                    // ([`baseline_my_drive`]) withholds on truncation instead, and reconciles rather
                    // than re-adding — the only way a change made while the cursor was dead is seen.
                    let outcome: Result<(Vec<DriveItem>, Option<String>, bool)> =
                        if cursor.is_none() {
                            baseline_my_drive(app, &token_key, &email, &cancel).await
                        } else {
                            match drive::list_changes(
                                &token_key,
                                cursor.as_deref().unwrap_or(""),
                                &cancel,
                            )
                            .await
                            {
                                Ok((changes, c, truncated)) => Ok((
                                    changes.into_iter().map(DriveItem::Changed).collect(),
                                    Some(c),
                                    truncated,
                                )),
                                Err(e) if drive::is_cursor_expired(&e) => {
                                    baseline_my_drive(app, &token_key, &email, &cancel).await
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
                match gather_shared(app, &token_key, &email, sel, &cancel).await {
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

        // --- Shared with me: files/folders granted directly to the account, indexed under their own
        // account-independent `gdrive:swm:<rootId>:` namespace and de-duplicated across accounts via
        // `claim_or_skip_swm_root` (the same ownership model as shared drives). Re-enumerated +
        // reconciled per picked root each pass, no cursor. Skipped if the account already auth-failed. ---
        if scope.shared_with_me && self.include_shared_with_me && !auth_failed {
            match gather_shared_with_me(
                app,
                &token_key,
                &email,
                scope.shared_with_me_roots.as_deref(),
                &cancel,
            )
            .await
            {
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
            // A reconciled enumeration: the event (Add/Update/Delete) is already built.
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
    /// A whole-drive incremental delta entry → mapped via `map_change` in phase 2.
    Delta(onedrive::DriveDelta),
    /// An enumeration reconciled against what is already indexed, with its event pre-built
    /// (`Add`/`Update`/`Delete`) — a folder-scoped listing or a whole-drive re-baseline
    /// (see [`onedrive_baseline_items`]).
    Reconciled {
        source_id: String,
        event: index_only::ChangeEvent,
        item: Option<onedrive::DriveItem>,
    },
}

/// Map a reconcile-plan entry ([`index_only::reconcile_enumeration`]) onto this connector's work item
/// — the enumerated item rides along so phase 2 can fetch a body on `Add`/`Update`. The OneDrive twin
/// of [`drive_reconciled`].
fn onedrive_reconciled(r: index_only::ReconcileItem<onedrive::DriveItem>) -> OneDriveItem {
    OneDriveItem::Reconciled {
        source_id: r.source_id,
        event: r.event,
        item: r.payload,
    }
}

/// Plan one OneDrive enumeration against what is already indexed for the account — the twin of
/// [`drive_baseline_items`], and for the same reason: the whole-drive re-baseline used to hand-build an
/// `Add` per enumerated item, and `react(Add, Some(ok))` is a `Noop`, so nothing that happened while
/// the delta cursor was dead (a deletion OR an edit) was ever noticed.
fn onedrive_baseline_items(
    files: Vec<onedrive::DriveItem>,
    known: std::collections::HashSet<String>,
    truncated: bool,
    source_id_of: impl Fn(&str) -> String,
) -> Vec<OneDriveItem> {
    index_only::reconcile_enumeration(files, known, !truncated, source_id_of)
        .into_iter()
        .map(onedrive_reconciled)
        .collect()
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
    exclude: &[String],
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<OneDriveItem>, bool)> {
    let (items, truncated) =
        onedrive::enumerate_folders(token_key, folders, exclude, cancel).await?;
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        onedrive::known_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    let work = onedrive_baseline_items(items, known, truncated, |id| {
        onedrive::source_id_for(email, id)
    });
    Ok((work, truncated))
}

/// Re-baseline **the whole drive**: the no-token `/root/delta` returns the full enumeration plus a
/// fresh delta link in one walk, and the enumeration is reconciled against the account's
/// currently-healthy items rather than replayed as `Add`s. Reached on a first sync and on a 410
/// `resyncRequired` — the second runs against a fully-populated known set, so a file deleted (or
/// edited) while the cursor was dead is only seen by diffing.
///
/// The returned link is always persisted, exactly as before: on a clean walk it is the true
/// `@odata.deltaLink`, and on a truncated one the last `@odata.nextLink`, itself resumable. Note the
/// consequence, which Drive does not share: a truncated re-baseline reconciles with `complete = false`
/// (no deletions) and the NEXT pass reads its cursor as an incremental delta, so gap deletions are
/// caught only when a re-baseline completes in one pass. Conservative under F-30 by design.
async fn baseline_onedrive(
    app: &AppHandle,
    token_key: &str,
    email: &str,
    cancel: connector_sync::Cancel<'_>,
) -> Result<(Vec<OneDriveItem>, String, bool)> {
    let (deltas, link, truncated) = onedrive::list_delta(token_key, None, cancel).await?;
    // A no-token delta carries no tombstones, so the live-file payloads ARE the enumeration.
    let files: Vec<onedrive::DriveItem> = deltas.into_iter().filter_map(|d| d.file).collect();
    let known: std::collections::HashSet<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        onedrive::known_source_ids(&conn, email)?
            .into_iter()
            .collect()
    };
    let items = onedrive_baseline_items(files, known, truncated, |id| {
        onedrive::source_id_for(email, id)
    });
    Ok((items, link, truncated))
}

/// The OneDrive connector's [`CloudDriver`] (the Microsoft sibling of [`DriveDriver`]). The whole drive
/// uses the efficient Graph delta cursor (first sync enumerates everything, later syncs apply only
/// changes); a folder-scoped account is re-enumerated + reconciled each pass. No shared-drive concept,
/// so `extra_cursors` stays empty.
#[derive(Clone, Copy)]
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
    fn is_item_gone(err: &Error) -> bool {
        onedrive::is_item_unfetchable(err)
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

        // See the Drive driver: the same cancel flag phase 1 reads between accounts, handed down into
        // the listings so a Stop lands inside the walk (#699).
        let cancel = || sync_cancelled::<Self>(app);

        let mut items: Vec<OneDriveItem> = Vec::new();
        let mut new_cursor: Option<String> = None;
        let mut auth_failed = false;
        let mut coverage_incomplete = false;
        let mut last_err: Option<Error> = None;

        match scope.folders.as_deref() {
            // Folder-scoped: re-enumerate selected folders + reconcile (no cursor).
            Some(folders) => {
                match gather_onedrive_folders(
                    app,
                    &token_key,
                    &email,
                    folders,
                    &scope.exclude,
                    &cancel,
                )
                .await
                {
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
            // no-token delta, reconciled against the known set ([`baseline_onedrive`]); otherwise pull
            // the incremental delta.
            None => {
                let cursor = {
                    let state = app.state::<AppState>();
                    let conn = state.conn()?;
                    onedrive::get_cursor(&conn, &email)?
                };
                let outcome: Result<(Vec<OneDriveItem>, String, bool)> = match &cursor {
                    None => baseline_onedrive(app, &token_key, &email, &cancel).await,
                    Some(link) => match onedrive::list_delta(&token_key, Some(link), &cancel).await
                    {
                        Ok((deltas, link, truncated)) => Ok((
                            deltas.into_iter().map(OneDriveItem::Delta).collect(),
                            link,
                            truncated,
                        )),
                        Err(e) if onedrive::is_cursor_expired(&e) => {
                            baseline_onedrive(app, &token_key, &email, &cancel).await
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
///
/// `include_shared_with_me` is false only for the background poller's frequent passes — see
/// [`DriveDriver::include_shared_with_me`]. Every user-initiated sync passes true.
pub(crate) async fn drive_sync_core(
    app: &AppHandle,
    account: Option<String>,
    include_shared_with_me: bool,
) -> Result<usize> {
    run_cloud_sync(
        app,
        DriveDriver {
            include_shared_with_me,
        },
        account,
    )
    .await
}

/// The sync engine behind [`crate::commands::sync_onedrive`] / [`crate::commands::resume_onedrive_sync`].
pub(crate) async fn onedrive_sync_core(app: &AppHandle, account: Option<String>) -> Result<usize> {
    run_cloud_sync(app, OneDriveDriver, account).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_temp_writes_unique_openable_files() {
        let bytes = b"<html><body>hi</body></html>";
        // Identical bytes used to map to one content-addressed name — the collision behind #403.
        let a = stage_temp("pm-test-", "page.html", bytes).unwrap();
        let b = stage_temp("pm-test-", "page.html", bytes).unwrap();
        assert_ne!(a, b, "each staged file must get a fresh name");
        for p in [&a, &b] {
            // Readable back means the writer handle was closed before the path was returned.
            assert_eq!(std::fs::read(p).unwrap(), bytes);
            assert_eq!(p.extension().and_then(|e| e.to_str()), Some("html"));
            assert!(p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pm-test-"));
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn stage_temp_defaults_and_caps_extension() {
        // No extension → `.bin`; an over-long "extension" is not treated as one → `.bin`.
        let noext = stage_temp("pm-test-", "noext", b"x").unwrap();
        assert_eq!(noext.extension().and_then(|e| e.to_str()), Some("bin"));
        let longext = stage_temp("pm-test-", "file.superlongext", b"x").unwrap();
        assert_eq!(longext.extension().and_then(|e| e.to_str()), Some("bin"));
        let _ = std::fs::remove_file(&noext);
        let _ = std::fs::remove_file(&longext);
    }

    #[test]
    fn is_file_lock_error_matches_lock_signatures_only() {
        let py = Error::Other(
            "sidecar convert failed: PermissionError: [Errno 13] Permission denied: 'C:\\Temp\\pm-drive-ab.html'"
                .into(),
        );
        let win = Error::Other(
            "The process cannot access the file because it is being used by another process. (os error 32)"
                .into(),
        );
        let genuine = Error::Other("sidecar convert failed: unsupported file type .xyz".into());
        assert!(is_file_lock_error(&py));
        assert!(is_file_lock_error(&win));
        assert!(!is_file_lock_error(&genuine));
    }

    // ---- the shared apply engine's accounting ------------------------------------------------
    //
    // Everything above this line tests a helper the engine happens to call. These test the engine's
    // own decision: what one item's outcome does to the pass, and — the part that matters — whether
    // it holds the account's delta cursor.

    /// Total of every counter, so "lands in exactly one bucket" can be asserted directly.
    fn total(eff: ItemEffect) -> usize {
        eff.indexed + eff.updated + eff.removed + eff.skipped + eff.failed
    }

    #[test]
    fn every_outcome_lands_in_exactly_one_counter() {
        // A pass whose counters don't sum to the number of items processed is a pass whose
        // "N indexed, M skipped" line silently doesn't add up.
        for outcome in [
            ItemOutcome::Unmapped,
            ItemOutcome::NoText,
            ItemOutcome::FetchFailed { permanent: true },
            ItemOutcome::FetchFailed { permanent: false },
            ItemOutcome::Applied(connector_sync::ActionKind::Indexed),
            ItemOutcome::Applied(connector_sync::ActionKind::Updated),
            ItemOutcome::Applied(connector_sync::ActionKind::Removed),
            ItemOutcome::Applied(connector_sync::ActionKind::Other),
            ItemOutcome::ApplyFailed,
        ] {
            assert_eq!(
                total(item_effect(outcome)),
                1,
                "{outcome:?} must count once"
            );
        }
    }

    #[test]
    fn only_a_real_failure_holds_the_accounts_cursor() {
        // The F-29 rule: an account with a failed item finalizes 'error' with its cursor
        // UNADVANCED, so the change retries instead of being stepped over by a misleading 'ok'.
        assert!(item_effect(ItemOutcome::ApplyFailed).fails_account);
        assert!(item_effect(ItemOutcome::FetchFailed { permanent: false }).fails_account);

        // …and the exception the field is named for: a failure that will recur identically forever
        // must not fail the account, or ONE such file pins it in 'error' with its cursor frozen —
        // M2's revoked "Shared with me" grant, and now any body PM can never index.
        let gone = item_effect(ItemOutcome::FetchFailed { permanent: true });
        assert!(!gone.fails_account);
        assert_eq!((gone.skipped, gone.failed), (1, 0));

        // The engine composes the flag from the driver's per-item classifier OR the shared
        // unindexable one, so pin that a hit on EITHER reaches the non-failing arm. A revert of
        // either half is then a red test rather than a silently re-pinned account.
        for e in [
            Error::Other("Microsoft Graph request failed (404 Not Found): itemNotFound".into()),
            Error::Other("sidecar convert failed [unconvertible]: cannot handle .xyz".into()),
        ] {
            let permanent =
                <OneDriveDriver as CloudDriver>::is_item_gone(&e) || is_permanently_unindexable(&e);
            assert!(permanent, "{e}");
            assert!(!item_effect(ItemOutcome::FetchFailed { permanent }).fails_account);
        }

        // Nothing else ever does.
        for benign in [
            ItemOutcome::Unmapped,
            ItemOutcome::NoText,
            ItemOutcome::Applied(connector_sync::ActionKind::Other),
        ] {
            assert!(!item_effect(benign).fails_account, "{benign:?}");
        }
    }

    #[test]
    fn a_file_the_user_never_hears_about_is_the_one_that_needed_no_work() {
        // An issue is the only trace a file leaves in the report, so the rule is: say something
        // whenever we reached a file and could not index it, and stay quiet when there was
        // nothing to do. A silent fetch failure is a file that vanishes from the user's library
        // with no explanation anywhere.
        assert!(!item_effect(ItemOutcome::Unmapped).records_issue);
        assert!(
            !item_effect(ItemOutcome::Applied(connector_sync::ActionKind::Indexed)).records_issue
        );
        assert!(item_effect(ItemOutcome::NoText).records_issue);
        assert!(item_effect(ItemOutcome::FetchFailed { permanent: true }).records_issue);
        assert!(item_effect(ItemOutcome::FetchFailed { permanent: false }).records_issue);
        assert!(item_effect(ItemOutcome::ApplyFailed).records_issue);
    }

    #[test]
    fn applied_outcomes_map_to_their_own_category() {
        let idx = item_effect(ItemOutcome::Applied(connector_sync::ActionKind::Indexed));
        assert_eq!(
            (idx.indexed, idx.updated, idx.removed, idx.skipped),
            (1, 0, 0, 0)
        );
        let upd = item_effect(ItemOutcome::Applied(connector_sync::ActionKind::Updated));
        assert_eq!(
            (upd.indexed, upd.updated, upd.removed, upd.skipped),
            (0, 1, 0, 0)
        );
        let rem = item_effect(ItemOutcome::Applied(connector_sync::ActionKind::Removed));
        assert_eq!(
            (rem.indexed, rem.updated, rem.removed, rem.skipped),
            (0, 0, 1, 0)
        );
        // `Other` is real work that changed nothing citable — it counts as skipped, not indexed.
        let oth = item_effect(ItemOutcome::Applied(connector_sync::ActionKind::Other));
        assert_eq!(
            (oth.indexed, oth.updated, oth.removed, oth.skipped),
            (0, 0, 0, 1)
        );
    }

    #[test]
    fn pass_counts_accumulate_across_a_mixed_account() {
        // One account's worth of items, exactly as phase 2 would feed them in.
        let mut counts = PassCounts::default();
        for outcome in [
            ItemOutcome::Applied(connector_sync::ActionKind::Indexed),
            ItemOutcome::Applied(connector_sync::ActionKind::Indexed),
            ItemOutcome::Applied(connector_sync::ActionKind::Updated),
            ItemOutcome::Applied(connector_sync::ActionKind::Removed),
            ItemOutcome::Unmapped,
            ItemOutcome::NoText,
            ItemOutcome::FetchFailed { permanent: true },
            ItemOutcome::ApplyFailed,
        ] {
            counts.add(item_effect(outcome));
        }
        assert_eq!(
            (
                counts.indexed,
                counts.updated,
                counts.removed,
                counts.skipped,
                counts.failed
            ),
            (2, 1, 1, 3, 1)
        );
        // The pass's own return value — what `run_detached_sync` reports as "did anything change?"
        assert_eq!(counts.indexed + counts.updated + counts.removed, 4);
    }

    /// A run is one or more passes. Reporting only the last one is how a run that indexed 50 files
    /// ends by announcing "0 indexed" — the follow-up sweep finds nothing new.
    #[test]
    fn a_run_reports_the_sum_of_its_passes_not_the_last_one() {
        let mut run = CloudSyncReport {
            indexed: 50,
            updated: 3,
            removed: 1,
            skipped: 2,
            failed: 1,
            ..Default::default()
        };
        // The folded-in sweep re-scans everything and finds nothing new.
        merge_pass_into_run(&mut run, CloudSyncReport::default());
        assert_eq!(
            run.indexed, 50,
            "the sweep must not erase the first pass's work"
        );
        assert_eq!(
            (run.updated, run.removed, run.skipped, run.failed),
            (3, 1, 2, 1)
        );
    }

    #[test]
    fn cancelled_takes_the_last_pass_not_the_union() {
        let mut run = CloudSyncReport {
            cancelled: true,
            ..Default::default()
        };
        merge_pass_into_run(&mut run, CloudSyncReport::default());
        assert!(
            !run.cancelled,
            "how the run ENDED is the last pass's answer, not a sum"
        );

        let mut run = CloudSyncReport::default();
        merge_pass_into_run(
            &mut run,
            CloudSyncReport {
                cancelled: true,
                ..Default::default()
            },
        );
        assert!(
            run.cancelled,
            "a stop in the final pass ends the run stopped"
        );
    }

    #[test]
    fn merged_issues_stay_within_the_one_pass_cap() {
        let issue = |n: usize| CloudSyncIssue {
            name: format!("f{n}"),
            reason: "nope".into(),
        };
        let cap = connector_sync::MAX_REPORT_ISSUES;
        let mut run = CloudSyncReport {
            issues: (0..cap - 1).map(issue).collect(),
            ..Default::default()
        };
        merge_pass_into_run(
            &mut run,
            CloudSyncReport {
                issues: (0..5).map(issue).collect(),
                ..Default::default()
            },
        );
        assert_eq!(
            run.issues.len(),
            cap,
            "a multi-pass run must not grow the list past the cap"
        );
        assert!(run.issues_truncated, "dropping issues has to be admitted");
    }

    #[test]
    fn truncation_is_sticky_across_passes() {
        let mut run = CloudSyncReport {
            issues_truncated: true,
            ..Default::default()
        };
        merge_pass_into_run(&mut run, CloudSyncReport::default());
        assert!(
            run.issues_truncated,
            "a clean later pass cannot un-truncate an earlier one"
        );
    }

    // ---- whole-corpus re-baselines --------------------------------------------------------------
    //
    // A re-baseline runs precisely when the delta cursor was dead for a while (a 410, a first sync, a
    // folder-scoped→whole-drive round trip that pruned the cursor), so the diff against what is
    // already indexed is the ONLY evidence of what happened in that gap. These pin the mappers both
    // whole-drive paths now go through; the pure planner they wrap is pinned in `index_only.rs`.

    const EMAIL: &str = "a@b.com";

    /// An enumerated Drive file. Only `id` and the hash carry meaning for a reconcile — the rest are
    /// the harmless shape `parse_files` yields for a plain uploaded binary.
    fn drive_file(id: &str, md5: &str, modified: &str) -> drive::DriveFile {
        drive::DriveFile {
            id: id.into(),
            name: format!("{id}.pdf"),
            mime_type: "application/pdf".into(),
            modified_time: Some(modified.into()),
            md5: Some(md5.into()),
            trashed: false,
            web_view_link: None,
            parent_id: None,
            shared_with_me: false,
            shared_with_me_time: None,
            shared_by: None,
            owned_by_me: true,
            shortcut_target_id: None,
            shortcut_target_mime: None,
            shortcut_target_resource_key: None,
            can_download: None,
            resource_key: None,
        }
    }

    /// The OneDrive twin of [`drive_file`].
    fn onedrive_item(id: &str, hash: &str) -> onedrive::DriveItem {
        onedrive::DriveItem {
            id: id.into(),
            name: format!("{id}.pdf"),
            mime_type: "application/pdf".into(),
            modified_time: Some("2026-07-01T00:00:00Z".into()),
            quick_xor_hash: Some(hash.into()),
            sha256_hash: None,
            size: Some(10),
            is_folder: false,
            is_file: true,
            web_url: None,
            parent_id: None,
            parent_name: None,
        }
    }

    /// Every `Delete` the plan emits, by source id — the assertion that actually matters, since a
    /// deletion is the one outcome a hand-built `Add` could never produce.
    fn deleted_ids(items: &[DriveItem]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|i| match i {
                DriveItem::Reconciled {
                    source_id,
                    event: index_only::ChangeEvent::Delete { .. },
                    file: None,
                } => Some(source_id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_completed_rebaseline_soft_deletes_a_file_that_vanished_in_the_gap() {
        // f2 was deleted upstream while the cursor was dead. The old path mapped the enumeration to
        // `Add`s, and `react(Add, Some(ok))` is a Noop — so f2 stayed `source_state = 'ok'` forever,
        // retrieval kept citing it, and a later body fetch just 404'd (I-09.2).
        let known = std::collections::HashSet::from([
            drive::source_id_for(EMAIL, "f1"),
            drive::source_id_for(EMAIL, "f2"),
        ]);
        let items = drive_baseline_items(
            vec![drive_file("f1", "h1", "2026-07-01T00:00:00Z")],
            known,
            false,
            |id| drive::source_id_for(EMAIL, id),
        );
        assert_eq!(deleted_ids(&items), vec!["gdrive:a@b.com:f2"]);
    }

    #[test]
    fn a_truncated_rebaseline_deletes_nothing() {
        // Same inputs, but the listing admits it didn't see everything — absence then proves nothing,
        // so the whole deletion pass is withheld while the files we DID reach still reconcile (F-30).
        let known = std::collections::HashSet::from([
            drive::source_id_for(EMAIL, "f1"),
            drive::source_id_for(EMAIL, "f2"),
        ]);
        let items = drive_baseline_items(
            vec![drive_file("f1", "h1", "2026-07-01T00:00:00Z")],
            known,
            true,
            |id| drive::source_id_for(EMAIL, id),
        );
        assert!(
            deleted_ids(&items).is_empty(),
            "a partial listing must never infer a deletion"
        );
        assert_eq!(items.len(), 1, "the file it did reach still gets an event");
    }

    #[tokio::test]
    async fn a_gather_stopped_mid_walk_deletes_nothing_and_baselines_no_cursor() {
        // The join the Stop fix rests on (#699), asserted end to end rather than in two halves: the
        // flag a CANCELLED listing raises is the same `truncated` these guards already consume. Get
        // that wrong and pressing Stop mid-walk becomes a mass deletion — every file the walk had not
        // reached yet reads as absent from a listing that claims to be complete.
        let known = std::collections::HashSet::from([
            drive::source_id_for(EMAIL, "f1"),
            drive::source_id_for(EMAIL, "f2"),
        ]);

        // One page listed, then Stop — standing in for the real enumeration's page loop.
        let stop = std::sync::atomic::AtomicBool::new(false);
        let (files, truncated) =
            connector_sync::paginate_until(100, &|| stop.swap(true, Ordering::SeqCst), |_page| {
                let f = drive_file("f1", "h1", "2026-07-01T00:00:00Z");
                async move { Ok::<_, Error>((vec![f], Some("more".to_string()))) }
            })
            .await
            .unwrap();

        assert!(truncated, "a cancelled walk reports itself incomplete");
        let items = drive_baseline_items(files, known, truncated, |id| {
            drive::source_id_for(EMAIL, id)
        });
        assert!(
            deleted_ids(&items).is_empty(),
            "f2 was simply never reached — a Stop must not delete it"
        );
        assert_eq!(items.len(), 1, "the page it did list still reconciles");
        // The cursor half of the same contract needs no assertion here: `baseline_my_drive` and
        // `gather_shared_whole` both spell it `(!truncated).then_some(cursor)` at the call site, so the
        // flag proved above is literally what withholds the advance. Phase 2 is belt to that brace —
        // it breaks on the cancel flag before `finalize_or_flag` runs at all.
    }

    #[test]
    fn a_fresh_accounts_first_baseline_is_all_adds() {
        // The unchanged half: with nothing indexed yet every enumerated file is a new ingest, so the
        // first sync of a new account behaves exactly as it did before reconciling.
        let files = vec![
            drive_file("f1", "h1", "2026-07-01T00:00:00Z"),
            drive_file("f2", "h2", "2026-07-01T00:00:00Z"),
        ];
        let items = drive_baseline_items(files, std::collections::HashSet::new(), false, |id| {
            drive::source_id_for(EMAIL, id)
        });
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| matches!(
            i,
            DriveItem::Reconciled {
                event: index_only::ChangeEvent::Add { .. },
                ..
            }
        )));
    }

    #[test]
    fn a_rebaseline_reembeds_a_file_edited_during_the_gap() {
        // The second silent loss: an EDIT during the gap. An `Add` carries no content hash, so the
        // reducer had nothing to compare and no-op'd. Reconciling emits an `Update`, and the chain
        // through `react` is what proves it actually reaches a re-embed rather than another no-op.
        let sid = drive::source_id_for(EMAIL, "f1");
        let items = drive_baseline_items(
            vec![drive_file("f1", "h2", "2026-07-20T00:00:00Z")],
            std::collections::HashSet::from([sid.clone()]),
            false,
            |id| drive::source_id_for(EMAIL, id),
        );
        assert_eq!(items.len(), 1);
        let event = match items.into_iter().next() {
            Some(DriveItem::Reconciled { event, .. }) => event,
            _ => panic!("a present, known file must reconcile"),
        };
        assert_eq!(
            event,
            index_only::ChangeEvent::Update {
                source_id: sid.clone(),
                modified_at: Some("2026-07-20T00:00:00Z".into()),
                new_content_hash: Some("h2".into()),
            }
        );
        let stored = index_only::ItemState {
            source_id: sid.clone(),
            source_modified_at: Some("2026-06-01T00:00:00Z".into()),
            source_content_hash: Some("h1".into()),
            source_state: index_only::SourceState::Ok,
            summary_indexed: false,
        };
        assert_eq!(
            index_only::react(event, Some(&stored)),
            vec![index_only::Action::ReEmbed {
                source_id: sid,
                new_content_hash: "h2".into(),
            }]
        );
    }

    #[test]
    fn onedrive_rebaseline_soft_deletes_a_known_absent_item() {
        // The OneDrive twin: a no-token `/root/delta` re-baseline is an enumeration too, so an item
        // that disappeared while the delta link was dead must be soft-flagged, not silently kept.
        let known = std::collections::HashSet::from([
            onedrive::source_id_for(EMAIL, "01F"),
            onedrive::source_id_for(EMAIL, "01G"),
        ]);
        let items = onedrive_baseline_items(vec![onedrive_item("01F", "h1")], known, false, |id| {
            onedrive::source_id_for(EMAIL, id)
        });
        let deletes: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                OneDriveItem::Reconciled {
                    source_id,
                    event: index_only::ChangeEvent::Delete { .. },
                    item: None,
                } => Some(source_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deletes, vec!["onedrive:a@b.com:01G"]);
    }

    // ---- a permanently-failing item must never pin the account ----------------------------------

    #[test]
    fn onedrive_skips_a_permanently_unfetchable_item_instead_of_pinning_the_account() {
        // Before this, OneDrive took the trait's `false` default: ANY body-fetch error failed the
        // account, so `finalize_or_flag` never wrote the delta link and every later pass replayed the
        // same window onto the same poison item. A whole-drive account has no `exclude` lever, so the
        // only offered remedy — "Press Sync now to retry" — could not work, ever.
        let gone =
            Error::Other("Microsoft Graph request failed (404 Not Found): itemNotFound".into());
        assert!(<OneDriveDriver as CloudDriver>::is_item_gone(&gone));
        assert!(!item_effect(ItemOutcome::FetchFailed { permanent: true }).fails_account);

        // …and the transient half is untouched: a 503 must still fail the account, or the cursor
        // steps past a change PM never applied — the silent gap F-29 exists to prevent.
        let flaky =
            Error::Other("Microsoft Graph request failed (503 Service Unavailable): ".into());
        assert!(!<OneDriveDriver as CloudDriver>::is_item_gone(&flaky));
        assert!(!is_permanently_unindexable(&flaky));
        assert!(item_effect(ItemOutcome::FetchFailed { permanent: false }).fails_account);
    }

    #[test]
    fn an_unconvertible_body_is_a_skip_but_a_broken_engine_is_a_failure() {
        // The provider-independent half (it fixes Drive as well as OneDrive): the engine ANSWERED and
        // refused this file, or the file is over a cap. Retrying either forever is what pins the
        // account, so they are skips. The `[unconvertible]` tag is minted by `SidecarManager::request`
        // from the sidecar's `error_kind`, which `do_convert` sets ONLY for markitdown's
        // UnsupportedFormatException / FileConversionException and PM's own caps.
        for terminal in [
            "sidecar convert failed [unconvertible]: cannot handle .xyz",
            "That OneDrive file is too large to index.",
            "sidecar convert failed [unconvertible]: file is too large to process (60 MiB; the limit is 40 MiB)",
        ] {
            assert!(
                is_permanently_unindexable(&Error::Other(terminal.into())),
                "{terminal}"
            );
        }
        // The negatives are the whole reason this is a predicate and not a `contains("sidecar")`. An
        // engine that is broken or absent will convert this file fine once fixed, and the antivirus
        // lock arrives wearing a convert-failure prefix — skipping either drops an indexable document.
        //
        // The UNTAGGED convert failures are the regression this test now pins. `str(exc)` on the
        // Python side carries no type, so a broken venv answers with an ordinary message under the
        // plain `sidecar convert failed:` prefix. Matching that prefix meant EVERY file was skipped
        // while the delta cursor advanced past it — permanently, since the changes feed never
        // re-offers a committed change. Untagged now means account-fatal, which holds the cursor.
        for retryable in [
            "sidecar convert IO error: broken pipe",
            "document engine is not installed yet — run setup first",
            "sidecar convert failed [unconvertible]: PermissionError: [Errno 13] Permission denied",
            "sidecar convert failed: No module named 'markitdown'",
            "sidecar convert failed: cannot import name 'MarkItDown' from partially initialized module",
            "sidecar convert failed: MissingDependencyException: PdfConverter requires pdfminer.six",
            "sidecar convert failed: [Errno 28] No space left on device",
        ] {
            assert!(
                !is_permanently_unindexable(&Error::Other(retryable.into())),
                "{retryable}"
            );
        }
    }
}
