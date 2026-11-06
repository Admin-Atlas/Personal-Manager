// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The voice switch (Mods spec §2 "Voice", decision D8): one stored setting that turns voice input
//! on or off. It is enforced here in Rust, not only by hiding the Composer's mic: while it is off,
//! `transcribe_audio` refuses, however the call arrives.
//!
//! It also owns the speech engine's place on disk. faster-whisper, with CTranslate2 and PyAV, used
//! to sit in the base venv for every user, so hiding the mic alone would have made PM no leaner. It
//! is an optional component now, like photo OCR (`sidecar::OPTIONAL_VOICE_COMPONENT`): switching
//! voice on installs it, with the usual progress on `voice://install`; switching it off offers to
//! remove it and the Whisper model's weights. The switch and the engine live on one row in
//! Settings → Storage.
//!
//! Defaults (D8): a brand-new store starts off. An existing store starts on only if the Whisper
//! model's own folder is on this computer (`sidecar::WHISPER_REPO_DIR`), which appears only after a
//! first transcription; `runtime/models` being non-empty proves nothing, because the embedder lives
//! there too. Like the Local AI switch, the default is decided once, at the first open of each
//! store ([`stamp_default`]), and written down: the folder comes and goes later (the Storage tab
//! removes it, a recording downloads it), and neither may flip the switch.
//!
//! Existing venvs need one more step. pip never removes a package because a lock stopped listing
//! it, so every venv an older PM built still holds the engine. The reconciler below hands the
//! sidecar each store's answer, and the sidecar removes that engine where voice is off (unless
//! someone switched voice off and chose to keep it) and adopts it as the component where voice is
//! on (`SidecarManager::reconcile_voice_engine`).

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::{db, AppState, BusyGuard};

/// The stored switch, `"true"` or `"false"`. Absent until the store's first open decides it.
pub(crate) const VOICE_ENABLED_KEY: &str = "voice_enabled";

/// What `transcribe_audio` says while voice is off. Reached only by a stale view or a script: the
/// mic is gone with the switch. The sidecar gives it too, to a recording that was let through just
/// before the switch went off and then found the engine gone.
pub(crate) const OFF_REFUSAL: &str = "Voice input is off. Turn it on in Settings → Storage first.";

/// A stored row's meaning: `None` for an absent row or one PM never writes.
fn parse(raw: Option<&str>) -> Option<bool> {
    match raw {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// D8's default for a store that has none yet: on only where the Whisper model was downloaded,
/// which is what using voice before it was a switch left behind. A new user has no such folder, so
/// starts off.
fn default_on(whisper_model_present: bool) -> bool {
    whisper_model_present
}

/// The switch as the open store has it. A store that has not been opened since this shipped has no
/// row yet, and reads as D8's default without writing it, so this is safe on any guard.
pub(crate) fn read(conn: &Connection, whisper_model_present: bool) -> Result<bool> {
    Ok(parse(db::get_setting(conn, VOICE_ENABLED_KEY)?.as_deref())
        .unwrap_or_else(|| default_on(whisper_model_present)))
}

/// Decide D8's default for a store that has no row yet, and write it down. Returns the switch. Run
/// on every store open (`AppState::open_session`); a store that already has a row is only read.
pub(crate) fn stamp_default(conn: &Connection, whisper_model_present: bool) -> Result<bool> {
    if let Some(on) = parse(db::get_setting(conn, VOICE_ENABLED_KEY)?.as_deref()) {
        return Ok(on);
    }
    let on = default_on(whisper_model_present);
    db::set_bool(conn, VOICE_ENABLED_KEY, on)?;
    Ok(on)
}

/// The gate itself, on an open connection: `Ok` while voice is on, else the refusal.
fn check_on(conn: &Connection, whisper_model_present: bool) -> Result<()> {
    if read(conn, whisper_model_present)? {
        Ok(())
    } else {
        Err(Error::Other(OFF_REFUSAL.into()))
    }
}

/// Whether voice is on in the open store. The folder check runs before the DB guard is taken, and
/// the guard closes before this returns.
pub(crate) fn is_on(state: &AppState) -> Result<bool> {
    let model_present = state.sidecar.whisper_model_present();
    let conn = state.conn()?;
    read(&conn, model_present)
}

/// Refuse a transcription while voice is off. Reads the store every time: a transcription is a
/// deliberate action, and the row is the switch.
pub(crate) fn require_on(state: &AppState) -> Result<()> {
    let model_present = state.sidecar.whisper_model_present();
    let conn = state.conn()?;
    check_on(&conn, model_present)
}

/// What the Composer needs to show or hide the mic.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct VoiceSwitch {
    pub on: bool,
}

/// The switch, for the Composer's mic (the Storage row reads it with the engine's state).
#[tauri::command]
pub fn get_voice_switch(state: State<'_, AppState>) -> Result<VoiceSwitch> {
    Ok(VoiceSwitch { on: is_on(&state)? })
}

/// Switch voice input on or off, and answer with where it now stands.
///
/// On installs the speech engine FIRST (a download, with progress on `voice://install`) and writes
/// the switch only once that has worked, so a failed or cut-off download leaves voice off rather
/// than showing a mic that can't transcribe. Installing is a marker read when the engine is
/// already there, so this is also how Storage repairs an engine an update left stale.
///
/// Off writes the switch first, so a transcription that arrives during the removal is refused.
/// With `remove_engine` it then uninstalls the engine (the worker is stopped first, so no native
/// library is held open) and deletes the Whisper model's weights. If that removal fails, voice is
/// still off and the error says so; the Storage row then shows what is left, with its own Remove.
/// Without it, an engine an older PM installed is first recorded as kept, before anything else
/// changes: the upgrade's clean-up would otherwise read this off as its own default and remove the
/// engine anyway, here or at the next launch.
///
/// One switch at a time (`voice_busy`): an off arriving while an on is still downloading would
/// otherwise write off, wait for the install lock, remove the engine, and then watch the on finish
/// by writing on.
#[tauri::command]
pub async fn set_voice_enabled(
    app: AppHandle,
    on: bool,
    remove_engine: bool,
) -> Result<VoiceSwitch> {
    let state = app.state::<AppState>();
    let Some(_busy) = BusyGuard::acquire(&state.voice_busy) else {
        return Err(Error::Other(
            "Voice input is already being switched. Wait for that to finish.".into(),
        ));
    };

    if on {
        let worker = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let progress_app = worker.clone();
            worker
                .state::<AppState>()
                .sidecar
                .install_optional_voice(move |fraction| {
                    crate::commands::emit_install_progress(&progress_app, "voice", fraction);
                })
        })
        .await
        .map_err(|e| Error::Other(format!("voice engine install task panicked: {e}")))??;
        {
            let conn = state.conn()?;
            db::set_bool(&conn, VOICE_ENABLED_KEY, true)?;
        }
        state.sidecar.set_voice_wanted(true);
        return Ok(VoiceSwitch { on: true });
    }

    if !remove_engine {
        state.sidecar.keep_voice_engine()?;
    }
    {
        let conn = state.conn()?;
        db::set_bool(&conn, VOICE_ENABLED_KEY, false)?;
    }
    state.sidecar.set_voice_wanted(false);
    if remove_engine {
        let worker = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let sidecar = &worker.state::<AppState>().sidecar;
            sidecar.uninstall_optional_voice()?;
            sidecar.remove_whisper_model()
        })
        .await
        .map_err(|e| Error::Other(format!("voice engine removal task panicked: {e}")))?
        .map_err(|e| {
            Error::Other(format!(
                "Voice input is off, but PM couldn't remove its engine ({e}). You can remove it \
                 in Settings → Storage."
            ))
        })?;
    }
    Ok(VoiceSwitch { on: false })
}

/// Run the sidecar's one-off clean-up of an older PM's speech engine whenever a store says what
/// voice is (each open, each switch). The work is pip, so it runs on the blocking pool; answers that
/// arrive while it runs are coalesced by the watch channel into one more pass, and each pass reads
/// the latest. A no-op on a venv that has nothing to settle.
pub(crate) fn spawn_engine_reconciler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut wanted = app.state::<AppState>().sidecar.subscribe_voice_wanted();
        loop {
            // The current answer first: a store opened at launch, before this subscribed, has
            // already said, and `changed` reports only what comes after.
            if wanted.borrow_and_update().is_some() {
                let worker = app.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    worker.state::<AppState>().sidecar.reconcile_voice_engine();
                })
                .await;
            }
            if wanted.changed().await.is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn
    }

    /// D8: on only where the Whisper model's own folder was found, which a new user never has.
    #[test]
    fn the_default_follows_the_whisper_model_folder() {
        assert!(default_on(true));
        assert!(!default_on(false));
    }

    /// Decided once from what this computer holds, written down, and never re-derived: removing the
    /// model in Storage later must not switch voice off, nor a download switch it on.
    #[test]
    fn the_default_is_decided_once_and_then_frozen() {
        let new_user = mem_conn();
        assert!(
            !read(&new_user, false).unwrap(),
            "a brand-new store reads as off"
        );
        assert!(!stamp_default(&new_user, false).unwrap());
        assert_eq!(
            db::get_setting(&new_user, VOICE_ENABLED_KEY)
                .unwrap()
                .as_deref(),
            Some("false"),
            "and the answer is written down"
        );
        assert!(!read(&new_user, true).unwrap());
        assert!(!stamp_default(&new_user, true).unwrap());

        let dictated_before = mem_conn();
        assert!(
            read(&dictated_before, true).unwrap(),
            "the model folder reads as on"
        );
        assert!(stamp_default(&dictated_before, true).unwrap());
        assert!(read(&dictated_before, false).unwrap());
        assert!(stamp_default(&dictated_before, false).unwrap());
    }

    #[test]
    fn a_row_pm_never_writes_reads_as_the_default() {
        let conn = mem_conn();
        db::set_setting(&conn, VOICE_ENABLED_KEY, "yes").unwrap();
        assert!(!read(&conn, false).unwrap());
        assert!(read(&conn, true).unwrap());
    }

    /// The refusal `transcribe_audio` gives: off (stored, or by default) refuses with words that
    /// say where to turn it on; on lets the call through.
    #[test]
    fn transcription_is_refused_while_voice_is_off() {
        let conn = mem_conn();
        let refused = check_on(&conn, false).unwrap_err().to_string();
        assert!(
            refused.contains("Voice input is off"),
            "unexpected refusal: {refused}"
        );
        assert!(check_on(&conn, true).is_ok(), "the default can be on");

        db::set_bool(&conn, VOICE_ENABLED_KEY, false).unwrap();
        assert!(check_on(&conn, true).is_err(), "a stored off wins");
        db::set_bool(&conn, VOICE_ENABLED_KEY, true).unwrap();
        assert!(check_on(&conn, false).is_ok(), "a stored on wins");
    }
}
