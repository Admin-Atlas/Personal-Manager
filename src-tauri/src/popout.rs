// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Calendar in a window of its own (#1039). Bobby: "i want to have a window which can just display
//! the calendar tab, and the full function / features etc. just pop out so i can it side by side with
//! whatever im doing on the main scrren."
//!
//! A `WebviewWindow` labelled `calendar`, pointed at `index.html?window=calendar`, where `main.tsx`
//! mounts the Calendar on its own (`CalendarWindow.tsx`) rather than a second `<App/>`, which would
//! repeat the whole boot: the vault gates, the connector polls, the briefing, the resume effects.
//!
//! # Built when asked for, from an async command
//!
//! The briefing window (tray.rs) is built hidden in `setup()` and lives as long as PM. This one is a
//! whole Calendar, a second webview most people never open, so it's built on demand and destroyed
//! when closed. `WebviewWindowBuilder::build()` deadlocks Windows when called from a synchronous
//! command or an event handler (tauri 2.12's own doc comment on `build`; tray.rs tells how that bit
//! PM once). Tauri documents an `async` command as the way to build a window on demand, since it runs
//! on the async runtime rather than the main thread — so [`crate::commands::open_calendar_window`] is
//! async. **Never make it sync, and never call [`open`] from an event handler or `on_window_event`.**
//!
//! # Closing, and quitting
//!
//! Its ✕ is an ordinary close: the window is destroyed, and nothing in `tray::on_window_event` stops
//! it. Closing PM's main window quits PM (tray off), which takes this window with it; with the tray on
//! the main window only hides, and this one carries on, like the briefing panel.
//!
//! # What it may do
//!
//! Its capability (`capabilities/calendar.json`) is the main window's minus the dialog, process and
//! updater grants: the core defaults (it listens for `calendar://changed`, `tasks://changed` and the
//! vault's curtain) and the window controls its title bar drives. Anything it needs from the rest of
//! PM goes through PM's own commands, which aren't ACL-gated: [`show_in_main`] for a link to a project
//! or the pinboard.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::error::{Error, Result};
use crate::{mods, settings, tray, AppState};

/// The pop-out Calendar's window label. Also named in `capabilities/calendar.json`.
pub const CALENDAR_LABEL: &str = "calendar";

/// Sent to the main window to show one of its views: the pop-out's link to a project or to the
/// pinboard. `App.tsx` follows it.
pub const NAVIGATE_EVENT: &str = "main://navigate";

/// Where the main window should go, from a link in another window.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Navigate {
    /// An app view's name, such as `"pinboard"` or `"project"`.
    pub view: String,
    /// With `view: "project"`, which one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// Whether PM may show its calendar in a window of its own right now: never while the app lock is
/// up or the store is closed (a passphrase vault still locked, or another profile holding a shared
/// vault). The button is only reachable past those gates in the main window, but the command is
/// callable from any webview, so it checks for itself.
fn may_open(app: &AppHandle) -> Result<()> {
    // Errors while the store is closed, which is exactly a refusal.
    if settings::app_lock_status_of(&app.state::<AppState>())?.locked {
        return Err(Error::Other("PM is locked: unlock it first".into()));
    }
    Ok(())
}

/// Open the pop-out Calendar, or bring it forward when it's already open. Called ONLY from the async
/// `open_calendar_window` command; see the module docs for why.
pub fn open(app: &AppHandle) -> Result<()> {
    may_open(app)?;
    if let Some(win) = app.get_webview_window(CALENDAR_LABEL) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let builder = WebviewWindowBuilder::new(
        app,
        CALENDAR_LABEL,
        WebviewUrl::App("index.html?window=calendar".into()),
    )
    .title("Calendar — PM (Alpha)")
    .inner_size(960.0, 720.0)
    // Small enough to sit beside another window on a laptop screen; the Calendar's narrow layouts
    // take over below its breakpoints.
    .min_inner_size(480.0, 400.0)
    // PM's own title bar, as in the main window.
    .decorations(false);
    // Sealed like the main window, though it hosts no mods: on Windows it must carry the same
    // browser args or WebView2 refuses to create it.
    mods::build_window(builder, app, mods::Role::NoMods)?;
    Ok(())
}

/// Show `to` in the main window and bring it forward: the pop-out's links to the rest of PM.
pub fn show_in_main(app: &AppHandle, to: Navigate) -> Result<()> {
    app.emit_to("main", NAVIGATE_EVENT, &to)
        .map_err(|e| Error::Other(e.to_string()))?;
    tray::show_main_window(app);
    Ok(())
}

/// DESTROY the pop-out, for "Remove PM data": it's a second JS context writing calendar preferences
/// into the same origin store the main window is about to clear (the briefing window's reason too).
pub fn destroy(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(CALENDAR_LABEL) {
        let _ = win.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_to_a_project_names_it_and_one_to_a_view_does_not() {
        let to = Navigate {
            view: "project".into(),
            project: Some("Atlas".into()),
        };
        assert_eq!(
            serde_json::to_value(&to).unwrap(),
            serde_json::json!({ "view": "project", "project": "Atlas" })
        );
        let to = Navigate {
            view: "pinboard".into(),
            project: None,
        };
        assert_eq!(
            serde_json::to_value(&to).unwrap(),
            serde_json::json!({ "view": "pinboard" })
        );
    }
}
