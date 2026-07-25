// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tray / menu-bar icon and the always-on-top briefing window it summons.
//!
//! Both are OFF by default: the tray icon is created hidden and the window is created hidden, so an
//! update never changes what an existing user sees until they switch one on in Settings.
//!
//! # The briefing window
//!
//! A second `WebviewWindow` labelled `briefing`, pointed at `index.html?window=briefing` so
//! `main.tsx` can mount a tiny popover root instead of a second full `<App/>` (which would duplicate
//! the boot IPC, the calendar poll and every resume effect). It is created ONCE at startup and then
//! only shown/hidden — creating it lazily would mean paying webview startup on every toggle.
//!
//! It deliberately holds NO capability entry. PM's own `#[tauri::command]`s are not ACL-gated (the
//! app ships no `permissions/` directory, so there is no `__app-acl__` manifest and the reject arm
//! in tauri's `on_message` is never taken), which means the popover can call `get_daily_briefing`
//! freely — while `plugin:`-prefixed calls stay denied. **That is the invariant to preserve: the
//! popover root calls PM app commands ONLY.** Adding `getCurrentWindow().hide()` or `listen()` there
//! would fail at runtime with nothing in `just check` catching it; the Rust side owns show/hide, and
//! widening `capabilities/default.json` to cover this window would hand it dialog/process/updater
//! permissions it has no business holding.
//!
//! # Linux reality
//!
//! `TrayIconEvent` is never emitted on Linux — upstream documents it as unsupported, so a left click
//! does nothing there no matter which desktop is running. The context menu is therefore the primary
//! affordance and is wired first, with the left-click handler as a Windows/macOS convenience. Tauri
//! also warns the Linux icon may not appear at all unless a menu is set, which makes the menu load-
//! bearing rather than optional. Whether the icon shows at all depends on the desktop having a
//! StatusNotifierItem host: KDE Plasma, XFCE, Cinnamon and MATE do; stock GNOME Shell does not, and
//! needs the third-party AppIndicator extension. A missing icon there is the desktop's arrangement,
//! not a PM bug.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

use crate::db;
use crate::error::{Error, Result};

/// `tauri::Error` isn't one of the kinds `error::Error` converts from, and widening that enum
/// for one module would be the tail wagging the dog — fold it to the string variant here.
fn tauri_err(e: tauri::Error) -> Error {
    Error::Other(e.to_string())
}
use crate::AppState;

/// Window label for the briefing popover. Also matched in `on_window_event`.
pub const BRIEFING_LABEL: &str = "briefing";
/// The tray icon declared in `tauri.conf.json` (`app.trayIcon.id`).
const TRAY_ID: &str = "main";

/// `settings` key: whether the tray icon is shown. Backend-owned rather than a localStorage pref,
/// because Rust must know it at boot (to decide the icon's visibility and whether closing the main
/// window quits) — the frontend's own display prefs have no such consumer.
pub const TRAY_ENABLED_KEY: &str = "tray_icon_enabled";

/// Whether the user has switched the tray icon on. Defaults to false.
pub fn tray_enabled(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    let Ok(conn) = state.conn() else {
        return false;
    };
    db::get_bool(&conn, TRAY_ENABLED_KEY, false).unwrap_or(false)
}

/// Create the briefing window, hidden. Called once during setup.
///
/// `always_on_top` + `skip_taskbar` make it read as a floating utility panel rather than a second
/// application window; `decorations(false)` matches the main window's custom chrome, and the popover
/// root draws its own drag strip.
pub fn build_briefing_window(app: &AppHandle) -> Result<()> {
    if app.get_webview_window(BRIEFING_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        BRIEFING_LABEL,
        WebviewUrl::App("index.html?window=briefing".into()),
    )
    .title("Today's briefing")
    .inner_size(360.0, 440.0)
    .min_inner_size(280.0, 200.0)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()
    .map_err(tauri_err)?;
    Ok(())
}

/// Show or hide the briefing window. The single entry point, reached from the tray menu, the tray
/// left click and the frontend command, so the three can never drift apart.
pub fn toggle_briefing_window(app: &AppHandle, force_show: bool) -> Result<()> {
    build_briefing_window(app)?;
    let Some(win) = app.get_webview_window(BRIEFING_LABEL) else {
        return Ok(());
    };
    let visible = win.is_visible().unwrap_or(false);
    if visible && !force_show {
        let _ = win.hide();
    } else {
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(())
}

/// Turn the tray icon on or off at runtime, persisting the choice.
pub fn set_tray_enabled(app: &AppHandle, enabled: bool) -> Result<()> {
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::set_bool(&conn, TRAY_ENABLED_KEY, enabled)?;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_visible(enabled);
    }
    // Switching the tray off must not leave an orphaned floating window with no way back to it.
    if !enabled {
        if let Some(win) = app.get_webview_window(BRIEFING_LABEL) {
            let _ = win.hide();
        }
    }
    Ok(())
}

/// Wire the tray menu + click handler and apply the stored visibility.
///
/// Best-effort by design: on a Linux box with no appindicator library, `libappindicator-sys` panics
/// on load, and this must not take the whole app down with it — PM's main window works perfectly
/// well without a tray. Hence `catch_unwind` and a logged failure rather than `?`.
pub fn init(app: &AppHandle) {
    let handle = app.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || wire_tray(&handle)));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("tray: setup failed ({e}); continuing without a tray icon"),
        Err(_) => eprintln!("tray: unavailable on this desktop; continuing without a tray icon"),
    }
}

fn wire_tray(app: &AppHandle) -> Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        // No tray in the config, or the platform refused to create one.
        return Ok(());
    };

    // The menu is the ONLY affordance that works on every platform, so it carries the real entry
    // points rather than being a right-click extra.
    let show = MenuItem::with_id(app, "briefing", "Today's briefing", true, None::<&str>)
        .map_err(tauri_err)?;
    let open = MenuItem::with_id(app, "open", "Open PM", true, None::<&str>).map_err(tauri_err)?;
    let quit = MenuItem::with_id(app, "quit", "Quit PM", true, None::<&str>).map_err(tauri_err)?;
    let menu = Menu::with_items(app, &[&show, &open, &quit]).map_err(tauri_err)?;
    tray.set_menu(Some(menu)).map_err(tauri_err)?;

    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "briefing" => {
            let _ = toggle_briefing_window(app, false);
        }
        "open" => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }
        "quit" => app.exit(0),
        _ => {}
    });

    // Left click opens the briefing directly. Windows and macOS only — Linux never emits this.
    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = toggle_briefing_window(tray.app_handle(), false);
        }
    });

    tray.set_visible(tray_enabled(app)).map_err(tauri_err)?;
    Ok(())
}

/// Window-close policy, applied to every window.
///
/// With the tray ON, closing the main window HIDES it and PM keeps running in the tray (Quit lives
/// in the tray menu) — the standard tray-app contract, and the only way the icon can outlive the
/// window. With the tray OFF, close quits exactly as it always has, so a user who never opts in sees
/// no behaviour change at all.
///
/// The briefing window's own close button always just hides it; it is a panel, not a document.
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let app = window.app_handle();
    if window.label() == BRIEFING_LABEL {
        api.prevent_close();
        let _ = window.hide();
        return;
    }
    if window.label() == "main" && tray_enabled(app) {
        api.prevent_close();
        let _ = window.hide();
    }
}
