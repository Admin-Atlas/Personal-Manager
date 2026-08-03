// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared detached-connector-sync machinery — the single-flight lifecycle the three index-only
//! connectors (Drive, OneDrive, local folder) run identically. Each connector keeps its own progress
//! snapshot (`*SyncState` in [`crate`]); this module owns the *lifecycle* those snapshots share, so a
//! fix lands once instead of in three near-identical copies.
//!
//! [`SyncRunGuard`] is the piece that matters: it claims the single-flight slot on construction and
//! releases it on `Drop` — including on a panic or an early `?` return mid-pass. The hand-rolled
//! `running = false` it replaces sat at the end of the sync loop, so anything that skipped that line
//! left `running = true` for the rest of the session, after which every later sync saw "already
//! running" and folded into a follow-up sweep that never came — silently killing the connector
//! (audit F-43). Because `Drop` runs during unwinding too, the guard always clears the flag.
//!
//! On top of the guard this module owns the rest of the shared connector-sync machinery the three
//! engines call identically: [`SyncQueue`] (what a run still owes to requests that arrived mid-sync,
//! and the merge rules for them), [`run_detached_sync`] (the single-flight + crash-resume-marker +
//! follow-up-sweep loop that wraps one connector's pass fn), [`apply_connector_actions`] (the index-only
//! executor — one copy for all three, formerly duplicated per connector), and [`action_category`] /
//! [`ActionKind`] (the tally bucket for a pass's reducer actions).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::error::{Error, Result};
use crate::{db, index_only, ingest, AppState};

/// Cap on how many not-indexed files a connector's sync report lists (memory-bounded; the count of
/// extras beyond this is still conveyed via each report's `issues_truncated`). Shared by the three
/// connectors' `record_*_issue` helpers, which is why it lives here rather than in any one engine.
pub(crate) const MAX_REPORT_ISSUES: usize = 200;

/// The sweeps a running sync still owes — the requests that arrived while a pass was in flight.
///
/// This replaces the bare `rerun: bool` the guard used to carry. That flag recorded *that* a request
/// had arrived but not *what* it asked for, so every follow-up swept **every** target: queueing one
/// account mid-run re-enumerated all of them, and the row showing "Queued" never showed "Syncing…"
/// for its own pass — the user asked for one account and watched the whole connector go round again.
///
/// The merge rules live here, in one place, so the three connectors cannot drift:
///   * request order is preserved, oldest first;
///   * a repeat of a target already waiting collapses into the one already there;
///   * an all-targets request **subsumes** the specific ones — one sweep over everything already
///     covers them, so keeping both would sync those targets twice.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct SyncQueue {
    /// Specific targets awaiting a sweep, oldest first, deduped. Kept empty while `all` is set.
    targets: Vec<String>,
    /// An all-targets sweep is owed. Set by a request that named no target — the background pollers,
    /// and the newly-connected-account case the single-flight fold was originally written for.
    all: bool,
}

impl SyncQueue {
    /// Record a request that arrived while a pass was running. `None` means every target.
    pub fn push(&mut self, target: Option<String>) {
        match target {
            None => {
                self.all = true;
                // One sweep over everything covers whatever was waiting individually.
                self.targets.clear();
            }
            Some(t) => {
                if !self.all && !self.targets.contains(&t) {
                    self.targets.push(t);
                }
            }
        }
    }

    /// Take the next sweep this run owes: `Some(Some(target))` for a specific one, `Some(None)` for
    /// an all-targets sweep, `None` when the run is finished.
    pub fn take_next(&mut self) -> Option<Option<String>> {
        if !self.targets.is_empty() {
            // O(n) on a list bounded by the number of connected accounts / tracked folders.
            return Some(Some(self.targets.remove(0)));
        }
        if self.all {
            self.all = false;
            return Some(None);
        }
        None
    }

    /// Nothing is owed.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty() && !self.all
    }

    /// Drop everything waiting — a stop, or a fresh claim clearing what a panicked pass orphaned.
    pub fn clear(&mut self) {
        self.targets.clear();
        self.all = false;
    }
}

/// What one pass of a run is being asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassRequest {
    /// The target for this pass — an account email / folder key, or `None` for every target.
    pub target: Option<String>,
    /// True for a follow-up sweep draining a request that arrived mid-run; false for the run's first
    /// pass. A connector that varies what a pass covers reads this: the folded-in requests are not
    /// recorded individually, so `rerun` is all a pass knows about why it exists (see
    /// `DriveDriver::for_pass` and its Shared-with-me widening).
    pub rerun: bool,
}

/// What [`SyncRunGuard::pass_complete`] decided at the end of a pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextPass {
    /// The run is over; `running` has been cleared.
    Done,
    /// Another sweep is owed, for this target (`None` = every target); `running` stays held.
    Sweep(Option<String>),
}

/// The single-flight fields every detached-sync snapshot carries, so [`SyncRunGuard`] can own the
/// `running`/queue lifecycle without knowing the concrete snapshot type. Implemented by the three
/// `*SyncState` snapshots in [`crate`] (each also carries connector-specific display fields — the
/// counters, the target, the last report — that the guard never touches).
///
/// **Every implementor must also be listed in [`AppState::connector_slots`]** — that register is what
/// the background stand-down gates read, and a slot missing from it fails silently: the gates simply
/// read "nothing is syncing" (audit B1-7, which is what happened to the local-folder slot).
pub trait SyncSlot {
    /// Whether a sync is in flight right now.
    fn running(&self) -> bool;
    fn set_running(&mut self, running: bool);
    /// The sweeps this run still owes — see [`SyncQueue`]. Handed out as `&mut` so the merge rules
    /// stay in the queue itself rather than being reimplemented by each of the three snapshots.
    fn queue(&mut self) -> &mut SyncQueue;
    /// Reset the per-pass display counters and point the snapshot at the follow-up sweep's target, so
    /// the row that showed "Queued" shows "Syncing…" when its turn comes.
    fn reset_for_rerun(&mut self, target: Option<String>);
    /// Reset the snapshot to the start of a fresh run: `running` set, `target` recorded (the account
    /// email / folder key, or `None` for an all-targets pass), every per-pass counter, the last
    /// report and any orphaned queue cleared. Called by [`SyncRunGuard::claim`] under the same lock
    /// that claims the slot — implementations must set `running` themselves.
    fn begin_pass(&mut self, target: Option<String>);
}

/// A poison-tolerant read of one detached-sync slot's `running` flag. Object-safe on purpose: the
/// three slots hold *different* snapshot types (`CloudSyncState` for both cloud connectors,
/// `LocalFolderSyncState` for the local one), so this is what lets them be enumerated together as one
/// register — see [`AppState::connector_slots`] and [`any_running`].
pub trait SlotStatus {
    /// Whether this slot's sync is in flight right now.
    fn slot_running(&self) -> bool;
}

impl<S: SyncSlot> SlotStatus for Mutex<S> {
    fn slot_running(&self) -> bool {
        // A poisoned slot reads as IDLE. These gates are advisory — a `true`-on-poison read would
        // stand every background job down for the rest of the session, which is exactly the wedge the
        // F-43 guard work exists to prevent.
        self.lock().map(|s| s.running()).unwrap_or(false)
    }
}

/// True when ANY of the given slots is mid-run — the whole body of [`AppState::sync_active`].
///
/// It takes a slice rather than named slots so the list of connectors lives in ONE place. The gate
/// used to be hand-written as `drive || onedrive` and the local-folder slot was simply left out of it
/// (audit B1-7), which no caller could see: every consumer just read "no sync is running".
pub fn any_running(slots: &[&dyn SlotStatus]) -> bool {
    slots.iter().any(|s| s.slot_running())
}

/// RAII single-flight guard for a detached connector sync. Construction ([`SyncRunGuard::claim`])
/// either claims the slot or folds the request into the running pass; the guard releases the slot on
/// `Drop`.
///
/// The `Drop` release is the fix for audit F-43. The hand-rolled `running = false` it replaces lived
/// at the tail of the sync loop, so a panic (or any early return) mid-pass skipped it and pinned
/// `running = true` for the session — after which every later sync saw "already running" and folded
/// into a rerun that never happened. `Drop` runs while unwinding too, so the guard clears the flag no
/// matter how the pass ends.
pub struct SyncRunGuard<'a, S: SyncSlot> {
    slot: &'a Mutex<S>,
}

impl<'a, S: SyncSlot> SyncRunGuard<'a, S> {
    /// Try to claim the single-flight slot for a run targeting `target` (`None` = every target).
    ///
    /// - `Ok(Some(guard))` — the slot was free; the snapshot has been reset for this run and
    ///   `running` stays ours until the guard drops.
    /// - `Ok(None)` — a sync is already running; `target` has been queued so the in-flight run sweeps
    ///   it when the current pass ends, and the caller should return without starting a second,
    ///   racing sync.
    pub fn claim(slot: &'a Mutex<S>, target: Option<String>) -> Result<Option<Self>> {
        let mut s = slot
            .lock()
            .map_err(|_| Error::Other("sync state poisoned".into()))?;
        if s.running() {
            s.queue().push(target);
            return Ok(None);
        }
        // Claim and reset under ONE lock. These used to be two steps — the claim here, `begin_pass`
        // in `run_detached_sync` — leaving a window in which a request that arrived in between was
        // queued and then wiped by the reset. Rare, but what it lost was the sync a user had just
        // asked for, silently.
        s.begin_pass(target);
        Ok(Some(Self { slot }))
    }

    /// End-of-pass handoff. Call once after each pass with whether the user asked to stop.
    ///
    /// Returns [`NextPass::Sweep`] when another sweep is owed, carrying the queued target it is for,
    /// with `running` still held and the per-pass counters reset for it. Returns [`NextPass::Done`]
    /// when the run is over, having cleared `running`. Draining the queue and clearing `running`
    /// happen under one lock, so a request landing exactly as a pass finishes is never lost to a race
    /// against the clear: it either extends this run or starts cleanly once `running` is false.
    pub fn pass_complete(&self, stopped: bool) -> Result<NextPass> {
        let mut s = self
            .slot
            .lock()
            .map_err(|_| Error::Other("sync state poisoned".into()))?;
        // A stop ends the run outright, dropping whatever was queued behind it: the user asked for
        // the indexing to stop, and honouring a request they made *before* deciding that would just
        // start it up again.
        if stopped {
            s.queue().clear();
            s.set_running(false);
            return Ok(NextPass::Done);
        }
        match s.queue().take_next() {
            Some(target) => {
                s.reset_for_rerun(target.clone());
                Ok(NextPass::Sweep(target))
            }
            None => {
                s.set_running(false);
                Ok(NextPass::Done)
            }
        }
    }
}

impl<S: SyncSlot> Drop for SyncRunGuard<'_, S> {
    fn drop(&mut self) {
        // Safety net. On the happy path `pass_complete` already cleared `running`, so this is a
        // no-op; it does real work only when the pass panicked or returned early before a clean
        // handoff — exactly the case the old hand-rolled flag missed (F-43). Best-effort: a poisoned
        // lock is already unrecoverable, so there is nothing to release.
        if let Ok(mut s) = self.slot.lock() {
            if s.running() {
                s.set_running(false);
            }
        }
    }
}

/// The single-flight lifecycle every detached connector sync runs identically: claim the slot (or
/// queue this request behind the running one and return), clear the stop flag, then run `pass` — once
/// per sweep the guard hands back — and finally drop the crash-resume marker on the clean exit.
///
/// A run can span several passes: a request arriving mid-sync is queued rather than started, and each
/// queued target gets its own pass in turn (an all-targets request collapses them into one). That is
/// what makes a row read "Queued" and then "Syncing…" rather than "Queued" until the whole connector
/// has been swept — see [`SyncQueue`].
///
/// `pass` is the connector's own one-pass fn (`run_drive_sync` / `run_onedrive_sync` /
/// `run_local_sync`); `slot`, `cancel`, and `pending_key` are that connector's `AppState` fields.
/// This is the body the three `*_sync_core` wrappers shared byte-for-byte except for those four
/// values — hoisting it keeps the F-43 guard handling and the crash-resume marking in one place.
///
/// `finish` emits the connector's terminal event, and is called exactly ONCE per run — after the
/// last pass, on every exit path including an error. It must not live inside `pass`: a run can span
/// several passes (that is what a folded-in request produces), and a terminal event per *pass* tells
/// the UI the run is over while it is still going, which resets the progress and queued indicators
/// mid-run and re-fires anything gated on completion.
///
/// It also cannot be replaced by a "more passes coming" flag on the per-pass event. The pass's own
/// cancelled flag and the `stopped` read below are taken at different moments — the gap spans the
/// manifest flush — so a Stop landing in between would announce "more coming" and then never send
/// it, stranding the bar for the rest of the session.
pub async fn run_detached_sync<F, Fut, S, G>(
    st: &AppState,
    slot: &Mutex<S>,
    cancel: &AtomicBool,
    pending_key: &str,
    target: Option<String>,
    pass: F,
    finish: G,
) -> Result<usize>
where
    S: SyncSlot,
    F: Fn(PassRequest) -> Fut,
    Fut: std::future::Future<Output = Result<usize>>,
    G: FnOnce(),
{
    // Claim the single-flight slot, or queue this request behind the running one. The guard clears
    // `running` on drop — including if a pass panics — so a crashed sync can't wedge the connector
    // with `running = true` for the rest of the session (F-43).
    let Some(guard) = SyncRunGuard::claim(slot, target.clone())? else {
        return Ok(0);
    };
    // Fresh stop flag for this run (the claim already reset the snapshot).
    cancel.store(false, Ordering::SeqCst);

    let mut req = PassRequest {
        target,
        rerun: false,
    };
    let mut result;
    loop {
        // The crash-resume marker, rewritten per pass. A run can now sweep several targets in turn,
        // and resuming the one that was actually interrupted beats resuming one that already
        // finished. Only the pass in flight is durable: the rest of the queue lives in memory, so a
        // crash still loses what was merely waiting — the same exposure the old single marker had.
        if let Ok(conn) = st.conn() {
            let marker = serde_json::to_string(&req.target).unwrap_or_else(|_| "null".to_string());
            let _ = db::set_setting(&conn, pending_key, &marker);
        }
        result = pass(req.clone()).await;
        let stopped = cancel.load(Ordering::SeqCst);
        // The guard drains the queue and clears `running` under one lock, so a request landing
        // exactly as we finish isn't lost to a race against clearing `running`.
        match guard.pass_complete(stopped)? {
            NextPass::Done => break,
            NextPass::Sweep(next) => {
                req = PassRequest {
                    target: next,
                    rerun: true,
                }
            }
        }
    }

    // Clean exit (finished or stopped): drop the crash-resume marker so launch doesn't re-run it.
    {
        if let Ok(conn) = st.conn() {
            let _ = db::delete_setting(&conn, pending_key);
        }
    }
    // The run is over — announce it once, however it ended. Deliberately before the `result` is
    // returned rather than only on `Ok`: a pass that bailed with `?` still leaves the UI mid-run,
    // and a bar that never terminates is a worse failure than a terminal event reporting little.
    finish();
    result
}

/// Drive a token-paginated listing to exhaustion or a runaway page guard — the single answer to "what
/// if the continuation token never clears", replacing the silent-truncate / hard-fail / node-break mix
/// each connector used to hand-roll (the root of audit F-30). `fetch(cursor)` returns one page's
/// `(items, next_cursor)`; the first call gets `None`, and a `None` next ends the walk.
///
/// Returns everything gathered plus whether the guard tripped. **`truncated == true` means the listing
/// is INCOMPLETE**: a caller diffing it against what's already indexed must NOT treat a file's absence
/// as a deletion (see [`index_only::reconcile_enumeration`]'s `complete` flag), and a delta/baseline
/// caller must NOT advance its sync cursor past this pass — else a backstop meant for a stuck token
/// turns into false deletions or silently-skipped files. `max_pages` is the caller's own bound (each
/// connector keeps its constant, so the number stays visible at the call site).
pub async fn paginate<T, F, Fut>(max_pages: usize, fetch: F) -> Result<(Vec<T>, bool)>
where
    F: Fn(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<(Vec<T>, Option<String>)>>,
{
    paginate_until(max_pages, &never_cancelled, fetch).await
}

/// A cheap, side-effect-free "has the user asked this run to stop?" probe, threaded into the listing
/// walk so a Stop lands mid-enumeration instead of at the next account boundary.
///
/// `Sync` because the probe is captured by the `Fn` closure each page's future is built from, and that
/// future has to stay `Send` to cross the sync engine's `.await` points.
pub type Cancel<'a> = &'a (dyn Fn() -> bool + Sync);

/// The probe for a listing with no run behind it to cancel — the folder/account pickers, the calendar,
/// and the backup destination walk. Named rather than an inline `&|| false` so the call sites read as a
/// deliberate "nothing to stop here" instead of a forgotten argument.
pub fn never_cancelled() -> bool {
    false
}

/// [`paginate`] with a cancellation probe — the seam that makes "Stop indexing" bite during the listing
/// walk rather than only between accounts.
///
/// The probe is read **before each page fetch**, so a Stop pressed mid-walk costs at most the request
/// already in flight. A cancelled walk returns what it gathered so far with `truncated = true`, which is
/// the same signal the page guard raises and therefore inherits its two guarantees for free: the
/// reconcile plan is built with `complete = false` (so no absence is read as a deletion — see
/// [`crate::index_only::reconcile_enumeration`]) and a baseline caller withholds its cursor. That is the
/// whole reason cancellation reuses this flag instead of a separate one: a Stop that deleted documents
/// would be far worse than a Stop that was slow.
pub async fn paginate_until<T, F, Fut>(
    max_pages: usize,
    cancelled: Cancel<'_>,
    fetch: F,
) -> Result<(Vec<T>, bool)>
where
    F: Fn(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<(Vec<T>, Option<String>)>>,
{
    let mut out: Vec<T> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..max_pages {
        if cancelled() {
            return Ok((out, true));
        }
        let (mut items, next) = fetch(cursor).await?;
        out.append(&mut items);
        match next {
            Some(n) => cursor = Some(n),
            None => return Ok((out, false)),
        }
    }
    // Every page up to the guard returned a next-token: the token never cleared. Hand back what we
    // gathered, flagged incomplete, rather than looping forever or hard-erroring the whole sync.
    Ok((out, true))
}

/// Which tally bucket a reducer's actions fall into, for a sync summary. Named for the shared
/// index-only foundation, not any one connector — Drive, OneDrive, and the local folder all classify
/// their per-item work through [`action_category`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionKind {
    Indexed,
    Updated,
    Removed,
    Other,
}

/// Classify a reducer's actions into the summary bucket a sync reports. A fresh ingest wins over a
/// re-embed wins over a soft-delete; anything else (a touch / no-op) is `Other`.
pub fn action_category(actions: &[index_only::Action]) -> ActionKind {
    if actions
        .iter()
        .any(|a| matches!(a, index_only::Action::IngestNew { .. }))
    {
        ActionKind::Indexed
    } else if actions
        .iter()
        .any(|a| matches!(a, index_only::Action::ReEmbed { .. }))
    {
        ActionKind::Updated
    } else if actions.iter().any(|a| {
        matches!(
            a,
            index_only::Action::SetState {
                state: index_only::SourceState::SourceMissing,
                ..
            }
        )
    }) {
        ActionKind::Removed
    } else {
        ActionKind::Other
    }
}

/// Run a reducer's actions against the store on a blocking thread (the index-only executor embeds via
/// the sidecar), returning whether the mirror changed (`dirtied`). One copy for all three connectors —
/// Drive, OneDrive, and the local folder ran byte-for-byte identical copies before this consolidation.
/// Does NOT write the portable manifest: the caller drives that through a [`ManifestFlusher`], so a
/// bulk pass rewrites the manifest once per `MANIFEST_FLUSH_EVERY` items instead of once per item.
/// Mirrors `dev_apply_change_event`'s execution shape.
///
/// This is also where a new document is ANNOUNCED. Every index-only connector — Drive, OneDrive, the
/// local folder, and the live filesystem watcher — funnels through here, so one emit covers all four,
/// and it is the innermost place that still holds an `AppHandle` (`AppState` alone cannot emit). The
/// row is already committed by the time the event goes out, so a listener that immediately queries
/// for it will find it.
///
/// The return type stays `bool` deliberately: widening it would touch seven call sites across
/// `cloud_sync` and `localfolder` that pass the value straight into `flusher.note(...)`, none of
/// which care about arrivals.
pub fn apply_connector_actions(
    app: &AppHandle,
    actions: &[index_only::Action],
    fetched: Option<index_only::PointerInput>,
) -> Result<bool> {
    let state = app.state::<AppState>();
    // Gentle mode caps the embedding batch to bound peak memory. This runs once per item, so reading
    // it here also makes gentle batching engage mid-sync (alongside the per-item pause).
    let gateway = {
        let conn = state.conn()?;
        state
            .gateway_for_write(&conn)?
            .with_embed_batch(db::indexing_embed_batch(&conn))
    };
    let applied = index_only::apply_actions(state.inner(), &gateway, actions, fetched.as_ref())?;
    // Drop the DB guard before emitting: `gateway` borrows a connection, and a listener reacting
    // synchronously must not find the mutex still held (it is not reentrant).
    drop(gateway);
    for document in &applied.landed {
        // Noted for the background duplicate check whether or not it is announced (#711): an
        // already-reviewed row is not news for the Review queue, but it is every bit as capable of
        // being the second copy of something.
        crate::duplicates::note_arrival(state.inner(), document.id);
        // An already-reviewed row is not an arrival for the Review queue's purposes. Cheap to check
        // here, and it keeps a promoted-then-resynced file from reappearing as new.
        if !document.reviewed {
            let _ = app.emit(ingest::DOCUMENT_LANDED, document);
        }
    }
    Ok(applied.dirtied)
}

/// Rewrite the index-only manifest now from the current DB mirror — the batched replacement for the
/// old per-item write inside `register_pointer`. Idempotent (writes the current mirror∪file union), so
/// re-running it is harmless; that's what lets [`ManifestFlusher`] flush on a bound and again at the end.
fn flush_manifest(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let (vault_root, cipher) = state.manifest_io()?;
    let conn = state.conn()?;
    // The rows this flush covers are already committed, so a failure here leaves the file BEHIND the
    // mirror. Record that before propagating: the `Drop` safety net can only log the error, and
    // without the flag the next boot would apply the older file over the newer rows.
    if let Err(e) = index_only::write_synced(&conn, &vault_root, &cipher) {
        index_only::mark_manifest_stale(&conn);
        return Err(e);
    }
    Ok(())
}

/// Batches index-only manifest rewrites across a sync pass. `apply_connector_actions` now only commits
/// DB rows; the encrypted manifest — an O(n) read-merge-encrypt-write each time, so writing it per item
/// made a pass O(n²) — is rewritten here every [`index_only::MANIFEST_FLUSH_EVERY`] items and once at
/// the end. Crash safety is unchanged in outcome: a crash between flushes leaves committed DB rows
/// without a manifest entry, which [`index_only::reconcile_on_open`] self-heals from the mirror on next
/// open (and the interrupted account's cursor stays unadvanced, so those items re-observe anyway). The
/// [`Drop`] flush is the belt-and-suspenders that bounds the exposure to `< MANIFEST_FLUSH_EVERY` items
/// even when an early return, `?`-propagated error, or panic mid-pass skips the explicit [`Self::flush`].
pub struct ManifestFlusher {
    /// The write action — `flush_manifest(app)` in production; a counter in tests. Boxed so the cadence
    /// logic (window, reset, the Drop safety net) is unit-testable without a live app + sidecar.
    flush_fn: Box<dyn FnMut() -> Result<()> + Send>,
    /// Mirror-changing items applied since the last flush (reset on each write).
    pending: usize,
}

impl ManifestFlusher {
    pub fn new(app: &AppHandle) -> Self {
        let app = app.clone();
        Self::with_flush(Box::new(move || flush_manifest(&app)))
    }

    fn with_flush(flush_fn: Box<dyn FnMut() -> Result<()> + Send>) -> Self {
        Self {
            flush_fn,
            pending: 0,
        }
    }

    /// Record that an applied item did (`dirtied`) or didn't change the mirror; write the manifest if
    /// the batch window is now full. Call after each `apply_connector_actions`.
    pub fn note(&mut self, dirtied: bool) -> Result<()> {
        if dirtied {
            self.pending += 1;
        }
        if self.pending >= index_only::MANIFEST_FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    /// Write the manifest now if anything is pending, resetting the window. Call at the end of a pass
    /// (and after a single watcher/dev event) so the tail is persisted with its error surfaced.
    pub fn flush(&mut self) -> Result<()> {
        if self.pending > 0 {
            (self.flush_fn)()?;
            self.pending = 0;
        }
        Ok(())
    }
}

impl Drop for ManifestFlusher {
    fn drop(&mut self) {
        // Safety net for an early return / `?`-propagated error / panic that skipped `flush()`: persist
        // the remainder best-effort so a bail strands at most a boot self-heal, never lost data. Errors
        // can only be logged here; `reconcile_on_open` re-derives the manifest from the mirror regardless.
        if self.pending > 0 {
            if let Err(e) = (self.flush_fn)() {
                eprintln!(
                    "index_only: manifest flush during cleanup failed ({e}); reconcile_on_open will heal it"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A `ManifestFlusher` whose write just bumps a shared counter, so the batching cadence is testable
    /// without a live app + sidecar. Returns the flusher and the count of writes it has performed.
    fn counting_flusher() -> (ManifestFlusher, Arc<AtomicUsize>) {
        let flushes = Arc::new(AtomicUsize::new(0));
        let seen = flushes.clone();
        let flusher = ManifestFlusher::with_flush(Box::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        (flusher, flushes)
    }

    #[test]
    fn manifest_flusher_writes_once_per_window_and_never_on_no_change() {
        let (mut flusher, flushes) = counting_flusher();
        // Items that didn't change the mirror never advance the window.
        for _ in 0..1000 {
            flusher.note(false).unwrap();
        }
        assert_eq!(
            flushes.load(Ordering::SeqCst),
            0,
            "no-change items don't flush"
        );
        // Exactly one window's worth of changes triggers exactly one write, and the window resets.
        for _ in 0..index_only::MANIFEST_FLUSH_EVERY {
            flusher.note(true).unwrap();
        }
        assert_eq!(
            flushes.load(Ordering::SeqCst),
            1,
            "a full window flushes once"
        );
        // A partial window doesn't write until the explicit end-of-pass flush.
        for _ in 0..10 {
            flusher.note(true).unwrap();
        }
        assert_eq!(flushes.load(Ordering::SeqCst), 1, "a partial window holds");
        flusher.flush().unwrap();
        assert_eq!(
            flushes.load(Ordering::SeqCst),
            2,
            "end-of-pass flush writes the tail"
        );
        // A second flush with nothing pending is a no-op, and so is Drop.
        flusher.flush().unwrap();
        drop(flusher);
        assert_eq!(
            flushes.load(Ordering::SeqCst),
            2,
            "no spurious writes when nothing is pending"
        );
    }

    #[test]
    fn manifest_flusher_drop_persists_an_unflushed_remainder() {
        // The belt-and-suspenders: an early return / `?` bail that skips `flush()` must still persist
        // what was ingested (bounded to < one window), so a mid-pass failure strands nothing beyond it.
        let (flusher, flushes) = counting_flusher();
        {
            let mut f = flusher;
            f.note(true).unwrap(); // one pending, below the window — no explicit flush (simulated bail)
            assert_eq!(flushes.load(Ordering::SeqCst), 0);
        }
        assert_eq!(
            flushes.load(Ordering::SeqCst),
            1,
            "Drop flushed the un-flushed remainder"
        );
    }

    /// A minimal snapshot standing in for the real `*SyncState`s — same single-flight fields, plus a
    /// stand-in target/counters so `reset_for_rerun` has something to clear.
    #[derive(Default)]
    struct FakeSlot {
        running: bool,
        queue: SyncQueue,
        processed: usize,
        total: Option<usize>,
        target: Option<String>,
    }

    impl SyncSlot for FakeSlot {
        fn running(&self) -> bool {
            self.running
        }
        fn set_running(&mut self, running: bool) {
            self.running = running;
        }
        fn queue(&mut self) -> &mut SyncQueue {
            &mut self.queue
        }
        fn reset_for_rerun(&mut self, target: Option<String>) {
            self.processed = 0;
            self.total = None;
            self.target = target;
        }
        fn begin_pass(&mut self, target: Option<String>) {
            *self = FakeSlot {
                running: true,
                target,
                ..Default::default()
            };
        }
    }

    /// Queue one request, as a busy slot's `claim` does.
    fn queue(slot: &Mutex<FakeSlot>, target: Option<&str>) {
        let second = SyncRunGuard::claim(slot, target.map(str::to_string)).unwrap();
        assert!(second.is_none(), "a busy slot can't be claimed twice");
    }

    #[test]
    fn queue_keeps_request_order_and_dedups() {
        let mut q = SyncQueue::default();
        assert!(q.is_empty());
        q.push(Some("a".into()));
        q.push(Some("b".into()));
        q.push(Some("a".into())); // already waiting — collapses
        assert!(!q.is_empty());
        assert_eq!(q.take_next(), Some(Some("a".into())));
        assert_eq!(q.take_next(), Some(Some("b".into())));
        assert_eq!(q.take_next(), None, "drained");
        assert!(q.is_empty());
    }

    #[test]
    fn an_all_targets_request_subsumes_the_specific_ones() {
        // Syncing "everything" already covers whatever was queued individually, so keeping both would
        // sweep those targets twice.
        let mut q = SyncQueue::default();
        q.push(Some("a".into()));
        q.push(None);
        q.push(Some("b".into()));
        assert_eq!(q.take_next(), Some(None), "one all-targets sweep");
        assert_eq!(q.take_next(), None, "and nothing else");
    }

    #[test]
    fn claim_takes_a_free_slot_and_marks_running() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot, Some("acc-a".into())).unwrap();
        assert!(guard.is_some(), "a free slot is claimable");
        let mut s = slot.lock().unwrap();
        assert!(s.running);
        assert_eq!(
            s.target.as_deref(),
            Some("acc-a"),
            "claim reset the snapshot"
        );
        assert!(s.queue().is_empty(), "a fresh claim owes nothing");
    }

    #[test]
    fn claim_queues_a_second_request_against_its_own_target() {
        // The bug this replaces: the fold recorded only *that* a request arrived, so the follow-up
        // swept every target and the queued account never got a pass of its own.
        let slot = Mutex::new(FakeSlot {
            running: true,
            ..Default::default()
        });
        queue(&slot, Some("acc-b"));
        assert_eq!(
            slot.lock().unwrap().queue().take_next(),
            Some(Some("acc-b".into())),
            "the queued sweep remembers which account was asked for"
        );
    }

    #[test]
    fn pass_complete_clears_running_when_nothing_is_queued() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot, None).unwrap().unwrap();
        assert_eq!(
            guard.pass_complete(false).unwrap(),
            NextPass::Done,
            "nothing owed → done"
        );
        assert!(!slot.lock().unwrap().running);
    }

    #[test]
    fn pass_complete_sweeps_each_queued_target_in_turn() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot, Some("acc-a".into()))
            .unwrap()
            .unwrap();
        queue(&slot, Some("acc-b"));
        queue(&slot, Some("acc-c"));
        {
            let mut s = slot.lock().unwrap();
            s.processed = 7;
            s.total = Some(7);
        }

        assert_eq!(
            guard.pass_complete(false).unwrap(),
            NextPass::Sweep(Some("acc-b".into())),
            "the first queued account gets the next pass"
        );
        {
            let s = slot.lock().unwrap();
            assert!(s.running, "running stays set across the follow-up sweep");
            assert_eq!(
                s.target.as_deref(),
                Some("acc-b"),
                "the snapshot points at the sweep's own target, so its row reads Syncing"
            );
            assert_eq!(s.processed, 0, "per-pass counters reset");
            assert_eq!(s.total, None);
        }
        assert_eq!(
            guard.pass_complete(false).unwrap(),
            NextPass::Sweep(Some("acc-c".into())),
        );
        assert_eq!(guard.pass_complete(false).unwrap(), NextPass::Done);
        assert!(!slot.lock().unwrap().running);
    }

    #[test]
    fn a_stop_beats_everything_queued() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot, None).unwrap().unwrap();
        queue(&slot, Some("acc-b"));
        assert_eq!(
            guard.pass_complete(true).unwrap(),
            NextPass::Done,
            "a stop request wins over anything waiting"
        );
        let mut s = slot.lock().unwrap();
        assert!(!s.running);
        assert!(
            s.queue().is_empty(),
            "stopping drops the queue — honouring it would restart the indexing the user just stopped"
        );
    }

    #[test]
    fn begin_pass_sets_running_and_target_and_clears_stale_state() {
        // `claim` calls begin_pass under its own lock to reset the snapshot for a fresh run.
        let mut slot = FakeSlot {
            running: true,
            processed: 9,
            total: Some(9),
            target: Some("stale".into()),
            queue: SyncQueue {
                targets: vec!["orphan".into()],
                all: true,
            },
        };
        slot.begin_pass(Some("acc-a".into()));
        assert!(slot.running, "the run holds the slot");
        assert_eq!(slot.target.as_deref(), Some("acc-a"), "target recorded");
        assert_eq!(slot.processed, 0, "stale counters cleared");
        assert_eq!(slot.total, None);
        assert!(
            slot.queue().is_empty(),
            "a queue orphaned by a panicked pass doesn't leak into the next run"
        );
    }

    #[test]
    fn any_running_is_false_when_every_slot_is_idle() {
        let drive = Mutex::new(FakeSlot::default());
        let onedrive = Mutex::new(FakeSlot::default());
        let local = Mutex::new(FakeSlot::default());
        assert!(
            !any_running(&[&drive, &onedrive, &local]),
            "nothing is syncing, so nothing stands down"
        );
    }

    #[test]
    fn any_running_is_true_when_only_the_last_slot_runs() {
        // The exact omission B1-7 shipped: the local-folder slot is the THIRD one, and the hand-rolled
        // gate ORed the first two, so every background stand-down read false through a local sync.
        let drive = Mutex::new(FakeSlot::default());
        let onedrive = Mutex::new(FakeSlot::default());
        let local = Mutex::new(FakeSlot {
            running: true,
            ..Default::default()
        });
        assert!(
            any_running(&[&drive, &onedrive, &local]),
            "the gate must not stop at the first two slots"
        );
    }

    #[test]
    fn a_poisoned_slot_reads_as_idle_and_never_masks_a_running_one() {
        // `running: true` under the poison, so this can only pass via the poison arm — a slot whose
        // lock is unrecoverable reads IDLE, because standing every background job down for the rest of
        // the session is a worse failure than one advisory gate answering optimistically.
        let poisoned = Mutex::new(FakeSlot {
            running: true,
            ..Default::default()
        });
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // the deliberate panic below isn't a test failure
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = poisoned.lock().unwrap();
            panic!("poison the slot");
        }));
        std::panic::set_hook(hook);

        assert!(poisoned.is_poisoned(), "the panic really poisoned the lock");
        assert!(!poisoned.slot_running(), "a poisoned slot reads as idle");

        let local = Mutex::new(FakeSlot {
            running: true,
            ..Default::default()
        });
        assert!(
            any_running(&[&poisoned, &local]),
            "and a poisoned slot never masks one that really is running"
        );
    }

    #[tokio::test]
    async fn paginate_stops_at_the_guard_and_flags_truncated() {
        // A continuation token that never clears must stop AT the bound rather than looping forever,
        // and hand back what it gathered flagged INCOMPLETE — that flag is what withholds the caller's
        // deletion sweep (F-30).
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let max_pages = 4;
        let (out, truncated) = paginate(max_pages, |cursor| {
            let seen = seen.clone();
            async move {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    cursor.is_none(),
                    n == 0,
                    "only the first call gets no cursor"
                );
                Ok::<_, Error>((vec![n], Some(format!("page-{}", n + 1))))
            }
        })
        .await
        .unwrap();

        assert!(
            truncated,
            "the token never cleared, so the listing is partial"
        );
        assert_eq!(out.len(), max_pages, "everything gathered up to the guard");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            max_pages,
            "the guard bounded the fetches — it didn't just stop counting them"
        );
    }

    #[tokio::test]
    async fn paginate_reports_complete_when_the_token_clears() {
        // The false-positive side, and it is not the "safe" one: `truncated` suppresses the caller's
        // deletion sweep, so a listing that IS complete but claims otherwise leaves files that really
        // were deleted in the index indefinitely.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let (out, truncated) = paginate(10, |_cursor| {
            let seen = seen.clone();
            async move {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Error>((vec![n], (n < 2).then(|| format!("page-{}", n + 1))))
            }
        })
        .await
        .unwrap();

        assert!(!truncated, "the token cleared inside the bound → complete");
        assert_eq!(out, vec![0, 1, 2], "every page's items, in page order");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "and it stopped asking once the token cleared"
        );
    }

    #[tokio::test]
    async fn paginate_until_stops_mid_walk_and_keeps_what_it_gathered() {
        // The Stop-indexing seam (#699). A cancel raised while page 3 is being asked for must end the
        // walk THERE — keeping pages 1 and 2 — and flag the listing incomplete, because that flag is
        // the only thing standing between a half-listed account and a reconcile that reads every
        // unreached file as deleted.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let asked = calls.clone();
        let probe = move || {
            // Trip the flag once two pages are in, so the cancel lands mid-walk rather than before it.
            if asked.load(Ordering::SeqCst) >= 2 {
                flag.store(true, Ordering::SeqCst);
            }
            stop.load(Ordering::SeqCst)
        };

        let (out, truncated) = paginate_until(100, &probe, |_cursor| {
            let seen = seen.clone();
            async move {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                // A token that never clears, so only the cancel can end this walk.
                Ok::<_, Error>((vec![n], Some(format!("page-{}", n + 1))))
            }
        })
        .await
        .unwrap();

        assert_eq!(
            out,
            vec![0, 1],
            "the pages fetched before the Stop are kept"
        );
        assert!(
            truncated,
            "a cancelled walk is INCOMPLETE — no absence-inferred deletions, no cursor advance"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the probe is read BEFORE each fetch, so a Stop costs at most the request in flight"
        );
    }

    #[tokio::test]
    async fn paginate_until_that_is_never_cancelled_matches_paginate() {
        // The probe must not change the shape of an ordinary walk — a complete listing has to keep
        // reporting itself complete, or every uncancelled sync silently stops deleting.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let (out, truncated) = paginate_until(10, &never_cancelled, |_cursor| {
            let seen = seen.clone();
            async move {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Error>((vec![n], (n < 2).then(|| format!("page-{}", n + 1))))
            }
        })
        .await
        .unwrap();

        assert!(!truncated, "nothing was cancelled and the token cleared");
        assert_eq!(out, vec![0, 1, 2], "every page's items, in page order");
    }

    #[tokio::test]
    async fn paginate_until_cancelled_before_the_first_page_fetches_nothing() {
        // A Stop pressed during an earlier account's walk leaves the flag set for the rest of the run:
        // every later listing must unwind immediately rather than fetching one more page each. Still
        // `truncated` — an empty listing that claimed to be complete would delete the whole corpus.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let (out, truncated) = paginate_until(10, &|| true, |_cursor| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Error>((vec![0usize], None))
            }
        })
        .await
        .unwrap();

        assert!(out.is_empty(), "nothing was listed");
        assert!(
            truncated,
            "and an empty listing must never read as complete"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no request was made at all"
        );
    }

    #[test]
    fn drop_without_a_clean_handoff_still_clears_running() {
        // The F-43 regression: a pass that panics (or returns early) never reaches `pass_complete`.
        // The guard's `Drop` must still clear `running`, or the connector wedges for the session.
        let slot = Mutex::new(FakeSlot::default());
        {
            let _guard = SyncRunGuard::claim(&slot, None).unwrap().unwrap();
            assert!(slot.lock().unwrap().running);
            // deliberately no `pass_complete` — this is the panic / early-return path
        }
        assert!(
            !slot.lock().unwrap().running,
            "Drop released the slot even without a clean handoff"
        );
    }
}
