// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Wires the cooperative vault lock ([`crate::vault::lock`]) into the running app: a
//! heartbeat + cross-process watcher that keeps exactly one instance the *active writer*
//! of a shared vault, hands the baton over when another instance asks, and force-takes a
//! crashed owner (stale heartbeat). It emits `vault://` events so the UI can raise or lift
//! a curtain, and it ties writer-ownership to the open store: when this instance is not
//! the active writer, the store is closed, so a DB command can't race another profile's.
//!
//! Device vaults need none of this (a single profile, guarded by the single-instance
//! plugin), so the session simply stays disengaged for them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::Result;
use crate::vault::{self, lock};
use crate::{AppState, VaultRuntime};

/// How often the watcher ticks (well under the stale threshold, fast enough for a
/// responsive hand-off). Cross-share filesystems give no reliable change notifications,
/// so we poll.
const TICK_INTERVAL_MS: u64 = 1500;

/// This instance's standing toward the shared vault it has engaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockMode {
    /// We hold the baton — the active writer (store open, heartbeat running).
    Active,
    /// Another live instance is active; we're curtained and have not asked to take over.
    Waiting,
    /// We've asked for the baton and are waiting for the holder to release.
    Requesting,
}

/// The lock coordination state for the engaged shared vault (if any).
#[derive(Default)]
pub struct LockSession {
    /// The shared vault folder being coordinated, or `None` for a device/locked vault.
    root: Option<PathBuf>,
    mode: Option<LockMode>,
    /// Our lockfile while Active, so the heartbeat can refresh it in place.
    lock: Option<lock::LockFile>,
    /// Profile label of the other instance, when we're curtained (for the UI).
    other_profile: Option<String>,
    last_heartbeat_ms: u64,
}

/// What the UI needs to draw the curtain: whether this instance is the active writer,
/// whether another live instance holds it, and whether that holder looks crashed.
#[derive(Debug, Clone, Serialize)]
pub struct VaultLockStatus {
    pub active: bool,
    pub contended: bool,
    pub stale: bool,
    pub other_profile: Option<String>,
}

/// Payload for `vault://curtain` — this instance has stepped back from being the writer.
#[derive(Clone, Serialize)]
struct CurtainEvent {
    /// "other-active" (found another writer on open) or "handed-off" (we released on request).
    reason: &'static str,
    other_profile: Option<String>,
}

/// A human label for the lockfile — the OS account, so the other profile is named in the UI.
fn os_profile() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "another profile".to_string())
}

/// Close the store (drop the connection + runtime) — used when this instance is not the
/// active writer, so any DB command fails cleanly rather than racing the other profile.
fn close_store(state: &AppState) {
    let _ = state.take_conn();
    let _ = state.clear_vault_runtime();
}

/// Reopen the store at the current (resolved) location with the cached key — used when we
/// (re)acquire the baton. A passphrase vault's key is cached after the first unlock, so
/// this is silent.
fn reopen_store(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let state = state.inner();
    let resolved = vault::resolve(app)?;
    let Some(meta) = vault::load_meta(&resolved.vault_root)? else {
        return Ok(());
    };
    if let Some((conn, master)) = vault::open_at_boot(&resolved, &meta)? {
        state.open_session(conn, VaultRuntime::build(&resolved, &meta, &master))?;
    }
    Ok(())
}

/// Bring the lock session into line with the current vault, after any open/migration. A
/// shareable vault with the store open engages the lock (acquire, force-take a crashed
/// owner, or step back behind a live one); anything else (device, or still locked)
/// disengages. Idempotent — re-calling while already active here is a no-op.
pub fn engage(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let state = state.inner();
    let id = state.instance_id.clone();

    let resolved = vault::resolve(app)?;
    let meta = vault::load_meta(&resolved.vault_root)?;
    let is_shareable = matches!(
        meta.as_ref().map(|m| m.key_mode),
        Some(vault::KeyMode::Passphrase)
    );

    // Only a shareable vault with the store actually open needs writer coordination.
    if !is_shareable || !state.is_unlocked() {
        disengage(app);
        return Ok(());
    }

    let root = resolved.vault_root.clone();
    {
        let session = state.lock_session.lock().unwrap();
        if session.root.as_deref() == Some(root.as_path()) && session.mode == Some(LockMode::Active)
        {
            return Ok(()); // already the active writer here
        }
    }
    // Moving to a different vault than we held — release the old lock first.
    release_held(state, &id);

    match lock::standing(&root, &id)? {
        lock::Standing::Free | lock::Standing::Owned => {
            let lock = lock::acquire(&root, &id, &os_profile())?;
            set_active(state, root, lock);
        }
        lock::Standing::HeldByStale(_) => {
            // The owner crashed; at open time we take over (its unsaved work, if any, is
            // already lost — the user-facing warned force-take is for the live-then-stale case).
            let lock = lock::force_take(&root, &id, &os_profile())?;
            set_active(state, root, lock);
        }
        lock::Standing::HeldByLive(holder) => {
            close_store(state);
            {
                let mut session = state.lock_session.lock().unwrap();
                session.root = Some(root);
                session.mode = Some(LockMode::Waiting);
                session.lock = None;
                session.other_profile = Some(holder.profile.clone());
            }
            let _ = app.emit(
                "vault://curtain",
                CurtainEvent {
                    reason: "other-active",
                    other_profile: Some(holder.profile),
                },
            );
        }
    }
    Ok(())
}

/// Release any lock we hold and clear the session (a vault became device-only, or closed).
pub fn disengage(app: &AppHandle) {
    let state = app.state::<AppState>();
    let state = state.inner();
    let id = state.instance_id.clone();
    release_held(state, &id);
    let mut session = state.lock_session.lock().unwrap();
    *session = LockSession::default();
}

/// Release the lockfile if the session currently holds one (helper for re-engage/disengage).
fn release_held(state: &AppState, id: &str) {
    let root = {
        let session = state.lock_session.lock().unwrap();
        match (session.root.clone(), session.mode) {
            (Some(root), Some(LockMode::Active)) => Some(root),
            _ => None,
        }
    };
    if let Some(root) = root {
        let _ = lock::release(&root, id);
    }
}

fn set_active(state: &AppState, root: PathBuf, lock: lock::LockFile) {
    let mut session = state.lock_session.lock().unwrap();
    session.root = Some(root);
    session.mode = Some(LockMode::Active);
    session.last_heartbeat_ms = lock.heartbeat_ms;
    session.lock = Some(lock);
    session.other_profile = None;
}

/// The lock status for the UI (see [`VaultLockStatus`]). A device/disengaged vault reports
/// itself active and uncontended — there is no second writer to coordinate with.
pub fn status(app: &AppHandle) -> VaultLockStatus {
    let state = app.state::<AppState>();
    let state = state.inner();
    let id = state.instance_id.clone();
    let (root, mode, other) = {
        let session = state.lock_session.lock().unwrap();
        (
            session.root.clone(),
            session.mode,
            session.other_profile.clone(),
        )
    };
    let Some(root) = root else {
        return VaultLockStatus {
            active: true,
            contended: false,
            stale: false,
            other_profile: None,
        };
    };
    if mode == Some(LockMode::Active) {
        return VaultLockStatus {
            active: true,
            contended: false,
            stale: false,
            other_profile: None,
        };
    }
    // Curtained: report whether the holder is live or looks crashed (offer force-take).
    let stale = matches!(
        lock::standing(&root, &id),
        Ok(lock::Standing::HeldByStale(_))
    );
    VaultLockStatus {
        active: false,
        contended: true,
        stale,
        other_profile: other,
    }
}

/// User chose "Continue here" on the curtain: ask the live holder to hand over (the
/// watcher completes the take-over once it releases). If the holder already vanished, take
/// it now.
pub fn continue_here(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let state = state.inner();
    let id = state.instance_id.clone();
    let root = {
        let session = state.lock_session.lock().unwrap();
        match session.root.clone() {
            Some(root) if session.mode != Some(LockMode::Active) => root,
            _ => return Ok(()), // already active, or no shared vault
        }
    };
    match lock::standing(&root, &id)? {
        lock::Standing::HeldByLive(_) => {
            lock::request_baton(&root, &id)?;
            let mut session = state.lock_session.lock().unwrap();
            session.mode = Some(LockMode::Requesting);
        }
        _ => take_over(app, &root, &id)?,
    }
    Ok(())
}

/// Force-take a crashed owner's lock (the UI shows the "may not have saved" warning first).
pub fn force_take(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let state = state.inner();
    let id = state.instance_id.clone();
    let root = {
        let session = state.lock_session.lock().unwrap();
        match session.root.clone() {
            Some(root) => root,
            None => return Ok(()),
        }
    };
    take_over(app, &root, &id)
}

/// Acquire the lock for this instance, reopen the store, and lift the curtain.
fn take_over(app: &AppHandle, root: &Path, id: &str) -> Result<()> {
    let lock = lock::force_take(root, id, &os_profile())?;
    let state = app.state::<AppState>();
    let state = state.inner();
    reopen_store(app)?;
    set_active(state, root.to_path_buf(), lock);
    let _ = app.emit("vault://acquired", ());
    Ok(())
}

/// Spawn the heartbeat + hand-off watcher. Runs for the life of the app; it is a no-op
/// each tick unless a shared vault is engaged.
pub fn spawn_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(TICK_INTERVAL_MS)).await;
            if let Err(e) = tick(&app) {
                eprintln!("vault: lock watcher tick failed: {e}");
            }
        }
    });
}

/// What a single [`tick`] decides to do from its cheap observations, before any disk effect.
/// Splitting the decision out of `tick` lets the hand-off / heartbeat / take-over policy — the
/// split-brain guard (B1-1) — be unit-tested without a running app. Only the fields relevant to
/// `mode` are consulted (see [`next_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickAction {
    /// Active + another instance asked for the baton → release + ack, close the store, curtain.
    HandOff,
    /// Active + the heartbeat interval elapsed → refresh our lockfile in place.
    RefreshHeartbeat,
    /// Requesting + the holder is gone → take over and lift the curtain.
    TakeOver,
    /// Nothing to do this tick.
    Idle,
}

/// Pure tick policy. `foreign_request` / `heartbeat_due` are the two things an Active writer
/// observes (a filed baton request, and whether its heartbeat is due); `holder_live` is what a
/// Requesting instance observes (does the current holder still look alive). Each is consulted
/// only in its owning mode, so the dummy `false`s the caller passes for the irrelevant ones
/// cannot change the result. A foreign request outranks a due heartbeat — we finish and hand
/// over rather than refresh a lock we're about to release. The heartbeat's *refresh outcome*
/// (kept writing vs. force-taken out from under us) is decided inline in `tick`, since it is
/// only known after `refresh` touches disk.
fn next_action(
    mode: LockMode,
    foreign_request: bool,
    heartbeat_due: bool,
    holder_live: bool,
) -> TickAction {
    match mode {
        LockMode::Active if foreign_request => TickAction::HandOff,
        LockMode::Active if heartbeat_due => TickAction::RefreshHeartbeat,
        LockMode::Active => TickAction::Idle,
        LockMode::Requesting if holder_live => TickAction::Idle,
        LockMode::Requesting => TickAction::TakeOver,
        LockMode::Waiting => TickAction::Idle,
    }
}

/// One watcher tick: refresh our heartbeat and hand the baton over if asked (when Active),
/// or take it once the holder releases (when Requesting). Gathers the observations, lets the
/// pure [`next_action`] pick the move, then applies its effect.
fn tick(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let state = state.inner();
    let id = state.instance_id.clone();
    let (root, mode) = {
        let session = state.lock_session.lock().unwrap();
        (session.root.clone(), session.mode)
    };
    let (Some(root), Some(mode)) = (root, mode) else {
        return Ok(());
    };

    match mode {
        LockMode::Active => {
            // The two Active observations, gathered in the original order (request first).
            let req = lock::read_request(&root)?;
            let foreign_request = req.as_ref().is_some_and(|r| r.requester_instance != id);
            let heartbeat_due = {
                let session = state.lock_session.lock().unwrap();
                lock::now_ms().saturating_sub(session.last_heartbeat_ms)
                    >= lock::HEARTBEAT_INTERVAL_SECS * 1000
            };
            match next_action(mode, foreign_request, heartbeat_due, false) {
                TickAction::HandOff => {
                    // Another instance asked to take over → finish here (we're between
                    // commands), release + ack, close the store, and curtain ourselves.
                    lock::release(&root, &id)?;
                    lock::ack_baton(&root, &id)?;
                    close_store(state);
                    {
                        let mut session = state.lock_session.lock().unwrap();
                        session.mode = Some(LockMode::Waiting);
                        session.lock = None;
                    }
                    let _ = app.emit(
                        "vault://curtain",
                        CurtainEvent {
                            reason: "handed-off",
                            other_profile: req.map(|r| r.requester_instance),
                        },
                    );
                }
                TickAction::RefreshHeartbeat => {
                    // Keep the heartbeat fresh — but re-verify ownership first. If another
                    // profile force-took this vault while we were suspended past the stale
                    // threshold (B1-1), `refresh` reports lost ownership instead of clobbering
                    // the new owner's lockfile; we then step back exactly like a hand-off
                    // (close the store, curtain, go Waiting) rather than running as a second
                    // Active writer.
                    let mut session = state.lock_session.lock().unwrap();
                    let outcome = match session.lock.as_mut() {
                        Some(lock) => Some(lock::refresh(&root, lock)?),
                        None => None,
                    };
                    match outcome {
                        Some(lock::RefreshOutcome::Refreshed) => {
                            if let Some(hb) = session.lock.as_ref().map(|l| l.heartbeat_ms) {
                                session.last_heartbeat_ms = hb;
                            }
                        }
                        Some(lock::RefreshOutcome::LostOwnership(new_owner)) => {
                            drop(session);
                            close_store(state);
                            {
                                let mut session = state.lock_session.lock().unwrap();
                                session.mode = Some(LockMode::Waiting);
                                session.lock = None;
                                session.other_profile = Some(new_owner.profile.clone());
                            }
                            // Reuse the "other-active" curtain: from the user's side this is
                            // the same situation as finding another writer active on open.
                            let _ = app.emit(
                                "vault://curtain",
                                CurtainEvent {
                                    reason: "other-active",
                                    other_profile: Some(new_owner.profile),
                                },
                            );
                        }
                        None => {}
                    }
                }
                // next_action never yields TakeOver for Active (and Idle is a no-op).
                TickAction::TakeOver | TickAction::Idle => {}
            }
        }
        LockMode::Requesting => {
            // The holder released (or crashed) → take over and lift the curtain.
            let holder_live = matches!(lock::standing(&root, &id)?, lock::Standing::HeldByLive(_));
            if next_action(mode, false, false, holder_live) == TickAction::TakeOver {
                take_over(app, &root, &id)?;
            }
        }
        LockMode::Waiting => {} // curtained, user hasn't chosen to continue here
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_hands_off_when_another_instance_requests() {
        // A foreign request wins even when the heartbeat is also due — we release rather than
        // refresh a lock we're about to hand over.
        assert_eq!(
            next_action(LockMode::Active, true, false, false),
            TickAction::HandOff
        );
        assert_eq!(
            next_action(LockMode::Active, true, true, true),
            TickAction::HandOff
        );
    }

    #[test]
    fn active_refreshes_only_when_due_and_unrequested() {
        assert_eq!(
            next_action(LockMode::Active, false, true, false),
            TickAction::RefreshHeartbeat
        );
        assert_eq!(
            next_action(LockMode::Active, false, false, false),
            TickAction::Idle
        );
    }

    #[test]
    fn requesting_takes_over_once_the_holder_is_gone() {
        assert_eq!(
            next_action(LockMode::Requesting, false, false, true),
            TickAction::Idle
        );
        assert_eq!(
            next_action(LockMode::Requesting, false, false, false),
            TickAction::TakeOver
        );
    }

    #[test]
    fn waiting_is_always_idle() {
        // Curtained and passive: nothing this instance observes moves it until the user acts.
        assert_eq!(
            next_action(LockMode::Waiting, true, true, true),
            TickAction::Idle
        );
    }
}
