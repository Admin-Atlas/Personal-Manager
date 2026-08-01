// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The data folder: reveal it, and export everything out of it.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::blocking::spawn_blocking_result;
use crate::error::{Error, Result};
use crate::ingest;
use crate::{pathguard, paths, vault, AppState};

// --- data folder: reveal + export ---

/// Reveal the data folder (the encrypted store + the Markdown vault) in the OS file
/// manager — Explorer on Windows, Finder on macOS — so the user can find, back up,
/// or copy it. Uses the same `open` crate that launches the OAuth browser.
#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> Result<()> {
    let dir = paths::data_dir(&app)?;
    open::that(dir).map_err(Error::from)
}

/// Bundle the user's data into a single `.zip` at `dest_path`: the encrypted store
/// plus the Markdown vault. The regenerable `runtime/` (the Python venv + model
/// cache) is deliberately excluded.
///
/// The live `pm.sqlite` is never copied directly — WAL means freshly committed pages
/// can still live in the `-wal` sidecar — so we `VACUUM INTO` a consistent snapshot
/// first (which preserves SQLCipher encryption and folds in all WAL pages) under the
/// DB lock, then archive that snapshot as `pm.sqlite`. The lock is released before the
/// slower zip walk. The exported store stays encrypted with the same key, so it opens
/// only on a machine whose keychain holds this app's DB key.
#[tauri::command]
pub async fn export_all_data(
    app: AppHandle,
    _state: State<'_, AppState>,
    dest_path: String,
) -> Result<()> {
    // L-5: `dest_path` is a webview-supplied write destination — validate its shape and that its
    // containing folder exists before we write the export archive there.
    pathguard::sanitize_destination(&dest_path)?;
    // A temp *directory* (not file) so `VACUUM INTO` writes a fresh file into an empty
    // dir — it refuses a pre-existing target. The dir (and snapshot) is removed on drop.
    let tmp = tempfile::Builder::new().prefix("pm-export-").tempdir()?;
    let snapshot = tmp.path().join(vault::DB_FILENAME);
    let data_dir = paths::data_dir(&app)?;
    let dest = dest_path;
    // Snapshot + zip on the blocking pool (F-42): a `VACUUM INTO` can copy a multi-GB store, so on
    // the async runtime it pinned a tokio worker *and* held the global DB mutex for the whole copy.
    // The guard is scoped to the vacuum inside the closure, so it still releases before the slower
    // zip walk — same lock lifetime as before, just off the runtime. The snapshot reaches the store
    // via the cloned `app` handle (DbGuard is !Send, so acquire it inside the closure). `tmp` stays
    // owned here and outlives the task.
    spawn_blocking_result("export", move || -> Result<()> {
        {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            vault::migrate::vacuum_into(&conn, &snapshot)?;
        }
        write_export_zip(&data_dir, &snapshot, std::path::Path::new(&dest))
    })
    .await
}

/// The result of a plaintext export: how many files were written and where.
#[derive(Serialize)]
pub struct PlaintextExportOutcome {
    pub count: usize,
    pub dest: String,
}

/// Export the Markdown vault as plaintext `.md` files — the spec's "you are never locked in" escape
/// hatch (§3). Reads every vault file, decrypting any encrypted ones with the in-session key, and
/// writes a clean tree with no `.pmenc` files, so the user can walk away with their notes in the
/// open at any time. The vault must be unlocked (the Markdown key has to be loaded). Unlike
/// `export_all_data`, this is a *plaintext* escape hatch, not an encrypted backup — it deliberately
/// strips the at-rest protection.
///
/// L-5: because this writes DECRYPTED vault content, the destination must not be a path a compromised
/// webview could fabricate. We therefore pick the folder in the BACKEND (off the main thread) rather
/// than trusting a webview-supplied string. Returns `None` if the user cancels; otherwise the count
/// and the chosen destination for the confirmation message.
#[tauri::command]
pub async fn export_plaintext_markdown(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<PlaintextExportOutcome>> {
    use tauri_plugin_dialog::DialogExt;
    let app2 = app.clone();
    // Deliberately NOT `blocking::spawn_blocking_result`: the picker returns an `Option`, not a
    // `Result`, so it does not fit the helper's bound — and its message says "failed", not
    // "panicked", which converting would silently retype.
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .file()
            .set_title("Choose a folder for the plaintext export")
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| Error::Other(format!("folder dialog task failed: {e}")))?;
    let Some(picked) = picked else {
        return Ok(None); // cancelled
    };
    let dest = picked
        .into_path()
        .map_err(|e| Error::Other(format!("couldn't read the chosen folder path: {e}")))?;
    let (vault, cipher) = state.markdown_io()?;
    let dest_for_task = dest.clone();
    let count = spawn_blocking_result("export", move || {
        ingest::export_plaintext(&vault, &cipher, &dest_for_task)
    })
    .await?;
    Ok(Some(PlaintextExportOutcome {
        count,
        dest: dest.to_string_lossy().into_owned(),
    }))
}

/// Write the export archive: the DB snapshot as `pm.sqlite`, then the vault tree.
fn write_export_zip(
    data_dir: &std::path::Path,
    db_snapshot: &std::path::Path,
    dest: &std::path::Path,
) -> Result<()> {
    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // The zip entry name is the layout constant, like the `.pmbackup` entry names: this archive is a
    // same-machine copy of the vault folder, so the member has to be called what the file is called.
    zip.start_file(vault::DB_FILENAME, opts)?;
    let mut snap = std::fs::File::open(db_snapshot)?;
    std::io::copy(&mut snap, &mut zip)?;

    let vault = data_dir.join("vault");
    if vault.is_dir() {
        add_dir_to_zip(&mut zip, &vault, "vault", opts)?;
    }
    zip.finish()?;
    Ok(())
}

/// Recursively add `dir` to the archive under `prefix`, preserving relative paths.
fn add_dir_to_zip<W: std::io::Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &std::path::Path,
    prefix: &str,
    opts: zip::write::SimpleFileOptions,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let zip_path = format!("{prefix}/{}", name.to_string_lossy());
        if path.is_dir() {
            add_dir_to_zip(zip, &path, &zip_path, opts)?;
        } else {
            zip.start_file(zip_path, opts)?;
            let mut f = std::fs::File::open(&path)?;
            std::io::copy(&mut f, zip)?;
        }
    }
    Ok(())
}
