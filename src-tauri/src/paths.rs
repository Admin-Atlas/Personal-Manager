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

/// The WebView2 (Edge runtime) user-data folder Tauri gives the app on Windows:
/// `%LOCALAPPDATA%\<identifier>` (holding `EBWebView/` — the webview cache + this app's
/// `localStorage`/IndexedDB). It sits OUTSIDE the "Personal Manager" data dir, under the
/// bundle identifier, so a "remove PM completely" flow has to clear it too. The identifier is
/// fixed (`org.itsatlas.pm`; renaming it orphans the keychain — see [`data_dir`]).
///
/// **Right on Windows AND Linux; wrong on macOS.** Tauri forces a webview data directory at
/// `<local data>/<identifier>` for *both* those platforms (its own comment says "windows", its
/// `cfg` says `linux, windows`) and creates it on every launch — and on Linux wry then points
/// WebKitGTK's `base_data_directory` AND `base_cache_directory` at exactly that path, so
/// `~/.local/share/<identifier>` really is this app's `localStorage`, IndexedDB, service workers
/// and cookie jar. macOS is the exception: the `cfg` excludes it, so WKWebView uses its default
/// store under `~/Library/WebKit/<identifier>` and this path is a near-empty vestige. Treating
/// this as "the webview folder" on macOS is what left a Mac's `localStorage` intact through a full
/// wipe — see [`os_app_leftovers`], which resolves the right answer per platform.
pub fn webview_data_dir(app: &AppHandle) -> Result<PathBuf> {
    let base = app
        .path()
        .local_data_dir()
        .map_err(|e| Error::Other(format!("could not resolve local data dir: {e}")))?;
    Ok(base.join(&app.config().identifier))
}

/// Everything macOS writes on PM's behalf OUTSIDE the data dir, keyed by the bundle identifier.
///
/// **Why this list has to exist at all.** [`webview_data_dir`] resolves to
/// `~/Library/Application Support/<identifier>` on macOS — and its doc describes the *Windows*
/// WebView2 layout (`%LOCALAPPDATA%\<identifier>\EBWebView`) as though it were universal. It is not:
/// WKWebView keeps this app's `localStorage` under `~/Library/WebKit/<identifier>/WebsiteData`, and
/// the OS scatters caches, cookies, saved window state and the `NSUserDefaults` plist across four
/// more places. None of them was ever removed, so "Remove PM data" left the dev-mode flag and the
/// last-seen-version marker behind and a reinstall did not look like a fresh install.
///
/// Pure and separated from the [`AppHandle`] so the path shapes — the part that can be wrong, and
/// the part a Windows CI can still check — are unit-tested. Order is stable for the tests; the
/// caller removes best-effort, so a path that does not exist is simply skipped.
pub fn macos_leftovers_in(home: &Path, identifier: &str) -> Vec<PathBuf> {
    let library = home.join("Library");
    vec![
        // PM-owned, and where the (Windows-only) uninstall marker used to be dropped.
        library.join("Application Support").join(identifier),
        // The WKWebView store: localStorage, IndexedDB, service workers. The actual reason a
        // reinstall remembered dev mode.
        library.join("WebKit").join(identifier),
        library.join("Caches").join(identifier),
        library.join("HTTPStorages").join(identifier),
        library
            .join("Preferences")
            .join(format!("{identifier}.plist")),
        library
            .join("Saved Application State")
            .join(format!("{identifier}.savedState")),
        // The cookie jar is a FILE beside the HTTPStorages directory, not inside it, so removing
        // the directory alone leaves it.
        library
            .join("HTTPStorages")
            .join(format!("{identifier}.binarycookies")),
    ]
}

/// Everything Linux writes on PM's behalf outside the data dir: one directory, and it is the
/// important one.
///
/// Tauri forces the webview data directory to `<local data>/<identifier>` on Linux as well as
/// Windows and creates it on every launch, and wry hands that same path to WebKitGTK as both its
/// data and its cache base — so this holds `localStorage`, IndexedDB, service-worker registrations,
/// the DOM cache and the cookie file. Unlike Windows there is no uninstaller to sweep it and no
/// package maintainer script that touches it, so if the wipe skips it nothing ever removes it.
///
/// Kept as a list of one, and pure like its macOS sibling, so both platforms take the same path
/// through [`os_app_leftovers`] and the shapes stay unit-tested from any CI host.
pub fn linux_leftovers_in(local_data: &Path, identifier: &str) -> Vec<PathBuf> {
    vec![local_data.join(identifier)]
}

/// Everything the OS writes on PM's behalf OUTSIDE the data dir, resolved for the running platform.
/// Empty on Windows — there the webview folder is in use while PM runs and cannot be removed by the
/// app itself, so the NSIS uninstaller purges it from outside instead (see [`UNINSTALL_PURGE_MARKER`]).
pub fn os_app_leftovers(app: &AppHandle) -> Vec<PathBuf> {
    let identifier = &app.config().identifier;
    if cfg!(target_os = "macos") {
        return match app.path().home_dir() {
            Ok(home) => macos_leftovers_in(&home, identifier),
            Err(_) => Vec::new(),
        };
    }
    if cfg!(target_os = "linux") {
        return match app.path().local_data_dir() {
            Ok(base) => linux_leftovers_in(&base, identifier),
            Err(_) => Vec::new(),
        };
    }
    Vec::new()
}

/// Filename of the marker a full "remove PM completely" wipe drops in [`webview_data_dir`] so the
/// NSIS uninstaller's post-uninstall hook knows to purge the leftover data + webview folders (a
/// normal uninstall leaves user data for a reinstall). It lives in the webview folder — which the
/// running app can't delete but the uninstaller can, after PM has exited — so it survives the app
/// deleting its own data dir. Cleared on every normal boot ([`clear_stale_uninstall_purge_marker`])
/// so a cancelled uninstall can never purge a later, still-wanted install.
pub const UNINSTALL_PURGE_MARKER: &str = ".pm-uninstall-purge";

/// Path to the full-uninstall purge marker (see [`UNINSTALL_PURGE_MARKER`]).
pub fn uninstall_purge_marker(app: &AppHandle) -> Result<PathBuf> {
    Ok(webview_data_dir(app)?.join(UNINSTALL_PURGE_MARKER))
}

/// Delete a stale full-uninstall purge marker at boot. It's only meant to bridge the seconds
/// between a full wipe and the uninstaller running; if the app is booting normally the user kept
/// (or reinstalled) PM, so any leftover marker must go or a future *ordinary* uninstall would
/// wrongly purge their data. Best-effort — an absent marker or unreadable folder is a no-op.
pub fn clear_stale_uninstall_purge_marker(app: &AppHandle) {
    if let Ok(marker) = uninstall_purge_marker(app) {
        let _ = std::fs::remove_file(marker);
    }
}

/// Walk up from `start`, returning the `sidecar/` folder of the nearest ancestor that holds a
/// `pm_sidecar.py`. This is the dev fallback: the binary sits under `src-tauri/target/<profile>/`
/// and the repo root above it holds `sidecar/`. Split out of [`sidecar_source_dir`] so the walk
/// is testable without a real executable path or app bundle. **Debug-only (M-5):** a release build
/// never walks `current_exe()` ancestors, so a planted `sidecar/pm_sidecar.py` above the installed
/// binary can't be picked up and run.
#[cfg(debug_assertions)]
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
/// `PM_SIDECAR_DIR` (dev override, debug builds only) → the bundled resource dir → (debug builds
/// only) walking up from the executable for the repo's `sidecar/` folder. In a **release** build a
/// missing bundled resource hard-errors rather than walking `current_exe()` ancestors (M-5).
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

    // Dev only (M-5): the binary sits under src-tauri/target/<profile>/; the repo root above it holds
    // `sidecar/`. Gated to debug builds so a release install with a missing/corrupt bundled resource
    // hard-errors here instead of walking `current_exe()` ancestors — on Windows that walk can reach
    // `C:\`, where Authenticated Users may create folders, letting a planted `C:\sidecar\pm_sidecar.py`
    // run as the user.
    #[cfg(debug_assertions)]
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
