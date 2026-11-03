// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PM's extra windows: the pop-out Calendar (#1039). The window itself lives in `popout.rs`.

use tauri::AppHandle;

use crate::error::Result;
use crate::popout::{self, Navigate};

/// Open the Calendar in a window of its own, or bring it forward when it's open. Refused while PM is
/// locked or its store is closed.
///
/// **Async on purpose.** Building a window from a synchronous command deadlocks Windows, and an async
/// command is Tauri's documented way to build one on demand (see `popout.rs`). Don't make it sync.
#[tauri::command]
pub async fn open_calendar_window(app: AppHandle) -> Result<()> {
    popout::open(&app)
}

/// Show one of the main window's views and bring it forward: the pop-out Calendar's links to a
/// project or to the pinboard. `view` is an app view's name; `project` goes with `"project"`.
#[tauri::command]
pub fn show_in_main_window(app: AppHandle, view: String, project: Option<String>) -> Result<()> {
    popout::show_in_main(&app, Navigate { view, project })
}

/// Destroy the pop-out Calendar before "Remove PM data" clears the webview store, for the reason
/// `destroy_briefing_window` gives: a second JS context on the same store.
#[tauri::command]
pub fn destroy_calendar_window(app: AppHandle) -> Result<()> {
    popout::destroy(&app);
    Ok(())
}
