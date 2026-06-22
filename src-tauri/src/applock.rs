// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Optional biometric / OS app-lock — a **soft UI gate** (opt-in, off by default).
//!
//! This does **not** gate the database key. The encrypted store still unlocks at
//! launch (see `lib.rs` → `secrets::get_or_create_db_key` → `db::open`); this only
//! withholds the *window* until the OS verifies the user (Windows Hello / Touch ID).
//! It's a convenience lock against a walk-up, not a second crypto layer — the data
//! is already decrypted in memory by the time the lock screen shows. Hard key-gating
//! (withhold the DB key until verification succeeds) is documented as deferred to v4
//! in `docs/DECISIONS.md`.

use crate::error::{Error, Result};

/// Pure policy: should the UI be locked right now? Locked iff the feature is enabled
/// *and* the user hasn't verified yet this session. Kept separate from the OS call so
/// the decision is unit-testable without a real biometric prompt.
pub fn should_lock(enabled: bool, verified_this_session: bool) -> bool {
    enabled && !verified_this_session
}

// --- Windows: Windows Hello via UserConsentVerifier ---------------------------------

#[cfg(target_os = "windows")]
mod platform {
    use super::{Error, Result};

    /// Run `f` on a fresh thread initialised into the COM **multi-threaded** apartment.
    ///
    /// The blocking `IAsyncOperation::get()` waits on a Win32 event with no message
    /// pump; on the STA UI thread that would deadlock (the completion can't be
    /// delivered to a blocked STA). An MTA worker thread delivers the completion to a
    /// pool thread, which signals the event — no pump needed. COM init is balanced with
    /// `CoUninitialize` on every return path via the guard.
    fn on_mta_thread<F, T>(f: F) -> Result<T>
    where
        F: FnOnce() -> Result<T> + Send,
        T: Send,
    {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    // SAFETY: balanced CoInitialize/CoUninitialize on this thread only.
                    unsafe {
                        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
                        if hr.is_err() {
                            return Err(Error::Other(format!(
                                "failed to initialise COM for biometric verification ({hr:?})"
                            )));
                        }
                    }
                    struct ComGuard;
                    impl Drop for ComGuard {
                        fn drop(&mut self) {
                            // SAFETY: matched with the CoInitializeEx above on this thread.
                            unsafe { CoUninitialize() };
                        }
                    }
                    let _com = ComGuard;
                    f()
                })
                .join()
                .map_err(|_| Error::Other("biometric verification thread panicked".into()))?
        })
    }

    /// True when the OS can perform a user-presence check (a Windows Hello PIN or
    /// biometric is enrolled). Gates the Settings toggle so the lock can't be enabled
    /// on a device that could never satisfy it (which would strand the user behind it).
    pub fn available() -> bool {
        use windows::Security::Credentials::UI::{
            UserConsentVerifier, UserConsentVerifierAvailability,
        };
        on_mta_thread(|| {
            let availability = UserConsentVerifier::CheckAvailabilityAsync()
                .map_err(win_err)?
                .get()
                .map_err(win_err)?;
            Ok(availability == UserConsentVerifierAvailability::Available)
        })
        .unwrap_or(false)
    }

    /// Prompt Windows Hello, parented to the given window. `Ok(true)` only on a
    /// successful verification; `Ok(false)` when the user cancels/fails; `Err` when the
    /// verifier can't run at all (so the UI can offer an escape rather than trap the
    /// user — the lock guards the window, not the already-decrypted data).
    pub fn verify(window_handle: isize, message: &str) -> Result<bool> {
        use windows::core::{factory, HSTRING};
        use windows::Security::Credentials::UI::{
            UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
        };
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
        use windows_future::IAsyncOperation;

        on_mta_thread(move || {
            // Availability first, so an unenrolled device gives a clear error instead of
            // a confusing async failure mid-prompt.
            let availability = UserConsentVerifier::CheckAvailabilityAsync()
                .map_err(win_err)?
                .get()
                .map_err(win_err)?;
            if availability != UserConsentVerifierAvailability::Available {
                return Err(Error::Other(format!(
                    "Windows Hello is unavailable on this device ({availability:?})"
                )));
            }

            // Desktop (Win32) apps must use the interop variant that takes an HWND;
            // the windowless `RequestVerificationAsync` only works for UWP.
            let interop =
                factory::<UserConsentVerifier, IUserConsentVerifierInterop>().map_err(win_err)?;
            // SAFETY: `window_handle` is the live main-window HWND captured on the UI
            // thread; the interop call only reads it to parent the system dialog.
            let operation: IAsyncOperation<UserConsentVerificationResult> = unsafe {
                interop
                    .RequestVerificationForWindowAsync(
                        HWND(window_handle as *mut core::ffi::c_void),
                        &HSTRING::from(message),
                    )
                    .map_err(win_err)?
            };
            let result = operation.get().map_err(win_err)?;
            Ok(result == UserConsentVerificationResult::Verified)
        })
    }

    fn win_err(e: windows::core::Error) -> Error {
        Error::Other(format!("biometric verification failed: {e}"))
    }
}

// --- macOS: stub, flagged for implementation on a Mac -------------------------------

#[cfg(target_os = "macos")]
mod platform {
    use super::Result;

    /// macOS Touch ID is not wired yet (untestable from the Windows dev box). Report
    /// unavailable so the Settings toggle stays disabled and the lock can't be enabled
    /// — no risk of stranding a Mac user behind a gate that can't open.
    ///
    /// TODO(mac, deferred): implement via `LAContext.evaluatePolicy(
    /// .deviceOwnerAuthenticationWithBiometrics, ...)` (objc2 / the security framework)
    /// and return real availability + verification. Flagged for the user to add+test.
    pub fn available() -> bool {
        false
    }

    pub fn verify(_window_handle: isize, _message: &str) -> Result<bool> {
        Err(super::Error::Other(
            "biometric app-lock isn't implemented on macOS yet".into(),
        ))
    }
}

// --- Other (Linux dev): no OS primitive wired ---------------------------------------

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    use super::Result;

    /// Dev only (the real targets are Windows/macOS). No Linux biometric primitive is
    /// wired, so treat verification as a no-op pass; the toggle stays usable for UI dev.
    pub fn available() -> bool {
        true
    }

    pub fn verify(_window_handle: isize, _message: &str) -> Result<bool> {
        Ok(true)
    }
}

pub use platform::{available, verify};

#[cfg(test)]
mod tests {
    use super::should_lock;

    #[test]
    fn locks_only_when_enabled_and_unverified() {
        assert!(should_lock(true, false), "enabled + unverified → locked");
        assert!(!should_lock(true, true), "verified this session → unlocked");
        assert!(!should_lock(false, false), "disabled → never locked");
        assert!(!should_lock(false, true), "disabled → never locked");
    }
}
