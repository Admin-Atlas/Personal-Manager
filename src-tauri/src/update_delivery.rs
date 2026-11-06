// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How this build receives updates — whether it's a Linux **package** install (rpm/deb)
//! that the in-app updater can't apply to, and which macOS it runs on.
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

/// The running macOS version as the system states it (`kern.osproductversion`, e.g. "14.5"),
/// or `None` off macOS or when it can't be read.
///
/// The updater UI compares it with the minimum macOS an offered update states (`latest.json`'s
/// `minimum_macos_version`, see `src/lib/updateGate.ts`) before it downloads anything: macOS
/// refuses to open an app whose minimum is above it, and the updater replaces PM in place, so
/// installing such an update would leave a PM that doesn't open. This read is the only platform
/// call in that decision; the comparison itself is pure and lives beside the other update gates.
pub fn macos_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut buf = [0u8; 64];
        let mut len: libc::size_t = buf.len();
        // SAFETY: the name is a NUL-terminated C string literal; `buf` is a live, writable buffer
        // and `len` holds its size, so the kernel writes at most `len` bytes into it and stores the
        // length it wrote back in `len`. No new value is set (null pointer, zero length).
        let rc = unsafe {
            libc::sysctlbyname(
                c"kern.osproductversion".as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return None;
        }
        sysctl_text(&buf[..len.min(buf.len())])
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// A sysctl string value: the bytes up to its NUL, as trimmed UTF-8, or `None` when that is
/// empty or not text.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn sysctl_text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let text = std::str::from_utf8(&bytes[..end]).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysctl_text_stops_at_the_nul_and_trims() {
        assert_eq!(sysctl_text(b"14.5\0").as_deref(), Some("14.5"));
        assert_eq!(sysctl_text(b"13.6.1\0\0junk").as_deref(), Some("13.6.1"));
        assert_eq!(sysctl_text(b" 15.0 ").as_deref(), Some("15.0"));
        assert_eq!(sysctl_text(b"\0"), None);
        assert_eq!(sysctl_text(b""), None);
        assert_eq!(sysctl_text(b"\xff\xfe\0"), None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn macos_version_is_none_off_macos() {
        assert_eq!(macos_version(), None);
    }

    /// Runs on the `rust-macos` CI job: the real read on a real Mac. A numeric major of 11 or
    /// more is what the updater UI needs; 10.16 would mean the read is in Apple's compatibility
    /// mode, which reports every macOS from 11 on as 10.16.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_version_reads_this_mac() {
        let v = macos_version().expect("kern.osproductversion is readable");
        let major: u32 = v
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
            .unwrap_or_else(|| panic!("{v:?} has no numeric major version"));
        assert!(major >= 11, "{v:?}");
    }

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
