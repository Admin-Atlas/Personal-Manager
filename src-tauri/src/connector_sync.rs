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

use std::sync::Mutex;

use crate::error::{Error, Result};

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
