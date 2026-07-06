// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::{Path, PathBuf};

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
///
/// We resolve the *machine-local* base (`%LOCALAPPDATA%` on Windows,
/// `~/Library/Application Support` on macOS) and join a human-readable
/// `"Personal Manager"` ourselves, rather than using `app_data_dir()`. Two reasons:
/// the data is large and machine-specific (the store + Python venv) and its
/// decryption key lives in the non-roaming OS keychain, so the local base is the
/// correct home; and the friendly name is far easier for the user to find and back
/// up. The folder name is deliberately decoupled from the bundle identifier
/// (`org.itsatlas.pm`) — the identifier stays fixed because the keychain service is
/// keyed to it (`secrets.rs`), so it must never be renamed.
pub fn data_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = match dev_override("PM_DATA_DIR") {
        Some(value) => PathBuf::from(value),
        None => app
            .path()
            .local_data_dir()
            .map_err(|e| Error::Other(format!("could not resolve local data dir: {e}")))?
            .join("Personal Manager"),
    };
    std::fs::create_dir_all(&dir)?;
    // The Markdown vault (source of truth) lives alongside the index; empty in v1.
    std::fs::create_dir_all(dir.join("vault"))?;
    Ok(dir)
}

/// Where the managed Python venv lives. Regenerable, kept out of the vault so
/// it never gets confused with user data.
pub fn venv_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(data_dir(app)?.join("runtime").join("venv"))
}

/// Walk up from `start`, returning the `sidecar/` folder of the nearest ancestor that holds a
/// `pm_sidecar.py`. This is the dev fallback: the binary sits under `src-tauri/target/<profile>/`
/// and the repo root above it holds `sidecar/`. Split out of [`sidecar_source_dir`] so the walk
/// is testable without a real executable path or app bundle.
fn find_sidecar_up(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join("sidecar");
        if candidate.join("pm_sidecar.py").exists() {
            return Some(candidate);
        }
    }
    None
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
        if let Some(dir) = find_sidecar_up(&exe) {
            return Ok(dir);
        }
    }

    Err(Error::Other(
        "could not locate the sidecar/ folder (set PM_SIDECAR_DIR)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drop a `sidecar/pm_sidecar.py` marker directly under `dir`.
    fn touch_marker(dir: &Path) {
        let sc = dir.join("sidecar");
        std::fs::create_dir_all(&sc).unwrap();
        std::fs::write(sc.join("pm_sidecar.py"), b"# marker").unwrap();
    }

    #[test]
    fn walks_up_to_the_sidecar_dir_above_the_exe() {
        let root = tempfile::tempdir().unwrap();
        touch_marker(root.path());
        // A lexical exe path a few levels below the marker — the dirs need not exist, the walk
        // is purely lexical until it probes for the marker file.
        let exe = root.path().join("target").join("debug").join("pm");
        assert_eq!(
            find_sidecar_up(&exe).unwrap(),
            root.path().join("sidecar"),
            "the walk finds the repo-root sidecar/ above the binary"
        );
    }

    #[test]
    fn finds_no_marker_inside_a_clean_subtree() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("a").join("b").join("pm");
        // The walk is unbounded (it climbs past the temp dir to the filesystem root), so we
        // can't assert `is_none()` without assuming nothing above the temp dir holds a marker.
        // Assert the real invariant instead: with no marker in our own subtree, the walk never
        // surfaces one from inside it.
        if let Some(found) = find_sidecar_up(&exe) {
            assert!(
                !found.starts_with(root.path()),
                "no sidecar/ exists in the clean subtree, but the walk returned {found:?}"
            );
        }
    }

    #[test]
    fn nearest_ancestor_wins_over_a_higher_one() {
        let root = tempfile::tempdir().unwrap();
        let mid = root.path().join("mid");
        std::fs::create_dir_all(&mid).unwrap();
        touch_marker(root.path());
        touch_marker(&mid);
        let exe = mid.join("target").join("debug").join("pm");
        assert_eq!(
            find_sidecar_up(&exe).unwrap(),
            mid.join("sidecar"),
            "the closer sidecar/ shadows the one further up"
        );
    }
}
