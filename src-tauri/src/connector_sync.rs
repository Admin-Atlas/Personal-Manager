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
//! engines call identically: [`run_detached_sync`] (the single-flight + crash-resume-marker + rerun
//! loop that wraps one connector's pass fn), [`apply_connector_actions`] (the blocking index-only
//! executor — one copy for all three, formerly duplicated per connector), and [`action_category`] /
//! [`ActionKind`] (the tally bucket for a pass's reducer actions).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::{db, index_only, AppState};

/// The single-flight fields every detached-sync snapshot carries, so [`SyncRunGuard`] can own the
/// `running`/`rerun` lifecycle without knowing the concrete snapshot type. Implemented by the three
/// `*SyncState` snapshots in [`crate`] (each also carries connector-specific display fields — the
/// counters, the target, the last report — that the guard never touches).
pub trait SyncSlot {
    /// Whether a sync is in flight right now.
    fn running(&self) -> bool;
    fn set_running(&mut self, running: bool);
    /// The coalescing flag: a sync was requested while one was already running, so the in-flight pass
    /// owes one more all-targets sweep to pick the request up.
    fn rerun(&self) -> bool;
    fn set_rerun(&mut self, rerun: bool);
    /// Reset the per-pass display counters + target for that follow-up sweep. The rerun path folds a
    /// mid-run request into one more pass over *everything*, so the specific target is dropped.
    fn reset_for_rerun(&mut self);
    /// Reset the snapshot to the start of a fresh pass: `running` set, `target` recorded (the account
    /// email / folder key, or `None` for an all-targets pass), every per-pass counter and the last
    /// report cleared. Called once by [`run_detached_sync`] right after the slot is claimed.
    fn begin_pass(&mut self, target: Option<String>);
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
    /// Try to claim the single-flight slot.
    ///
    /// - `Ok(Some(guard))` — the slot was free; `running` is now set and stays ours until the guard
    ///   drops.
    /// - `Ok(None)` — a sync is already running; a rerun has been marked so the in-flight pass sweeps
    ///   once more, and the caller should return without starting a second, racing sync.
    pub fn claim(slot: &'a Mutex<S>) -> Result<Option<Self>> {
        let mut s = slot
            .lock()
            .map_err(|_| Error::Other("sync state poisoned".into()))?;
        if s.running() {
            s.set_rerun(true);
            return Ok(None);
        }
        s.set_running(true);
        // A fresh claim owes no rerun yet; clear any left orphaned by a prior panicked pass.
        s.set_rerun(false);
        Ok(Some(Self { slot }))
    }

    /// End-of-pass handoff. Call once after each pass with whether the user asked to stop.
    ///
    /// Returns `true` when another sweep is owed — a rerun was requested and we weren't stopped — in
    /// which case `running` stays set and the per-pass counters are reset for the next pass. Returns
    /// `false` otherwise, having cleared `running`. The rerun check and the `running` clear happen
    /// under one lock, so a request landing exactly as a pass finishes is never lost to a race
    /// against the clear: it either re-arms this guard or starts cleanly once `running` is false.
    pub fn pass_complete(&self, stopped: bool) -> Result<bool> {
        let mut s = self
            .slot
            .lock()
            .map_err(|_| Error::Other("sync state poisoned".into()))?;
        if s.rerun() && !stopped {
            s.set_rerun(false);
            s.reset_for_rerun();
            Ok(true)
        } else {
            s.set_running(false);
            Ok(false)
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
/// fold into the running pass's follow-up sweep and return), reset the progress snapshot for this
/// run, clear the stop flag, persist a crash-resume marker, then run `pass` once — rerunning it while
/// the guard reports another sweep is owed — and finally drop the marker on the clean exit.
///
/// `pass` is the connector's own one-pass fn (`run_drive_sync` / `run_onedrive_sync` /
/// `run_local_sync`); `slot`, `cancel`, and `pending_key` are that connector's `AppState` fields.
/// This is the body the three `*_sync_core` wrappers shared byte-for-byte except for those four
/// values — hoisting it keeps the F-43 guard handling and the crash-resume marking in one place.
pub async fn run_detached_sync<F, Fut, S>(
    st: &AppState,
    slot: &Mutex<S>,
    cancel: &AtomicBool,
    pending_key: &str,
    target: Option<String>,
    pass: F,
) -> Result<usize>
where
    S: SyncSlot,
    F: Fn(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<usize>>,
{
    // Claim the single-flight slot, or fold this request into the running pass's follow-up sweep. The
    // guard clears `running` on drop — including if a pass panics — so a crashed sync can't wedge the
    // connector with `running = true` for the rest of the session (F-43).
    let Some(guard) = SyncRunGuard::claim(slot)? else {
        return Ok(0);
    };
    // Reset the snapshot for this run (the guard already holds `running`).
    {
        let mut snap = slot
            .lock()
            .map_err(|_| Error::Other("sync state poisoned".into()))?;
        snap.begin_pass(target.clone());
    }
    // Fresh stop flag for this run; persist a crash-resume marker (cleared on the clean exit below).
    {
        cancel.store(false, Ordering::SeqCst);
        if let Ok(conn) = st.conn() {
            let marker = serde_json::to_string(&target).unwrap_or_else(|_| "null".to_string());
            let _ = db::set_setting(&conn, pending_key, &marker);
        }
    }

    let mut pass_target = target;
    let mut result;
    loop {
        result = pass(pass_target).await;
        let stopped = cancel.load(Ordering::SeqCst);
        // The guard drains `rerun` and clears `running` under one lock, so a request landing exactly
        // as we finish isn't lost to a race against clearing `running`.
        if !guard.pass_complete(stopped)? {
            break;
        }
        pass_target = None;
    }

    // Clean exit (finished or stopped): drop the crash-resume marker so launch doesn't re-run it.
    {
        if let Ok(conn) = st.conn() {
            let _ = db::delete_setting(&conn, pending_key);
        }
    }
    result
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

/// Run a reducer's actions against the store + manifest, on a blocking thread (the index-only
/// executor embeds via the sidecar). One copy for all three connectors — Drive, OneDrive, and the
/// local folder ran byte-for-byte identical copies before this consolidation. Mirrors
/// `dev_apply_change_event`'s execution shape.
pub fn apply_connector_actions(
    app: &AppHandle,
    actions: &[index_only::Action],
    fetched: Option<index_only::PointerInput>,
) -> Result<()> {
    let state = app.state::<AppState>();
    let (vault_root, cipher) = state.manifest_io()?;
    // Gentle mode caps the embedding batch to bound peak memory. This runs once per item, so reading
    // it here also makes gentle batching engage mid-sync (alongside the per-item pause).
    let gateway = {
        let conn = state.conn()?;
        state
            .gateway_for_write(&conn)?
            .with_embed_batch(db::indexing_embed_batch(&conn))
    };
    index_only::apply_actions(
        state.inner(),
        &gateway,
        &vault_root,
        &cipher,
        actions,
        fetched.as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal snapshot standing in for the real `*SyncState`s — same single-flight fields, plus a
    /// stand-in target/counters so `reset_for_rerun` has something to clear.
    #[derive(Default)]
    struct FakeSlot {
        running: bool,
        rerun: bool,
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
        fn rerun(&self) -> bool {
            self.rerun
        }
        fn set_rerun(&mut self, rerun: bool) {
            self.rerun = rerun;
        }
        fn reset_for_rerun(&mut self) {
            self.processed = 0;
            self.total = None;
            self.target = None;
        }
        fn begin_pass(&mut self, target: Option<String>) {
            *self = FakeSlot {
                running: true,
                target,
                ..Default::default()
            };
        }
    }

    #[test]
    fn claim_takes_a_free_slot_and_marks_running() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot).unwrap();
        assert!(guard.is_some(), "a free slot is claimable");
        assert!(slot.lock().unwrap().running);
    }

    #[test]
    fn claim_folds_a_second_request_into_a_rerun() {
        let slot = Mutex::new(FakeSlot {
            running: true,
            ..Default::default()
        });
        let second = SyncRunGuard::claim(&slot).unwrap();
        assert!(second.is_none(), "a busy slot can't be claimed twice");
        assert!(
            slot.lock().unwrap().rerun,
            "the in-flight pass is told to sweep once more"
        );
    }

    #[test]
    fn pass_complete_clears_running_when_no_rerun() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot).unwrap().unwrap();
        assert!(!guard.pass_complete(false).unwrap(), "nothing owed → done");
        assert!(!slot.lock().unwrap().running);
    }

    #[test]
    fn pass_complete_rearms_and_resets_when_rerun_pending() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot).unwrap().unwrap();
        {
            let mut s = slot.lock().unwrap();
            s.rerun = true;
            s.processed = 7;
            s.total = Some(7);
            s.target = Some("acc-a".into());
        }
        assert!(
            guard.pass_complete(false).unwrap(),
            "a pending rerun owes another sweep"
        );
        let s = slot.lock().unwrap();
        assert!(s.running, "running stays set across the follow-up sweep");
        assert!(!s.rerun, "the rerun was drained");
        assert_eq!(s.processed, 0);
        assert_eq!(s.total, None);
        assert_eq!(
            s.target, None,
            "the follow-up sweeps all targets, not the prior one"
        );
    }

    #[test]
    fn a_stop_beats_a_pending_rerun() {
        let slot = Mutex::new(FakeSlot::default());
        let guard = SyncRunGuard::claim(&slot).unwrap().unwrap();
        slot.lock().unwrap().rerun = true;
        assert!(
            !guard.pass_complete(true).unwrap(),
            "a stop request wins over a pending rerun"
        );
        assert!(!slot.lock().unwrap().running);
    }

    #[test]
    fn begin_pass_sets_running_and_target_and_clears_stale_counters() {
        // run_detached_sync calls begin_pass right after claim to reset the snapshot for a fresh run.
        let mut slot = FakeSlot {
            running: true,
            processed: 9,
            total: Some(9),
            target: Some("stale".into()),
            rerun: true,
        };
        slot.begin_pass(Some("acc-a".into()));
        assert!(slot.running, "the run holds the slot");
        assert_eq!(slot.target.as_deref(), Some("acc-a"), "target recorded");
        assert_eq!(slot.processed, 0, "stale counters cleared");
        assert_eq!(slot.total, None);
        assert!(!slot.rerun, "no rerun owed at the start of a fresh pass");
    }

    #[test]
    fn drop_without_a_clean_handoff_still_clears_running() {
        // The F-43 regression: a pass that panics (or returns early) never reaches `pass_complete`.
        // The guard's `Drop` must still clear `running`, or the connector wedges for the session.
        let slot = Mutex::new(FakeSlot::default());
        {
            let _guard = SyncRunGuard::claim(&slot).unwrap().unwrap();
            assert!(slot.lock().unwrap().running);
            // deliberately no `pass_complete` — this is the panic / early-return path
        }
        assert!(
            !slot.lock().unwrap().running,
            "Drop released the slot even without a clean handoff"
        );
    }
}
