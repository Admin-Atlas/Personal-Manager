// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};

/// Read a dev-only env override. Honoured only in debug builds; a release build
/// ignores it, so a poisoned environment can't relocate the store or point the
/// sidecar interpreter at an attacker-chosen script directory.
fn dev_override(var: &str) -> Option<std::ffi::OsString> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os(var)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = var;
        None
    }
}

/// The stable directory that holds all of the user's data: the encrypted SQLite
/// store and the Markdown vault. It lives *outside* the app bundle and the repo
/// (spec §7) so updates never wipe it. `PM_DATA_DIR` overrides it in dev builds.
pub fn data_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = match dev_override("PM_DATA_DIR") {
        Some(value) => PathBuf::from(value),
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| Error::Other(format!("could not resolve app data dir: {e}")))?,
    };
    std::fs::create_dir_all(&dir)?;
    // The Markdown vault (source of truth) lives alongside the index; empty in v1.
    std::fs::create_dir_all(dir.join("vault"))?;
    Ok(dir)
}

/// Path to the encrypted SQLite database.
pub fn db_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("pm.sqlite"))
}

/// The Markdown vault — the source of truth (spec §3). Every ingested document
/// is written here and the SQLite index is rebuildable from it.
pub fn vault_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = data_dir(app)?.join("vault");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Where the managed Python venv lives. Regenerable, kept out of the vault so
/// it never gets confused with user data.
pub fn venv_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("runtime").join("venv"))
}

/// Resolve the directory holding the sidecar script + requirements. Order:
/// `PM_SIDECAR_DIR` (dev override, debug builds only) → the bundled resource dir
/// → walking up from the executable for the repo's `sidecar/` folder (dev).
pub fn sidecar_source_dir(app: &AppHandle) -> Result<PathBuf> {
    if let Some(dir) = dev_override("PM_SIDECAR_DIR") {
        return Ok(PathBuf::from(dir));
    }

    if let Ok(resources) = app.path().resource_dir() {
        let candidate = resources.join("sidecar");
        if candidate.join("pm_sidecar.py").exists() {
            return Ok(candidate);
        }
    }

    // Dev: the binary sits under src-tauri/target/<profile>/; the repo root above
    // it holds `sidecar/`.
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors() {
            let candidate = ancestor.join("sidecar");
            if candidate.join("pm_sidecar.py").exists() {
                return Ok(candidate);
            }
        }
    }

    Err(Error::Other(
        "could not locate the sidecar/ folder (set PM_SIDECAR_DIR)".into(),
    ))
}
