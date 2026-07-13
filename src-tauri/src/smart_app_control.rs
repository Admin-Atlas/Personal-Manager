// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Windows **Smart App Control** (SAC) state, read for the auto-updater UI.
//!
//! SAC (Windows 11) is a hard allowlist: in *enforcement* mode it runs an app only if
//! Microsoft's cloud vouches for it OR it carries an Authenticode signature chaining to a
//! Trusted Root CA. There is **no per-app "Run anyway"** override. Our Windows installer is
//! an unsigned NSIS `*-setup.exe` (the updater's minisign key is not an Authenticode cert),
//! so SAC silently blocks it — and the stock `tauri-plugin-updater` applies an update by
//! calling `ShellExecuteW(setup.exe)` then `std::process::exit(0)` **without checking the
//! launch result**. The block is therefore invisible: the app closes and reopens on the old
//! version with no error. The updater banner reads this state to warn *before* offering a
//! restart that would silently no-op (see `src/lib/useUpdater.ts`).
//!
//! State is a single registry DWORD, readable from our non-elevated backend:
//! `HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy` → `VerifiedAndReputablePolicyState`
//! (Microsoft's SAC testing docs: 0 = Off, 1 = On/Enforced, 2 = Evaluation).

use serde::Serialize;

/// Smart App Control enforcement state, as reported by the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SmartAppControlState {
    /// Not running — the installer launches normally (worst case a dismissible SmartScreen
    /// prompt the user can click through).
    Off,
    /// Enforcing — an unsigned installer is blocked outright, with no user override.
    Enforced,
    /// Evaluating in the background; it may or may not block.
    Evaluation,
    /// Not Windows, the key is absent (SAC never present on this build), or the read failed
    /// — treated as "no warning needed" so the UI fails open.
    Unknown,
}

/// Pure mapping from the `VerifiedAndReputablePolicyState` DWORD to our enum. Any value
/// outside the documented 0/1/2 set is reported as `Unknown` rather than guessed at.
///
/// Only the Windows `query_state` (and the tests) call this, so it is compiled just there —
/// otherwise the non-Windows lib target would flag it `dead_code` under `clippy -D warnings`.
#[cfg(any(target_os = "windows", test))]
fn state_from_dword(value: u32) -> SmartAppControlState {
    match value {
        0 => SmartAppControlState::Off,
        1 => SmartAppControlState::Enforced,
        2 => SmartAppControlState::Evaluation,
        _ => SmartAppControlState::Unknown,
    }
}

/// The machine's current Smart App Control state (`Unknown` off-Windows).
pub fn state() -> SmartAppControlState {
    query_state()
}

#[cfg(target_os = "windows")]
fn query_state() -> SmartAppControlState {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};

    let mut data: u32 = 0;
    let mut size: u32 = std::mem::size_of::<u32>() as u32;
    // SAFETY: `data`/`size` describe a valid, initialised DWORD-sized buffer; RRF_RT_REG_DWORD
    // constrains the read to a REG_DWORD so nothing larger can be written back. This key is
    // world-readable (BUILTIN\Users : ReadKey) — no elevation required.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Control\\CI\\Policy"),
            w!("VerifiedAndReputablePolicyState"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if status == ERROR_SUCCESS {
        state_from_dword(data)
    } else {
        // Value/key absent (SAC never present) or a read error: report Unknown so the UI
        // never warns a machine that has no Smart App Control.
        SmartAppControlState::Unknown
    }
}

#[cfg(not(target_os = "windows"))]
fn query_state() -> SmartAppControlState {
    // Smart App Control is a Windows-only feature; nothing to gate elsewhere.
    SmartAppControlState::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_documented_dword_values() {
        assert_eq!(state_from_dword(0), SmartAppControlState::Off);
        assert_eq!(state_from_dword(1), SmartAppControlState::Enforced);
        assert_eq!(state_from_dword(2), SmartAppControlState::Evaluation);
    }

    #[test]
    fn unexpected_values_are_unknown() {
        assert_eq!(state_from_dword(3), SmartAppControlState::Unknown);
        assert_eq!(state_from_dword(u32::MAX), SmartAppControlState::Unknown);
    }

    #[test]
    fn serializes_to_lowercase_string() {
        assert_eq!(
            serde_json::to_string(&SmartAppControlState::Enforced).unwrap(),
            "\"enforced\""
        );
        assert_eq!(
            serde_json::to_string(&SmartAppControlState::Off).unwrap(),
            "\"off\""
        );
        assert_eq!(
            serde_json::to_string(&SmartAppControlState::Unknown).unwrap(),
            "\"unknown\""
        );
    }
}
