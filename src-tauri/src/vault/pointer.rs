// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The per-profile pointer to where this profile's vault lives (spec §5). It is a
//! tiny, non-secret file in the profile's own data dir — it CAN'T live in the
//! encrypted DB, because we need it to find the DB in the first place, and it
//! CAN'T live in the shared vault folder, because each profile points at that
//! folder independently (and caches its own key). Absent ⇒ the default location
//! (today's behaviour), so a plain device-only install needs no pointer at all.
//!
//! Detaching from a shared vault RETIRES the pointer instead of erasing it: the
//! retired file records which folder this profile walked away from, so Settings can
//! offer "rejoin the shared vault at …" later. Knowledge of where the user's data
//! lives must never be destroyed by an escape hatch (the lockout incident: detach
//! left no trace of the only folder that still held the data).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};

/// Filename of the per-profile pointer, stored in the profile's data dir.
pub const POINTER_FILENAME: &str = "vault-pointer.json";
/// Filename of the retired (detached-from) pointer — same dir, last detach wins.
pub const RETIRED_POINTER_FILENAME: &str = "vault-pointer.retired.json";

/// Where this profile's vault root lives. `vault_root` is the portable folder that
/// holds `pm.sqlite`, `vault-meta.json`, and the Markdown — the thing that moves or
/// is shared as one unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultPointer {
    pub schema: u32,
    pub vault_root: PathBuf,
}

impl VaultPointer {
    pub fn new(vault_root: PathBuf) -> Self {
        Self {
            schema: 1,
            vault_root,
        }
    }
}

/// A pointer this profile detached from: the shared folder it used to open, kept so
/// the UI can offer a rejoin. One file, last-detach-wins — a history list is more
/// than the feature needs (adverts already re-offer every other shared vault).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetiredPointer {
    pub schema: u32,
    pub vault_root: PathBuf,
    /// RFC3339; any display goes through the frontend's date formatting.
    pub retired_at: String,
}

fn pointer_path(data_dir: &Path) -> PathBuf {
    data_dir.join(POINTER_FILENAME)
}

fn retired_path(data_dir: &Path) -> PathBuf {
    data_dir.join(RETIRED_POINTER_FILENAME)
}

/// Read the pointer from a profile's data dir. `None` means "use the default
/// location" — the common, zero-config case.
pub fn load(data_dir: &Path) -> Result<Option<VaultPointer>> {
    let path = pointer_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let pointer: VaultPointer = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Other(format!("{POINTER_FILENAME} is unreadable: {e}")))?;
            Ok(Some(pointer))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_at("read PM's vault pointer", &path)(e)),
    }
}

/// Write the pointer atomically (temp file in the same dir, then rename).
pub fn store(data_dir: &Path, pointer: &VaultPointer) -> Result<()> {
    std::fs::create_dir_all(data_dir).map_err(io_at("update PM's vault pointer", data_dir))?;
    let path = pointer_path(data_dir);
    let json = serde_json::to_vec_pretty(pointer)
        .map_err(|e| Error::Other(format!("could not encode {POINTER_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(io_at("update PM's vault pointer", &path))?;
    std::fs::rename(&tmp, &path).map_err(io_at("update PM's vault pointer", &path))?;
    Ok(())
}

/// Remove the pointer (reverting this profile to the default location). Prefer
/// [`retire`] on a user-facing detach — `clear` erases without leaving the rejoin
/// breadcrumb, and is kept for the flows that already know the folder is gone for
/// good (a wipe, a completed rejoin).
pub fn clear(data_dir: &Path) -> Result<()> {
    let path = pointer_path(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_at("update PM's vault pointer", &path)(e)),
    }
}

/// Retire the pointer: record the folder being detached from (atomically, last-wins),
/// then remove the live pointer. Returns the retired record, or `Ok(None)` when there
/// was no pointer to retire. Ordered write-then-clear so a crash in between leaves
/// both files — harmless, since boot follows the live pointer and the retired copy
/// merely repeats it.
pub fn retire(data_dir: &Path) -> Result<Option<RetiredPointer>> {
    let Some(pointer) = load(data_dir)? else {
        return Ok(None);
    };
    let retired = RetiredPointer {
        schema: 1,
        vault_root: pointer.vault_root,
        retired_at: chrono::Utc::now().to_rfc3339(),
    };
    let path = retired_path(data_dir);
    let json = serde_json::to_vec_pretty(&retired)
        .map_err(|e| Error::Other(format!("could not encode {RETIRED_POINTER_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(io_at("record the detached vault", &path))?;
    std::fs::rename(&tmp, &path).map_err(io_at("record the detached vault", &path))?;
    clear(data_dir)?;
    Ok(Some(retired))
}

/// Read the retired pointer, if this profile ever detached from a shared vault.
/// Unreadable-but-present is reported (not swallowed) — the caller decides whether a
/// rejoin offer without a trustworthy record is worth showing.
pub fn load_retired(data_dir: &Path) -> Result<Option<RetiredPointer>> {
    let path = retired_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let retired: RetiredPointer = serde_json::from_slice(&bytes).map_err(|e| {
                Error::Other(format!("{RETIRED_POINTER_FILENAME} is unreadable: {e}"))
            })?;
            Ok(Some(retired))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_at("read the detached-vault record", &path)(e)),
    }
}

/// Remove the retired pointer (idempotent) — a rejoin completed, or a wipe is
/// clearing every trace.
pub fn clear_retired(data_dir: &Path) -> Result<()> {
    let path = retired_path(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_at("update the detached-vault record", &path)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_pointer_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
    }

    #[test]
    fn store_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = VaultPointer::new(PathBuf::from("C:/ProgramData/org.itsatlas.pm/shared"));
        store(dir.path(), &p).unwrap();
        assert_eq!(load(dir.path()).unwrap(), Some(p));
    }

    #[test]
    fn clear_reverts_to_default() {
        let dir = tempfile::tempdir().unwrap();
        store(dir.path(), &VaultPointer::new(PathBuf::from("/somewhere"))).unwrap();
        clear(dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
        // clearing again is fine (idempotent)
        clear(dir.path()).unwrap();
    }

    #[test]
    fn retire_moves_the_pointer_into_the_retired_record() {
        let dir = tempfile::tempdir().unwrap();
        store(dir.path(), &VaultPointer::new(PathBuf::from("/shared/a"))).unwrap();
        let retired = retire(dir.path()).unwrap().expect("had a pointer");
        assert_eq!(retired.vault_root, PathBuf::from("/shared/a"));
        assert!(!retired.retired_at.is_empty());
        // The live pointer is gone; the retired record answers.
        assert_eq!(load(dir.path()).unwrap(), None);
        assert_eq!(load_retired(dir.path()).unwrap(), Some(retired));
    }

    #[test]
    fn retire_without_a_pointer_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(retire(dir.path()).unwrap(), None);
        assert_eq!(load_retired(dir.path()).unwrap(), None);
    }

    #[test]
    fn detach_rejoin_cycles_keep_the_last_root() {
        // store -> retire -> store -> retire must end with the LAST root retired
        // (last-detach-wins), pinning the repeated detach/rejoin cycle behaviour.
        let dir = tempfile::tempdir().unwrap();
        store(dir.path(), &VaultPointer::new(PathBuf::from("/shared/a"))).unwrap();
        retire(dir.path()).unwrap();
        store(dir.path(), &VaultPointer::new(PathBuf::from("/shared/b"))).unwrap();
        retire(dir.path()).unwrap();
        assert_eq!(
            load_retired(dir.path()).unwrap().unwrap().vault_root,
            PathBuf::from("/shared/b")
        );
        // A completed rejoin clears the record; clearing twice stays fine.
        clear_retired(dir.path()).unwrap();
        assert_eq!(load_retired(dir.path()).unwrap(), None);
        clear_retired(dir.path()).unwrap();
    }
}
