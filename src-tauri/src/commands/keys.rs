// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Secrets the app holds and the posture of the machine it runs on: the OpenRouter keys,
//! the app lock, and the OS/packaging readouts.
//!
//! `unlock_app` is the APP lock — OS user presence, then the app-unlocked flag — and is
//! deliberately not beside `vault_lock_status` / `continue_here` / `force_take_vault` in
//! `vaults`, which are `lock_session`: which *device* currently holds the vault. The pair
//! reads as duplicated and is not; keep them apart.

use tauri::State;

use crate::error::{Error, Result};
use crate::{applock, openrouter, secrets, AppState};

// --- secrets ---

#[tauri::command]
pub fn has_openrouter_key() -> Result<bool> {
    Ok(secrets::get_openrouter_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    secrets::set_openrouter_key(key)
}

#[tauri::command]
pub fn has_openrouter_background_key() -> Result<bool> {
    Ok(secrets::get_openrouter_background_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_background_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    secrets::set_openrouter_background_key(key)
}

/// Run the OS verification (Windows Hello / Touch ID) to lift the launch lock. Returns
/// `true` on success, `false` when the user cancels/fails. The HWND is read on the UI
/// thread (it's `!Send`) and the blocking WinRT wait runs on a worker thread so the UI
/// stays responsive while the system prompt is up.
#[tauri::command]
pub async fn unlock_app(state: State<'_, AppState>, window: tauri::WebviewWindow) -> Result<bool> {
    let raw_handle = {
        #[cfg(target_os = "windows")]
        {
            window
                .hwnd()
                .map_err(|e| Error::Other(format!("no window handle for verification: {e}")))?
                .0 as isize
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Unused off Windows (the stubs ignore it), but keep the binding so the
            // worker closure is identical across platforms.
            let _ = &window;
            0isize
        }
    };
    // Deliberately NOT `blocking::spawn_blocking_result`: this one says "verification task FAILED",
    // not "panicked", and that string reaches the user on the app-unlock path. Converting would be a
    // wording change dressed as a cleanup.
    let verified =
        tauri::async_runtime::spawn_blocking(move || applock::verify(raw_handle, "Unlock PM"))
            .await
            .map_err(|e| Error::Other(format!("verification task failed: {e}")))??;
    if verified {
        state
            .app_unlocked
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(verified)
}

/// Current Windows Smart App Control state, so the updater UI can warn before offering a
/// restart that SAC would silently block (an unsigned installer under SAC-enforced closes
/// PM and reopens on the old version with no error — see `crate::smart_app_control`).
/// Off-Windows, or when SAC is absent, this reports `Unknown` and the UI proceeds normally.
#[tauri::command]
pub fn smart_app_control_state() -> crate::smart_app_control::SmartAppControlState {
    crate::smart_app_control::state()
}

/// Whether the running app is a Linux **package** install (rpm/deb) rather than an AppImage.
/// Tauri's in-app updater can only replace an AppImage in place, so on a package install the
/// updater UI skips the (doomed) background auto-download and points the user at reinstalling
/// the new package instead. False on Windows, macOS, and the Linux AppImage.
#[tauri::command]
pub fn package_managed_linux() -> bool {
    crate::update_delivery::package_managed_linux()
}

/// The OpenRouter model catalogue (public endpoint, no key needed) so the user can
/// browse, search, and pick a model with pricing in Settings (spec §6 — any model,
/// swappable).
#[tauri::command]
pub async fn list_models() -> Result<Vec<openrouter::ModelInfo>> {
    openrouter::list_models().await
}
