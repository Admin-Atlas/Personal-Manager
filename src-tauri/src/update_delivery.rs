// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How this build receives updates — specifically, whether it's a Linux **package**
//! install (rpm/deb) that the in-app updater can't apply to.
//!
//! Tauri's updater replaces a running **AppImage** in place (it finds it via the `APPIMAGE`
//! env var the AppImage runtime sets). A package install (rpm/deb) sets no such variable, so
//! `install()` has nothing to swap and fails. But `check()` compares versions only, so a
//! package install is still *offered* the AppImage update and would download the whole thing
//! before failing. The updater UI reads this to skip that dead path and point package users at
//! reinstalling instead (see `src/lib/useUpdater.ts`).
//!
//! `sidecar.rs` independently reads the same AppImage signal for an unrelated purpose (locating
//! the bundled Python); this module keeps its own tiny, unit-tested predicate rather than couple
//! the two.

/// Pure decision: is this a package-managed Linux install (rpm/deb)?
/// `is_linux` = compiled for Linux; `from_appimage` = launched from a mounted AppImage.
pub fn is_package_managed_linux(is_linux: bool, from_appimage: bool) -> bool {
    is_linux && !from_appimage
}

/// Whether the running app is a Linux package install the in-app updater can't apply to.
/// False on Windows, macOS, and the Linux AppImage (self-update works on all three).
pub fn package_managed_linux() -> bool {
    // The AppImage runtime sets both of these; either one present means we're an AppImage.
    let from_appimage = std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty())
        || std::env::var_os("APPDIR").is_some_and(|v| !v.is_empty());
    is_package_managed_linux(cfg!(target_os = "linux"), from_appimage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_appimage_is_not_package_managed() {
        assert!(!is_package_managed_linux(true, true));
    }

    #[test]
    fn linux_without_appimage_is_package_managed() {
        assert!(is_package_managed_linux(true, false));
    }

    #[test]
    fn non_linux_is_never_package_managed() {
        assert!(!is_package_managed_linux(false, false));
        assert!(!is_package_managed_linux(false, true));
    }
}
