// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The per-profile pointer to where this profile's vault lives (spec §5). It is a
//! tiny, non-secret file in the profile's own data dir — it CAN'T live in the
//! encrypted DB, because we need it to find the DB in the first place, and it
//! CAN'T live in the shared vault folder, because each profile points at that
//! folder independently (and caches its own key). Absent ⇒ the default location
//! (today's behaviour), so a plain device-only install needs no pointer at all.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Filename of the per-profile pointer, stored in the profile's data dir.
pub const POINTER_FILENAME: &str = "vault-pointer.json";

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

fn pointer_path(data_dir: &Path) -> PathBuf {
    data_dir.join(POINTER_FILENAME)
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
        Err(e) => Err(e.into()),
    }
}

/// Write the pointer atomically (temp file in the same dir, then rename).
pub fn store(data_dir: &Path, pointer: &VaultPointer) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = pointer_path(data_dir);
    let json = serde_json::to_vec_pretty(pointer)
        .map_err(|e| Error::Other(format!("could not encode {POINTER_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove the pointer (reverting this profile to the default location).
pub fn clear(data_dir: &Path) -> Result<()> {
    let path = pointer_path(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
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
}
