// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cooperative single-writer lock for a (possibly shared) vault (spec §5). SQLite is
//! single-writer; when a vault folder is reachable from more than one OS profile, two
//! PM instances can run at once. They share only the folder, so all coordination is
//! file-based here: a heartbeat lockfile says who's active, and a request/ack pair
//! lets a second instance ASK the active one to hand over. The live writer always
//! finishes its write and releases on its own terms — we never seize a live writer;
//! only a crashed one (stale heartbeat) can be force-taken, and only with a warning.
//!
//! This module is the pure mechanism (lockfile + staleness + request/ack files). The
//! async heartbeat task, the cross-process watcher, and the UI curtain are wired on
//! top of it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How often the active instance refreshes its lockfile heartbeat.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 10;
/// A lock whose heartbeat is older than this is treated as abandoned (a crashed
/// owner); only then may a second instance force-take it, and only with a warning.
pub const STALE_AFTER_SECS: u64 = 30;

const LOCK_FILE: &str = "vault.lock";
const REQUEST_FILE: &str = "vault.baton-request";
const ACK_FILE: &str = "vault.baton-ack";

/// Who currently holds the vault, and when they last proved they're alive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockFile {
    /// Random per-process id, so we can tell our own (possibly stale) lock from another's.
    pub instance_id: String,
    /// Human label for the UI (e.g. the OS profile/user name).
    pub profile: String,
    pub pid: u32,
    pub acquired_at_ms: u64,
    pub heartbeat_ms: u64,
}

/// A second instance's request that the active one hand over the baton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatonRequest {
    pub requester_instance: String,
    pub requested_at_ms: u64,
}

/// The active instance's acknowledgement that it has released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatonAck {
    pub by_instance: String,
    pub released_at_ms: u64,
}

/// What a starting instance should do, given the lock it found in the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// No lockfile — acquire it freely.
    Free,
    /// The lockfile is already ours (same instance id).
    Owned,
    /// Another instance holds it with a fresh heartbeat — must hand off (prompt the user).
    HeldByLive(LockFile),
    /// Another instance's lock is stale (it likely crashed) — offer a warned force-take.
    HeldByStale(LockFile),
}

/// Current epoch milliseconds (0 on the impossible pre-1970 clock).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A fresh random instance id for this process run.
pub fn new_instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn lock_path(vault_root: &Path) -> PathBuf {
    vault_root.join(LOCK_FILE)
}
fn request_path(vault_root: &Path) -> PathBuf {
    vault_root.join(REQUEST_FILE)
}
fn ack_path(vault_root: &Path) -> PathBuf {
    vault_root.join(ACK_FILE)
}

/// Write JSON atomically: a temp file beside the target (distinct name per target, so
/// concurrent lock/request/ack writes can't collide), then rename into place.
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::Other(format!("could not encode {}: {e}", path.display())))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|e| {
            Error::Other(format!("could not read {}: {e}", path.display()))
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Is this lock abandoned (heartbeat older than the stale threshold)?
pub fn is_stale(lock: &LockFile, now: u64) -> bool {
    now.saturating_sub(lock.heartbeat_ms) > STALE_AFTER_SECS * 1000
}

/// Classify the lock a starting instance found. Pure, so the policy is unit-tested.
pub fn evaluate(existing: Option<LockFile>, my_instance_id: &str, now: u64) -> Standing {
    match existing {
        None => Standing::Free,
        Some(lock) if lock.instance_id == my_instance_id => Standing::Owned,
        Some(lock) if is_stale(&lock, now) => Standing::HeldByStale(lock),
        Some(lock) => Standing::HeldByLive(lock),
    }
}

pub fn read_lock(vault_root: &Path) -> Result<Option<LockFile>> {
    read_json(&lock_path(vault_root))
}

/// Classify the current standing of the vault folder for `my_instance_id`.
pub fn standing(vault_root: &Path, my_instance_id: &str) -> Result<Standing> {
    Ok(evaluate(read_lock(vault_root)?, my_instance_id, now_ms()))
}

/// Take the lock by writing our lockfile (caller decides this is allowed, via
/// [`standing`]). Returns the lockfile we wrote so the heartbeat can refresh it.
pub fn acquire(vault_root: &Path, instance_id: &str, profile: &str) -> Result<LockFile> {
    let now = now_ms();
    let lock = LockFile {
        instance_id: instance_id.to_string(),
        profile: profile.to_string(),
        pid: std::process::id(),
        acquired_at_ms: now,
        heartbeat_ms: now,
    };
    write_lock(vault_root, &lock)?;
    Ok(lock)
}

fn write_lock(vault_root: &Path, lock: &LockFile) -> Result<()> {
    write_json_atomic(&lock_path(vault_root), lock)
}

/// The result of a heartbeat [`refresh`]: did we still own the lockfile, or did another
/// instance take it while we were Active? The caller relinquishes on the latter rather than
/// clobbering the new owner — a blind overwrite would leave two Active writers on one vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The lockfile was still ours (or absent — no competing owner); the heartbeat was written.
    Refreshed,
    /// A *different* instance owns the lockfile now; we did NOT write. Carries the new owner's
    /// lockfile so the caller can name it in the curtain.
    LostOwnership(LockFile),
}

/// Refresh our heartbeat (called on the heartbeat interval while active). Re-reads the
/// lockfile FIRST and only writes if it is still ours: a plain overwrite would clobber a new
/// owner that legitimately force-took the vault while this instance was suspended past the
/// stale threshold, leaving two Active writers on one folder (split-brain; WAL corruption on
/// a share — B1-1). A missing file means no competing owner, so we re-acquire, as before.
pub fn refresh(vault_root: &Path, lock: &mut LockFile) -> Result<RefreshOutcome> {
    if let Some(current) = read_lock(vault_root)? {
        if current.instance_id != lock.instance_id {
            return Ok(RefreshOutcome::LostOwnership(current));
        }
    }
    lock.heartbeat_ms = now_ms();
    write_lock(vault_root, lock)?;
    Ok(RefreshOutcome::Refreshed)
}

/// Release the lock, but only if it is still ours — never delete another instance's lock.
pub fn release(vault_root: &Path, instance_id: &str) -> Result<()> {
    if let Some(lock) = read_lock(vault_root)? {
        if lock.instance_id == instance_id {
            remove_if_exists(&lock_path(vault_root))?;
        }
    }
    Ok(())
}

/// Force-take a stale (crashed-owner) lock: clear any leftover request/ack and write
/// our own lock. Callers gate this behind the "may not have saved" warning (spec §5.3).
pub fn force_take(vault_root: &Path, instance_id: &str, profile: &str) -> Result<LockFile> {
    clear_baton_files(vault_root)?;
    acquire(vault_root, instance_id, profile)
}

// --- baton request / ack (cross-process hand-off signalling) ---

/// A second instance asks the active one to hand over.
pub fn request_baton(vault_root: &Path, requester_instance: &str) -> Result<()> {
    write_json_atomic(
        &request_path(vault_root),
        &BatonRequest {
            requester_instance: requester_instance.to_string(),
            requested_at_ms: now_ms(),
        },
    )
}

pub fn read_request(vault_root: &Path) -> Result<Option<BatonRequest>> {
    read_json(&request_path(vault_root))
}

/// The active instance acknowledges it has released the lock.
pub fn ack_baton(vault_root: &Path, by_instance: &str) -> Result<()> {
    write_json_atomic(
        &ack_path(vault_root),
        &BatonAck {
            by_instance: by_instance.to_string(),
            released_at_ms: now_ms(),
        },
    )
}

pub fn read_ack(vault_root: &Path) -> Result<Option<BatonAck>> {
    read_json(&ack_path(vault_root))
}

/// Clear both hand-off files (after a completed hand-off or a force-take).
pub fn clear_baton_files(vault_root: &Path) -> Result<()> {
    remove_if_exists(&request_path(vault_root))?;
    remove_if_exists(&ack_path(vault_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_with_heartbeat(id: &str, heartbeat_ms: u64) -> LockFile {
        LockFile {
            instance_id: id.to_string(),
            profile: "tester".to_string(),
            pid: 1234,
            acquired_at_ms: heartbeat_ms,
            heartbeat_ms,
        }
    }

    #[test]
    fn staleness_uses_the_threshold() {
        let now = 1_000_000;
        let fresh = lock_with_heartbeat("a", now - 5_000); // 5s ago
        let old = lock_with_heartbeat("a", now - (STALE_AFTER_SECS * 1000 + 1));
        assert!(!is_stale(&fresh, now));
        assert!(is_stale(&old, now));
    }

    #[test]
    fn evaluate_classifies_every_case() {
        let now = 1_000_000;
        assert_eq!(evaluate(None, "me", now), Standing::Free);
        assert_eq!(
            evaluate(Some(lock_with_heartbeat("me", now)), "me", now),
            Standing::Owned
        );
        let live = lock_with_heartbeat("other", now - 1_000);
        assert_eq!(
            evaluate(Some(live.clone()), "me", now),
            Standing::HeldByLive(live)
        );
        let stale = lock_with_heartbeat("other", now - (STALE_AFTER_SECS * 1000 + 1));
        assert_eq!(
            evaluate(Some(stale.clone()), "me", now),
            Standing::HeldByStale(stale)
        );
    }

    #[test]
    fn acquire_read_release_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(standing(root, "me").unwrap(), Standing::Free);
        let lock = acquire(root, "me", "Alice").unwrap();
        assert_eq!(read_lock(root).unwrap().as_ref(), Some(&lock));
        assert_eq!(standing(root, "me").unwrap(), Standing::Owned);
        release(root, "me").unwrap();
        assert_eq!(read_lock(root).unwrap(), None);
    }

    #[test]
    fn release_never_deletes_another_instances_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let other = acquire(root, "other", "Bob").unwrap();
        // We try to release, but it isn't ours — the lock must remain.
        release(root, "me").unwrap();
        assert_eq!(read_lock(root).unwrap(), Some(other));
    }

    #[test]
    fn refresh_advances_the_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut lock = acquire(root, "me", "Alice").unwrap();
        let original = lock.heartbeat_ms;
        lock.heartbeat_ms = original.saturating_sub(10_000); // pretend it's old
        assert_eq!(refresh(root, &mut lock).unwrap(), RefreshOutcome::Refreshed);
        assert!(lock.heartbeat_ms >= original);
        assert_eq!(
            read_lock(root).unwrap().unwrap().heartbeat_ms,
            lock.heartbeat_ms
        );
    }

    #[test]
    fn refresh_relinquishes_when_another_instance_took_the_lock() {
        // The split-brain guard (B1-1): while we were Active another profile force-took the
        // vault (wrote its own lockfile). Our next heartbeat must NOT overwrite it — it must
        // report lost ownership and leave the new owner's lockfile untouched, so we can step
        // back rather than run as a second Active writer.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut ours = acquire(root, "us", "Alice").unwrap();
        let theirs = force_take(root, "them", "Bob").unwrap();
        match refresh(root, &mut ours).unwrap() {
            RefreshOutcome::LostOwnership(owner) => assert_eq!(owner.instance_id, "them"),
            RefreshOutcome::Refreshed => panic!("must not reclaim another instance's lock"),
        }
        // The on-disk lock is still the new owner's — we did not clobber it.
        assert_eq!(read_lock(root).unwrap(), Some(theirs));
    }

    #[test]
    fn refresh_reacquires_when_the_lockfile_vanished() {
        // No competing owner (the file is simply gone) — re-acquire rather than curtain, so a
        // transient removal doesn't needlessly step us back.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut ours = acquire(root, "us", "Alice").unwrap();
        remove_if_exists(&lock_path(root)).unwrap();
        assert_eq!(refresh(root, &mut ours).unwrap(), RefreshOutcome::Refreshed);
        assert_eq!(read_lock(root).unwrap().unwrap().instance_id, "us");
    }

    #[test]
    fn force_take_clears_handoff_files_and_takes_over() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        acquire(root, "crashed", "Ghost").unwrap();
        request_baton(root, "me").unwrap();
        ack_baton(root, "crashed").unwrap();
        let lock = force_take(root, "me", "Alice").unwrap();
        assert_eq!(lock.instance_id, "me");
        assert_eq!(read_lock(root).unwrap().unwrap().instance_id, "me");
        assert_eq!(read_request(root).unwrap(), None);
        assert_eq!(read_ack(root).unwrap(), None);
    }

    #[test]
    fn baton_request_ack_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(read_request(root).unwrap(), None);
        request_baton(root, "me").unwrap();
        assert_eq!(
            read_request(root).unwrap().unwrap().requester_instance,
            "me"
        );
        ack_baton(root, "owner").unwrap();
        assert_eq!(read_ack(root).unwrap().unwrap().by_instance, "owner");
        clear_baton_files(root).unwrap();
        assert_eq!(read_request(root).unwrap(), None);
        assert_eq!(read_ack(root).unwrap(), None);
    }

    #[test]
    fn lockfile_serde_round_trips() {
        let lock = lock_with_heartbeat("abc", 42);
        let json = serde_json::to_string(&lock).unwrap();
        assert_eq!(serde_json::from_str::<LockFile>(&json).unwrap(), lock);
    }
}
