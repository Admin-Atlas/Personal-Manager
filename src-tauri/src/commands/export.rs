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

/// Where PM's data actually is, for a Settings card that has to SHOW the path it opens (#712).
///
/// Two paths rather than one, because there genuinely are two once a vault has been moved or joined:
/// the vault folder holds the store and the Markdown, while this profile's app-data folder keeps the
/// pointer that redirects to it plus the regenerable runtime. Before this, no command exposed either
/// to the frontend at all — the Vault card showed `status.location` while the button forty lines away
/// opened the profile default, so on a moved vault it opened somewhere the documents are not.
#[derive(Serialize)]
pub struct DataLocations {
    /// The folder holding `pm.sqlite`, `vault-meta.json` and the Markdown — what "Open data folder"
    /// opens, because it is where the user's data is.
    pub vault_root: String,
    /// This profile's own folder. Equal to `vault_root` for a vault that has never been moved.
    pub app_data_dir: String,
    /// True when the two differ — a pointed (moved or shared) vault. The card names the second
    /// folder only then, so an ordinary install still reads as one place.
    pub pointed: bool,
}

/// Resolve both folders. Cheap, and safe to call on every render of the Data section.
#[tauri::command]
pub fn data_locations(app: AppHandle) -> Result<DataLocations> {
    let app_data_dir = paths::data_dir(&app)?;
    let resolved = vault::resolve(&app)?;
    Ok(DataLocations {
        pointed: resolved.vault_root != app_data_dir,
        vault_root: resolved.vault_root.to_string_lossy().into_owned(),
        app_data_dir: app_data_dir.to_string_lossy().into_owned(),
    })
}

/// Reveal the data folder (the encrypted store + the Markdown vault) in the OS file
/// manager — Explorer on Windows, Finder on macOS — so the user can find, back up,
/// or copy it. Uses the same `open` crate that launches the OAuth browser.
///
/// Opens the RESOLVED vault root, not this profile's data dir (#712). It used to open the latter,
/// which is the same folder for an ordinary install and the wrong one for every moved or joined
/// vault — the case where a user most needs to be shown where their documents went.
#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> Result<()> {
    let dir = vault::resolve(&app)?.vault_root;
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
    // The RESOLVED vault root, not the profile data dir (#712). The snapshot below already comes
    // from the live (possibly pointed) store, so joining a hard-coded "vault" under the data dir
    // paired the right database with the wrong Markdown tree on every moved or shared vault — and
    // on a vault moved out of the profile, with no Markdown at all.
    let resolved = vault::resolve(&app)?;
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
        write_export_zip(&resolved, &snapshot, std::path::Path::new(&dest))
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

/// Write the export archive: the DB snapshot as `pm.sqlite`, the two always-encrypted sidecars and
/// the vault metadata, then the Markdown tree.
///
/// The file set matches [`crate::backup::pack`]'s, and that is the point (#712). This archive used to
/// carry the store and the Markdown alone, so a user who unzipped it onto a new machine had a vault
/// PM could not open — `vault-meta.json` holds the KDF parameters and the verifier — and, if it did
/// open, one that had silently lost its entity rules and every index-only pointer. An export missing
/// what a backup includes is a promise of portability the archive cannot keep.
fn write_export_zip(
    resolved: &vault::ResolvedVault,
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

    // Optional in the same sense the backup treats them: a fresh vault may not have written them
    // yet, and NotFound is the only absence accepted — any other read error propagates rather than
    // quietly producing an archive that is missing something it could not prove was absent.
    for name in [
        vault::META_FILENAME,
        crate::entities::RULES_FILENAME,
        crate::index_only::MANIFEST_FILENAME,
    ] {
        let path = resolved.vault_root.join(name);
        match std::fs::File::open(&path) {
            Ok(mut f) => {
                zip.start_file(name, opts)?;
                std::io::copy(&mut f, &mut zip)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::from(e)),
        }
    }

    if resolved.markdown_dir.is_dir() {
        add_dir_to_zip(
            &mut zip,
            &resolved.markdown_dir,
            vault::MARKDOWN_DIRNAME,
            opts,
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(path: &std::path::Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn entries(zip_path: &std::path::Path) -> Vec<String> {
        let f = std::fs::File::open(zip_path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn an_export_carries_what_a_backup_carries() {
        // The archive used to hold the store and the Markdown alone. Unzipped onto another machine
        // that is a vault PM cannot open — `vault-meta.json` holds the KDF parameters and the
        // verifier — and if it did open, one that had silently lost its entity rules and every
        // index-only pointer. An export missing what a backup includes cannot keep the promise the
        // word "export" makes.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Vault");
        write(&root.join(vault::META_FILENAME), "{}");
        write(&root.join(crate::entities::RULES_FILENAME), "rules");
        write(&root.join(crate::index_only::MANIFEST_FILENAME), "index");
        write(&root.join("vault").join("a.md"), "# a");
        write(&root.join("vault").join("chats").join("b.md"), "# b");
        let snapshot = dir.path().join("snap.sqlite");
        write(&snapshot, "db");

        let resolved = vault::ResolvedVault {
            vault_root: root.clone(),
            db_path: root.join(vault::DB_FILENAME),
            markdown_dir: root.join("vault"),
        };
        let dest = dir.path().join("out.zip");
        write_export_zip(&resolved, &snapshot, &dest).unwrap();

        assert_eq!(
            entries(&dest),
            vec![
                "entities.pmrules".to_string(),
                "index-only.pmindex".to_string(),
                "pm.sqlite".to_string(),
                "vault-meta.json".to_string(),
                "vault/a.md".to_string(),
                "vault/chats/b.md".to_string(),
            ]
        );
    }

    #[test]
    fn a_moved_vault_exports_its_own_markdown_not_the_profiles() {
        // The defect #712 names. The DB snapshot has always come from the live (possibly pointed)
        // store while the tree came from `<data_dir>/vault` — so a moved or shared vault paired the
        // right database with a stale, unrelated Markdown tree, or with none at all.
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Personal Manager");
        let moved = dir.path().join("Shared").join("PM Vault");
        write(&profile.join("vault").join("stale.md"), "old profile copy");
        write(&moved.join("vault").join("real.md"), "the live vault");
        let snapshot = dir.path().join("snap.sqlite");
        write(&snapshot, "db");

        let resolved = vault::ResolvedVault {
            vault_root: moved.clone(),
            db_path: moved.join(vault::DB_FILENAME),
            markdown_dir: moved.join("vault"),
        };
        let dest = dir.path().join("out.zip");
        write_export_zip(&resolved, &snapshot, &dest).unwrap();

        let names = entries(&dest);
        assert!(names.contains(&"vault/real.md".to_string()));
        assert!(!names.contains(&"vault/stale.md".to_string()));
    }

    #[test]
    fn a_fresh_vault_exports_without_the_sidecars_it_has_not_written_yet() {
        // Absent is fine; unreadable is not. The three optional members are skipped only on
        // NotFound, so an archive can never quietly omit something it could not prove was absent.
        let dir = tempfile::tempdir().unwrap();
        let root: PathBuf = dir.path().join("Vault");
        std::fs::create_dir_all(&root).unwrap();
        let snapshot = dir.path().join("snap.sqlite");
        write(&snapshot, "db");

        let resolved = vault::ResolvedVault {
            vault_root: root.clone(),
            db_path: root.join(vault::DB_FILENAME),
            markdown_dir: root.join("vault"),
        };
        let dest = dir.path().join("out.zip");
        write_export_zip(&resolved, &snapshot, &dest).unwrap();
        assert_eq!(entries(&dest), vec!["pm.sqlite".to_string()]);
    }
}
