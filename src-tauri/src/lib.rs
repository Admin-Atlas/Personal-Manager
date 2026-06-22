// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

mod briefing;
mod calendar;
mod clock;
mod commands;
mod db;
mod error;
mod google;
mod ics;
mod ingest;
mod learning;
mod openrouter;
mod paths;
mod projects;
mod retrieval;
mod review;
mod secrets;
mod sidecar;

use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;
use tauri::Manager;

use sidecar::{SidecarManager, SidecarPaths};

/// Shared app state. The SQLite connection is guarded by a mutex; commands lock
/// it only for short synchronous work, never across an `.await`. The sidecar
/// manages its own interior locking.
pub struct AppState {
    pub db: Mutex<Connection>,
    pub sidecar: SidecarManager,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first. One instance only: a second launch (e.g. a
        // double-click or an updater relaunch overlap) focuses the running window
        // and exits, so two processes can't race to create the encrypted store
        // with different keys and orphan one of them (rule #2).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let handle = app.handle();
            let db_path = paths::db_path(handle)?;
            let key = secrets::get_or_create_db_key()?;
            let conn = db::open(&db_path, &key)?;

            // The sidecar source folder is optional at boot — chat works without
            // it; ingestion surfaces a clear error if it (or Python) is missing.
            let source_dir = paths::sidecar_source_dir(handle).unwrap_or_else(|_| PathBuf::from("sidecar"));
            let venv_dir = paths::venv_dir(handle)?;
            let sidecar = SidecarManager::new(SidecarPaths { source_dir, venv_dir });

            app.manage(AppState {
                db: Mutex::new(conn),
                sidecar,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::has_openrouter_key,
            commands::set_openrouter_key,
            commands::has_openrouter_background_key,
            commands::set_openrouter_background_key,
            commands::get_settings,
            commands::set_chat_models,
            commands::set_background_models,
            commands::set_chat_auto_switch,
            commands::set_background_auto_switch,
            commands::set_help_mode,
            commands::get_time_zone,
            commands::set_time_zone,
            commands::list_models,
            commands::get_learning_profile,
            commands::refresh_learning_profile,
            commands::list_conversations,
            commands::create_conversation,
            commands::get_messages,
            commands::send_message,
            commands::sidecar_status,
            commands::ensure_sidecar,
            commands::ingest_paths,
            commands::rebuild_index,
            commands::list_documents,
            commands::search_documents,
            commands::transcribe_audio,
            commands::list_projects,
            commands::review_queue,
            commands::propose_metadata,
            commands::commit_review,
            commands::set_document_metadata,
            commands::list_project_overviews,
            commands::set_project_metadata,
            commands::propose_project_metadata,
            commands::calendar_status,
            commands::list_ics_feeds,
            commands::add_ics_feed,
            commands::remove_ics_feed,
            commands::set_google_client,
            commands::clear_google_client,
            commands::connect_google,
            commands::disconnect_google,
            commands::list_google_calendars,
            commands::set_google_calendar_ids,
            commands::sync_calendar,
            commands::list_calendar_events,
            commands::get_daily_briefing,
            commands::refresh_daily_briefing,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
