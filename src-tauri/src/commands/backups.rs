// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Encrypted portable backups (`.pmbackup`) and every destination they reach: local disk,
//! Proton Drive and Google Drive, plus restore/adopt and the schedule.

use rusqlite::Connection;
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::backup::{
    self, destination::BackupDestination, BackupEvent, BackupKind, BackupPhase, BackupReport,
    RetentionOutcome,
};
use crate::blocking::{spawn_blocking_join, spawn_blocking_result};
use crate::error::{Error, Result};
use crate::google;
use crate::{pathguard, paths, secrets, vault, AppState, BusyGuard, VaultRuntime};

use super::vaults::attach_profile_here;
use super::vaults::engage_or_warn;

// --- encrypted portable backup (local `.pmbackup`; Proton push/pull + scheduling land later) ---

/// Update the shared backup snapshot and broadcast a `backup://progress` event globally
/// (detached from the view that started the op, like the Drive sync). The snapshot lets
/// the Backup settings UI restore an in-flight op after navigating away.
fn emit_backup_progress(app: &AppHandle, ev: BackupEvent) {
    let state = app.state::<AppState>();
    if let Ok(mut snap) = state.backup_state.lock() {
        match &ev {
            BackupEvent::Phase { phase, fraction } => {
                // Edge-triggered on idle→running: EVERY phase transition arrives as a `Phase` event,
                // so stamping unconditionally would reset the elapsed timer at snapshot→pack→upload
                // and read even more wrongly than the mount-time fallback it replaces.
                if !snap.running {
                    snap.started_at_ms = Some(crate::epoch_ms());
                }
                snap.running = true;
                snap.phase = Some(*phase);
                snap.fraction = *fraction;
                snap.last_error = None;
                // …and drop the PREVIOUS run's outcome with it. Without this, a panel mounted
                // mid-run replayed the last run's partial-failure banner underneath a live
                // progress bar. Unconditional is fine here (unlike `started_at_ms` above, which
                // must be edge-triggered): clearing is idempotent, and nothing reads
                // `last_report` while a run is in flight.
                snap.last_report = None;
            }
            BackupEvent::Finished { report } => {
                snap.running = false;
                snap.started_at_ms = None;
                snap.phase = None;
                snap.fraction = 1.0;
                snap.last_report = Some(report.clone());
            }
            BackupEvent::Failed { message } => {
                snap.running = false;
                snap.started_at_ms = None;
                snap.phase = None;
                snap.last_error = Some(message.clone());
            }
        }
    }
    let _ = app.emit("backup://progress", ev);
}

/// A restore's frontend-safe summary — deliberately WITHOUT the embedded DB key (which
/// stays in Rust and is seeded straight into this device's keychain). `Clone` so it can also
/// be parked in [`BackupState::pending_restore`] and re-served to a remounted UI.
#[derive(Clone, Serialize)]
pub struct RestoreSummary {
    pub vault_id: String,
    pub key_mode: String,
    pub markdown_encryption: String,
    pub app_version: String,
    pub created_at: String,
    /// Absolute path of the restored (not-yet-active) vault, for a follow-up "switch".
    pub target_dir: String,
}

/// Create an encrypted, portable `.pmbackup` at `dest_path`, protected by `passphrase`.
/// The live DB is snapshotted with `VACUUM INTO` under the lock (folding WAL, preserving
/// SQLCipher), then — off the lock, in a blocking task — the snapshot + Markdown vault +
/// metadata are streamed through zstd and a chunked XChaCha20-Poly1305 STREAM. The
/// archive embeds the DB key inside its encrypted layer, so it restores on any machine
/// that has the passphrase (unlike `export_all_data`, which is same-machine only).
#[tauri::command]
pub async fn create_local_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    dest_path: String,
    passphrase: String,
) -> Result<()> {
    // I-03: wipe the backup passphrase plaintext from memory on return — it flows into `pack` as a
    // borrow and is dropped (zeroized) when the blocking task that owns it completes. The derived
    // key is already Zeroizing; the raw passphrase was the backup-family gap left after #257.
    let passphrase = zeroize::Zeroizing::new(passphrase);
    if passphrase.is_empty() {
        return Err(Error::Other("a backup passphrase is required".into()));
    }
    // L-5: `dest_path` is a webview-supplied write destination — validate its shape and that its
    // containing folder exists before we write the archive there.
    pathguard::sanitize_destination(&dest_path)?;
    // M-4: strength floor before packing — the archive embeds the raw DB key and is portable.
    vault::kdf::validate_passphrase_strength(&passphrase)?;
    let _busy = begin_backup_run(&app, &state, BackupPhase::Snapshot)?;

    // Consistent, encrypted DB snapshot under the lock; drop the guard before the slow work.
    let tmp = tempfile::Builder::new()
        .prefix("pm-backup-snap-")
        .tempdir()?;
    let snapshot = tmp.path().join(vault::DB_FILENAME);
    {
        // Snapshot on the blocking pool (F-42): a `VACUUM INTO` of a multi-GB store must not pin a
        // tokio worker or hold the DB mutex on the async runtime. The guard is acquired and dropped
        // inside the closure (DbGuard is !Send) via a cloned handle; `snapshot` is cloned in and the
        // original flows into the pack inputs below.
        let app = app.clone();
        let snapshot = snapshot.clone();
        spawn_blocking_result("snapshot", move || -> Result<()> {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            vault::migrate::vacuum_into(&conn, &snapshot)
        })
        .await?;
    }
    emit_backup_progress(
        &app,
        BackupEvent::Phase {
            phase: BackupPhase::Snapshot,
            fraction: 1.0,
        },
    );

    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata to back up".into()))?;
    let db_key = vault::current_db_key(&meta)?
        .ok_or_else(|| Error::Other("unlock the vault before backing it up".into()))?;
    let inputs = backup::pack::PackInputs {
        vault_root: resolved.vault_root.clone(),
        db_snapshot: snapshot,
        markdown_dir: resolved.markdown_dir.clone(),
        meta: meta.clone(),
        db_key_hex: db_key,
        app_version: app.package_info().version.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let vault_id = meta.vault_id.clone();
    let dest = std::path::PathBuf::from(dest_path);

    let app2 = app.clone();
    let result = spawn_blocking_join("backup", move || {
        let st = app2.state::<AppState>();
        let report = |phase, fraction| {
            emit_backup_progress(&app2, BackupEvent::Phase { phase, fraction });
        };
        backup::pack::pack(inputs, &dest, &passphrase, report, &st.backup_cancel)
    })
    .await?;
    // The snapshot tempdir stayed alive through the task above; release it now.
    drop(tmp);

    match result {
        Ok(()) => {
            emit_backup_progress(
                &app,
                BackupEvent::Finished {
                    report: BackupReport {
                        kind: BackupKind::Backup,
                        vault_id: Some(vault_id),
                        target_dir: None,
                        created_at: None,
                        failed_destinations: Vec::new(),
                        retention_notes: Vec::new(),
                    },
                },
            );
            Ok(())
        }
        Err(e) => {
            let msg = if state.backup_cancel.load(Ordering::SeqCst) {
                "Backup cancelled.".to_string()
            } else {
                e.to_string()
            };
            emit_backup_progress(
                &app,
                BackupEvent::Failed {
                    message: msg.clone(),
                },
            );
            Err(Error::Other(msg))
        }
    }
}

/// Restore a `.pmbackup` into a fresh folder under the data dir. Validated end-to-end
/// (the DB opens with the embedded key and passes an integrity check) before anything is
/// promoted, so a wrong passphrase or a corrupt archive never touches the live vault. On
/// success the restored vault's key is stashed IN MEMORY only (see [`finalize_restore`]) and the
/// returned summary lets the UI offer "switch to it now" — [`switch_to_vault`] is the commit point
/// that promotes the key to the keychain, so inspecting a restore never overwrites the live
/// vault's cached key.
#[tauri::command]
pub async fn restore_local_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    src_path: String,
    passphrase: String,
) -> Result<RestoreSummary> {
    // I-03: wipe the backup passphrase plaintext from memory on return (see `create_local_backup`).
    let passphrase = require_backup_passphrase(passphrase)?;
    // Opens on `Restore`, not `Download`: the archive is already on this machine.
    let _busy = begin_backup_run(&app, &state, BackupPhase::Restore)?;

    // L-5: `src_path` is a webview string pointing at an existing `.pmbackup` — require a real,
    // absolute, well-formed location before we open and validate the archive.
    pathguard::sanitize_source(&src_path)?;
    let src = std::path::PathBuf::from(src_path);
    let target = restore_staging_target(&app)?;

    let app2 = app.clone();
    let target2 = target.clone();
    let result = spawn_blocking_join("restore", move || {
        let st = app2.state::<AppState>();
        let report = |phase, fraction| {
            emit_backup_progress(&app2, BackupEvent::Phase { phase, fraction });
        };
        backup::restore::restore(&src, &passphrase, &target2, report, &st.backup_cancel)
    })
    .await?;

    let outcome = unwrap_restore_result(&app, &state, result)?;
    Ok(finalize_restore(&app, &state, outcome))
}

/// Whether an open store holds anything the user put there — the test standing between a restore and
/// [`crate::vault::migrate::delete_vault_artifacts`], so it errs towards "yes" at every step.
///
/// `documents` alone used to answer this, which quietly equated "nothing indexed" with "nothing here".
/// A vault can hold projects, milestones, flags, teachings, chats, connected calendars and connector
/// accounts before a single file is imported, and all of it was invisible to that question.
///
/// Three tables carry migration-seeded rows on EVERY vault, so they are matched by VALUE and never by
/// "any row" — `settings` and `entities`/`entity_aliases` (migrations.rs `INSERT OR IGNORE` of the
/// embedder defaults and of the `Unsorted` inbox), plus the keys ordinary boot writes with no user
/// action at all. Get that wrong in the other direction and re-homing silently never happens again.
/// Document-derived tables (chunks, tags, layout, proposals, activity) are left out: they cannot be
/// non-empty while `documents` is empty, so they would only add ways to be wrong.
fn db_holds_user_data(conn: &rusqlite::Connection) -> bool {
    // Each on its own row so a failure is per-question, and every failure counts as "yes".
    const ANY_ROW: &[&str] = &[
        "documents",
        "projects",
        "project_milestones",
        "flags",
        "preferences",
        "connector_sources",
        "calendars",
        "calendar_events",
        "conversations",
        "chat_sessions",
    ];
    for table in ANY_ROW {
        let q = format!("SELECT EXISTS(SELECT 1 FROM {table})");
        if conn
            .query_row(&q, [], |r| r.get::<_, bool>(0))
            .unwrap_or(true)
        {
            return true;
        }
    }
    // The seeded inbox is not user intent; a renamed or merged entity is.
    let entities = "SELECT EXISTS(SELECT 1 FROM entities \
                      WHERE NOT (type = 'project' AND canonical_name = 'Unsorted'))";
    // The pinboard is the one intent that lives ONLY in `settings`. Matched by key rather than by an
    // exclusion list, because boot writes at least five keys of its own and that list would rot.
    let pinboard = "SELECT EXISTS(SELECT 1 FROM settings \
                      WHERE key = 'pinboard' AND trim(COALESCE(value, '')) <> '')";
    for q in [entities, pinboard] {
        if conn
            .query_row(q, [], |r| r.get::<_, bool>(0))
            .unwrap_or(true)
        {
            return true;
        }
    }
    false
}

/// Whether this profile's DEFAULT home slot holds only a pristine (empty, device-mode) vault — the one
/// case where re-homing a restored vault may replace it. A missing vault (free slot), a passphrase
/// (shareable) home vault, one PM can't open/inspect, or one that already holds any of the user's own
/// work ([`db_holds_user_data`]) ALL read as NOT pristine, so a restore never clobbers real data — it
/// falls back to running from its folder.
pub(super) fn home_is_pristine(data_dir: &std::path::Path) -> Result<bool> {
    let Some(meta) = vault::load_meta(data_dir)? else {
        return Ok(true); // no vault at the default location → the slot is free
    };
    if meta.key_mode != vault::KeyMode::Device {
        return Ok(false); // a passphrase home vault is a deliberate, real vault — never clobber it
    }
    let Some(key) = vault::current_db_key(&meta)? else {
        return Ok(false); // can't resolve its key → treat as real, leave it alone
    };
    let Ok(conn) = crate::db::open(&data_dir.join(vault::DB_FILENAME), key.expose()) else {
        return Ok(false); // unreadable → treat as real, never clobber
    };
    Ok(!db_holds_user_data(&conn))
}

/// Reconcile a just-activated restored vault to THIS machine (blocking; runs off the async thread).
///
/// Three things happen, in order:
/// 1. **Re-home** — when the vault sits in the restore-staging folder AND the home slot is a pristine
///    default vault, vacate that empty default and relocate the restored vault into the profile's
///    default location (via the crash-safe, journaled [`migrate_vault`]), so it becomes the local
///    vault instead of a pointer into a "staging" folder. Falls back to running from the folder when
///    home already holds real data.
/// 2. **Private vs passphrase** — a restored passphrase ("shareable") vault is made private on this
///    device when `make_private` (re-key to a device key, decrypt notes at rest), or kept
///    passphrase-protected otherwise. A restored device vault is already private.
/// 3. **Normalize identity** — always drop the source `owner_sid` and re-stamp the meta MAC, so a
///    foreign Windows account SID never rides along (see [`vault::normalize_adopted_meta`]).
fn adopt_restored_vault(
    app: &AppHandle,
    staging_root: &std::path::Path,
    restored_meta: &vault::VaultMeta,
    data_dir: &std::path::Path,
    make_private: bool,
) -> Result<Vec<String>> {
    let restored_is_passphrase = restored_meta.key_mode == vault::KeyMode::Passphrase;
    // A restored device vault is already private; a passphrase vault becomes private only if asked.
    let target_private = make_private || !restored_is_passphrase;

    // Re-home only a genuine restore-staging vault, and only onto a pristine home slot. The
    // staging prefix gate fronts destructive steps (vacating home, `remove_dir_all`, the relocate
    // that deletes the source), so reject any `..` component first: `Path::starts_with` is
    // component-wise and would otherwise let a crafted `…/restored-vaults/<ts>/../../elsewhere`
    // satisfy the prefix while the OS resolves it out of the staging tree. A real restore target
    // (`data_dir/restored-vaults/restore-<ts>`) never contains `..`.
    let has_parent_traversal = staging_root
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir));
    let is_restore_staging = !has_parent_traversal
        && staging_root.starts_with(data_dir.join(crate::wipe::RESTORE_STAGING_DIR));
    let rehome = is_restore_staging && home_is_pristine(data_dir)?;

    // Vacate the pristine default so the relocate's collision guard sees a Vacant home. Only reached
    // once `home_is_pristine` has confirmed it's an empty device vault — safe to drop.
    if rehome {
        crate::vault::migrate::delete_vault_artifacts(data_dir);
    }

    let mut warnings = Vec::new();
    // Migrate when re-homing (a location move) or when converting a passphrase vault to private. A
    // keep-passphrase-in-place restore needs no migration — only the identity normalize below.
    let needs_migration = rehome || (target_private && restored_is_passphrase);
    if needs_migration {
        let target_location = rehome.then(|| data_dir.to_path_buf());
        // BOTH arms pass `new_passphrase: None`, so neither reaches `prepare_shareable` and `owner`
        // is never read on this path — it is here to satisfy the compiler, not to decide anything.
        // Do not "fix" it to something else: the restore's ownership answer is `normalize_adopted_meta`
        // a few lines below, which clears the owner (and any transfer record) outright, because a SID
        // from the machine the backup came off cannot mean anything on this one.
        let plan = if target_private {
            crate::vault::migrate::MigrationPlan {
                target_key_mode: vault::KeyMode::Device,
                new_passphrase: None,
                target_markdown: vault::MarkdownEncryption::None,
                target_location,
                owner: vault::OwnerOnRekey::Keep, // inert — see above
            }
        } else {
            crate::vault::migrate::MigrationPlan {
                target_key_mode: vault::KeyMode::Passphrase,
                new_passphrase: None, // keep the restored key — this is a location move only
                target_markdown: vault::MarkdownEncryption::XChaCha20Poly1305,
                target_location,
                owner: vault::OwnerOnRekey::Keep, // inert — see above
            }
        };
        warnings = crate::vault::migrate::migrate_vault(app, plan)?;
    }

    // Normalize the FINAL vault's metadata (owner_sid + MAC). `resolve` reads the pointer, which the
    // relocate flipped to the home location (or which still names the staging folder when not re-homed).
    let final_resolved = vault::resolve(app)?;
    if let Some(final_meta) = vault::load_meta(&final_resolved.vault_root)? {
        if let Some(key) = vault::current_db_key(&final_meta)? {
            let master = vault::master_from_db_key_hex(key.expose())?;
            vault::normalize_adopted_meta(&final_resolved.vault_root, &master)?;
        }
    }

    // A re-home lands the vault at the default location — drop the (now-redundant) pointer so the UI
    // treats it as the local vault, not a "pointed"/joined one, and clear the emptied staging folder.
    if rehome {
        let _ = vault::pointer::clear(data_dir);
        let _ = std::fs::remove_dir_all(staging_root);
    }

    Ok(warnings)
}

/// Commit a restored (or otherwise relocated) vault as this profile's active vault, then reconcile it
/// to this machine. `make_private` decides whether a restored passphrase ("shareable") vault is made
/// private on this device or kept passphrase-protected (ignored for a device-mode restore). The key
/// stashed in memory by the restore is promoted to the keychain here — the deliberate commit point.
#[tauri::command]
pub async fn switch_to_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
    make_private: bool,
) -> Result<()> {
    // L-5: `folder` is a webview string pointing at an existing vault folder — require a real,
    // absolute, well-formed location before we open `folder/pm.sqlite` and promote its key.
    pathguard::sanitize_source(&folder)?;
    let root = std::path::PathBuf::from(&folder);
    let data_dir = paths::data_dir(&app)?;
    let meta = vault::load_meta(&root)?
        .ok_or_else(|| Error::Other("no PM vault found in that folder".into()))?;
    // If this folder was just restored, promote its stashed key into the keychain NOW (the
    // user is committing to it), so `open_at_boot` below can open it. Removing it from the
    // pending map also means it isn't seeded twice.
    let pending = state
        .pending_restore_keys
        .lock()
        .ok()
        .and_then(|mut m| m.remove(&folder));
    if let Some(key) = pending {
        secrets::set_cached_vault_key(&meta.vault_id, key.expose())?;
    }
    let resolved = vault::resolve_layout(&root, None);
    let (conn, master, report) = vault::open_at_boot(&resolved, &meta)?.ok_or_else(|| {
        Error::Other(
            "this vault's key isn't available on this device; restore it from a backup first"
                .into(),
        )
    })?;
    // A different vault from here on, so the previous one's tamper notice no longer applies.
    state.clear_meta_warning();
    state.note_meta_report(&report);
    let runtime = VaultRuntime::build(&resolved, &meta, &master);
    // Point this profile here, then install the new session — `attach_profile_here`
    // stores the pointer first (the next launch reads it), and `open_session` swaps
    // `db` + `vault` together and drops the old connection, so there's no
    // locked-in-between window. This makes the restored vault the active vault that
    // `adopt_restored_vault` then re-homes / normalizes.
    attach_profile_here(&app, &state, root.clone(), conn, runtime)?;

    // Reconcile to this machine (re-home + private/normalize). Heavy work (a full re-key / copy) runs
    // off the async runtime thread; it reads `AppState` back through the `AppHandle`, like the other
    // migration commands.
    let app2 = app.clone();
    let mut warnings = spawn_blocking_result("adopt", move || {
        adopt_restored_vault(&app2, &root, &meta, &data_dir, make_private)
    })
    .await?;
    // Re-engage the writer lock for the final (possibly relocated) vault — best-effort, mirroring the
    // other mode-change commands (a device vault needs none; a passphrase vault does).
    engage_or_warn(&app, &mut warnings);

    // Committed: drop the staged-restore banner so a reopened Backup panel doesn't offer to
    // "switch" to the vault that's now already active.
    if let Ok(mut snap) = state.backup_state.lock() {
        snap.pending_restore = None;
    }
    Ok(())
}

/// Acknowledge the last run's outcome, so its banner stops coming back.
///
/// The partial-failure banner is a REPLAY: `backup_status` hands the mount effect the stored
/// `last_report`, and only a new run ever overwrote it. Nothing re-derived it from reality, so a
/// user who fixed the problem out-of-band — deleting the extra archives in Drive, say — kept being
/// told about a failure that no longer existed, across tab switches and app restarts alike. A
/// frontend-only dismiss cannot fix that; the next mount replays it.
///
/// Acknowledge rather than re-check, deliberately: PM cannot cheaply re-verify a genuine upload
/// failure (that would mean uploading again), so "I've seen this" is the honest primitive. The
/// retention half IS re-derivable and heals itself against a fresh listing instead — see
/// `RetentionNote::over_limit`.
#[tauri::command]
pub fn clear_backup_report(state: State<'_, AppState>) -> Result<()> {
    if let Ok(mut snap) = state.backup_state.lock() {
        snap.last_report = None;
    }
    Ok(())
}

/// The current backup/restore snapshot (empty / `running:false` when idle), so the
/// Backup settings UI can resume showing progress after the user leaves and returns.
#[tauri::command]
pub fn backup_status(state: State<'_, AppState>) -> Result<crate::BackupState> {
    state
        .backup_state
        .lock()
        .map(|s| s.clone())
        .map_err(|_| Error::Other("backup state lock poisoned".into()))
}

/// Cooperatively cancel the running backup/restore (checked between reads). A no-op when
/// nothing is running.
#[tauri::command]
pub fn stop_backup(state: State<'_, AppState>) {
    state.backup_cancel.store(true, Ordering::SeqCst);
}

/// Whether the official `proton-drive` CLI is installed (for backing up to Proton Drive).
/// PM does not bundle or download the CLI — when it's missing, the UI links the user to
/// `install_url` to install the official signed build themselves (the locate-then-guide
/// model). `path` is the resolved executable when found.
#[derive(Serialize)]
pub struct ProtonCliStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub install_url: String,
}

/// The manual "Locate the CLI" override the user set in the Backup UI, if any and still valid — read
/// from the store's settings. `None` when unset, when the store isn't open, or when the saved path no
/// longer points at a file (a moved/deleted binary falls back to auto-detection rather than erroring).
fn proton_cli_override(state: &AppState) -> Option<std::path::PathBuf> {
    let conn = state.conn().ok()?;
    let raw = crate::db::get_setting(&conn, crate::backup::proton::CLI_PATH_SETTING).ok()??;
    let path = std::path::PathBuf::from(raw);
    path.is_file().then_some(path)
}

/// Probe for the Proton Drive CLI — a manual override first, then PATH + well-known install and
/// download dirs. Cheap (a few `stat`s, no process spawn), so the Backup UI can call it on mount and
/// re-call it (a "Check again" button / on window focus) after the user installs it, no restart.
#[tauri::command]
pub fn proton_cli_status(state: State<'_, AppState>) -> ProtonCliStatus {
    let override_path = proton_cli_override(&state);
    let located = crate::backup::proton::locate_proton_cli(override_path.as_deref());
    ProtonCliStatus {
        installed: located.is_some(),
        path: located.map(|p| p.to_string_lossy().into_owned()),
        install_url: crate::backup::proton::INSTALL_URL.to_string(),
    }
}

/// Remember a manual path to the `proton-drive` binary — the escape hatch for when the portable CLI
/// lives somewhere auto-detection doesn't look.
///
/// L-5: the stored path is later handed to `Command::new(...)` and SPAWNED, so a webview-supplied
/// string here is a code-execution sink that no amount of after-the-fact string validation can
/// close (a compromised webview could name any real executable). We therefore open the native file
/// picker in the BACKEND and use its result directly — the chosen path never round-trips through the
/// webview. Cancelling leaves the current setting untouched. The dialog is run on the blocking pool,
/// not the main thread (a blocking pick on the main thread would deadlock the event loop).
#[tauri::command]
pub async fn set_proton_cli_path(app: AppHandle) -> Result<()> {
    use tauri_plugin_dialog::DialogExt;
    let app2 = app.clone();
    // Deliberately NOT `blocking::spawn_blocking_result`: the picker returns an `Option`, not a
    // `Result`, so it does not fit the helper's bound — and its message says "failed", not
    // "panicked", which converting would silently retype.
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .file()
            .set_title("Locate the proton-drive program")
            .blocking_pick_file()
    })
    .await
    .map_err(|e| Error::Other(format!("file dialog task failed: {e}")))?;
    let Some(picked) = picked else {
        return Ok(()); // cancelled — keep the current setting
    };
    let path = picked
        .into_path()
        .map_err(|e| Error::Other(format!("couldn't read the chosen file path: {e}")))?;
    if !path.is_file() {
        return Err(Error::Other(
            "That isn't a file — pick the proton-drive program itself.".into(),
        ));
    }
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    crate::db::set_setting(
        &conn,
        crate::backup::proton::CLI_PATH_SETTING,
        &path.to_string_lossy(),
    )
}

/// Resolve the CLI (honouring a manual override) or return a friendly "not installed" error (shared
/// by every Proton command below).
fn require_proton_cli(state: &AppState) -> Result<std::path::PathBuf> {
    crate::backup::proton::locate_proton_cli(proton_cli_override(state).as_deref())
        .ok_or_else(|| Error::Other("the Proton Drive CLI is not installed".into()))
}

/// The automatic-backup schedule shown in Settings. `passphrase_stored` reflects the keychain
/// opt-in (a non-`off` frequency requires it); `last_backup_at` is RFC3339 or null. The one cadence
/// fans out to every enabled + ready destination: `proton_enabled` (default on) and `gdrive_enabled`
/// (opt-in, requires a granted `gdrive_account`).
#[derive(Serialize)]
pub struct BackupSchedule {
    pub frequency: String,
    pub retention_n: u32,
    pub passphrase_stored: bool,
    pub last_backup_at: Option<String>,
    /// Whether scheduled runs push to Proton Drive (defaults on — preserves prior behavior).
    pub proton_enabled: bool,
    /// Whether scheduled runs push to Google Drive (opt-in).
    pub gdrive_enabled: bool,
    /// The Google account chosen for backup (email), or null if none is set up.
    pub gdrive_account: Option<String>,
    /// Per-destination last-success stamps (F-22, RFC3339 or null). Distinct from `last_backup_at`
    /// (the shared cadence clock), these let Settings show that one destination has gone stale while a
    /// sibling keeps succeeding — the silent-staleness the shared stamp hid.
    pub proton_last_backup_at: Option<String>,
    pub gdrive_last_backup_at: Option<String>,
}

/// Read the current automatic-backup schedule (cadence + retention + opt-in state + last run +
/// per-destination enable flags).
#[tauri::command]
pub fn get_backup_schedule(state: State<'_, AppState>) -> Result<BackupSchedule> {
    use crate::backup::schedule::{
        setting_bool, BACKUP_FREQUENCY_KEY, BACKUP_GDRIVE_ACCOUNT_KEY, BACKUP_GDRIVE_ENABLED_KEY,
        BACKUP_PROTON_ENABLED_KEY, BACKUP_RETENTION_KEY, DEFAULT_RETENTION_N, LAST_BACKUP_AT_KEY,
    };
    let conn = state.conn()?;
    Ok(BackupSchedule {
        frequency: crate::db::get_setting(&conn, BACKUP_FREQUENCY_KEY)?
            .unwrap_or_else(|| "off".into()),
        retention_n: crate::db::get_setting(&conn, BACKUP_RETENTION_KEY)?
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(DEFAULT_RETENTION_N),
        passphrase_stored: secrets::get_backup_passphrase()?.is_some(),
        last_backup_at: crate::db::get_setting(&conn, LAST_BACKUP_AT_KEY)?,
        proton_enabled: setting_bool(&conn, BACKUP_PROTON_ENABLED_KEY, true),
        gdrive_enabled: setting_bool(&conn, BACKUP_GDRIVE_ENABLED_KEY, false),
        gdrive_account: crate::db::get_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY)?
            .filter(|s| !s.is_empty()),
        proton_last_backup_at: crate::db::get_setting(
            &conn,
            &crate::backup::schedule::last_backup_at_key("proton"),
        )?,
        gdrive_last_backup_at: crate::db::get_setting(
            &conn,
            &crate::backup::schedule::last_backup_at_key("gdrive"),
        )?,
    })
}

/// Set the automatic-backup cadence + retention. A non-`off` cadence requires a stored passphrase
/// (unattended runs can't prompt), so this refuses to enable automation until one is saved.
#[tauri::command]
pub fn set_backup_schedule(
    state: State<'_, AppState>,
    frequency: String,
    retention_n: u32,
) -> Result<()> {
    use crate::backup::schedule::{Frequency, BACKUP_FREQUENCY_KEY, BACKUP_RETENTION_KEY};
    let freq = Frequency::from_setting(&frequency);
    if freq != Frequency::Off && secrets::get_backup_passphrase()?.is_none() {
        return Err(Error::Other(
            "save a backup passphrase before turning on automatic backups".into(),
        ));
    }
    let retention_n = retention_n.max(1);
    let conn = state.conn()?;
    crate::db::set_setting(&conn, BACKUP_FREQUENCY_KEY, freq.as_setting())?;
    crate::db::set_setting(&conn, BACKUP_RETENTION_KEY, &retention_n.to_string())?;
    Ok(())
}

/// Store the backup passphrase in the OS keychain for unattended (scheduled) backups. Explicit
/// opt-in — manual backups never read this.
#[tauri::command]
pub fn set_backup_passphrase(passphrase: String) -> Result<()> {
    // I-03/L-1: wipe the passphrase plaintext from memory on return (the keychain write borrows it).
    let passphrase = zeroize::Zeroizing::new(passphrase);
    if passphrase.is_empty() {
        return Err(Error::Other("a backup passphrase is required".into()));
    }
    // M-4: strength floor at the storage seam. The scheduler reads this stored passphrase and hands it
    // to run_backup for unattended backups, so validating here covers scheduled runs — and keeps the
    // floor off run_backup itself, which must still accept an already-stored (pre-floor) passphrase.
    vault::kdf::validate_passphrase_strength(&passphrase)?;
    secrets::set_backup_passphrase(&passphrase)
}

/// Forget the stored backup passphrase and turn automatic backups off (they can't run without it).
#[tauri::command]
pub fn forget_backup_passphrase(state: State<'_, AppState>) -> Result<()> {
    use crate::backup::schedule::{Frequency, BACKUP_FREQUENCY_KEY};
    // Turn automation OFF first, THEN drop the passphrase — so a failure between the two can never
    // leave "cadence != off" with no stored passphrase (the state the scheduler must never see).
    {
        let conn = state.conn()?;
        crate::db::set_setting(&conn, BACKUP_FREQUENCY_KEY, Frequency::Off.as_setting())?;
    }
    secrets::delete_backup_passphrase()
}

/// Sign in to Proton Drive — opens the browser and blocks until the flow completes. The
/// session is stored and owned by the CLI (OS secret store); PM never sees Proton credentials.
#[tauri::command]
pub async fn proton_connect(state: State<'_, AppState>) -> Result<()> {
    let cli = require_proton_cli(&state)?;
    spawn_blocking_result("connect", move || crate::backup::proton::connect(&cli)).await
}

/// Sign out of Proton Drive (`auth logout`).
#[tauri::command]
pub async fn proton_disconnect(state: State<'_, AppState>) -> Result<()> {
    let cli = require_proton_cli(&state)?;
    spawn_blocking_result("disconnect", move || {
        crate::backup::proton::disconnect(&cli)
    })
    .await
}

/// Whether the CLI has an active Proton session (+ the account email if available). A clean
/// "not signed in" is reported as `connected: false`, not an error.
#[tauri::command]
pub async fn proton_status(
    state: State<'_, AppState>,
) -> Result<crate::backup::proton::ProtonConnStatus> {
    let cli = require_proton_cli(&state)?;
    spawn_blocking_result("status", move || {
        Ok(crate::backup::proton::connection(&cli))
    })
    .await
}

/// List PM's encrypted archives already on Proton Drive (newest first), for the restore picker.
#[tauri::command]
pub async fn list_proton_backups(
    state: State<'_, AppState>,
) -> Result<Vec<crate::backup::naming::BackupEntry>> {
    let cli = require_proton_cli(&state)?;
    spawn_blocking_result("list", move || crate::backup::proton::list_archives(&cli)).await
}

/// Shared core: snapshot the DB under the lock, then — off the lock — pack ONE `.pmbackup` and push
/// the same blob to every destination in `targets`, emitting the detached `backup://progress`
/// events. `retention` (when `Some(n)`) trims each destination to keep-last-N after its upload.
/// Reused by the manual `backup_to_proton` / `backup_to_gdrive` commands (one target, no retention)
/// and the scheduler ([`crate::backup::schedule`], the enabled set + retention). Single-flight via
/// the `backup_busy` guard.
///
/// For a SINGLE target this is byte-for-byte the prior single-destination behavior: the Upload
/// phase brackets `0.0 → 1.0`, `last_backup_at` is stamped on success, a failure emits `Failed`.
/// With several targets, `last_backup_at` is stamped (and `Finished` emitted) if ANY succeeded;
/// per-destination failures are logged, and a total failure emits `Failed` + errors so the
/// scheduler stays due and retries.
pub(crate) async fn run_backup(
    app: &AppHandle,
    passphrase: String,
    targets: Vec<BackupDestination>,
    retention: Option<u32>,
) -> Result<String> {
    // I-03: wipe the passphrase plaintext on return. This is the shared multi-destination path — the
    // scheduler reaches it by cloning the passphrase out of its keychain `Secret` (schedule.rs), so
    // that transient copy is owned (and zeroized) here rather than lingering on the stack.
    let passphrase = zeroize::Zeroizing::new(passphrase);
    if targets.is_empty() {
        return Err(Error::Other("no backup destination selected".into()));
    }
    // `app` is borrowed (not owned) so the `State` we derive from it borrows *through* the
    // reference — holding it across the `.await` below is fine, whereas an owned `app` would make
    // this future self-referential.
    let state = app.state::<AppState>();
    let _busy = begin_backup_run(app, &state, BackupPhase::Snapshot)?;

    let tmp = tempfile::Builder::new().prefix("pm-backup-").tempdir()?;
    let snapshot = tmp.path().join(vault::DB_FILENAME);
    {
        // Snapshot on the blocking pool (F-42): a `VACUUM INTO` of a multi-GB store must not pin a
        // tokio worker or hold the DB mutex on the async runtime. The guard is acquired and dropped
        // inside the closure (DbGuard is !Send) via a cloned handle; `snapshot` is cloned in and the
        // original flows into the pack inputs below.
        let app = app.clone();
        let snapshot = snapshot.clone();
        spawn_blocking_result("snapshot", move || -> Result<()> {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            vault::migrate::vacuum_into(&conn, &snapshot)
        })
        .await?;
    }
    emit_backup_progress(
        app,
        BackupEvent::Phase {
            phase: BackupPhase::Snapshot,
            fraction: 1.0,
        },
    );

    let resolved = vault::resolve(app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata to back up".into()))?;
    let db_key = vault::current_db_key(&meta)?
        .ok_or_else(|| Error::Other("unlock the vault before backing it up".into()))?;
    let now = chrono::Utc::now();
    let archive_name = crate::backup::naming::archive_name(
        &meta.vault_id,
        &now.format("%Y%m%dT%H%M%SZ").to_string(),
    );
    let archive_path = tmp.path().join(&archive_name);
    let inputs = backup::pack::PackInputs {
        vault_root: resolved.vault_root.clone(),
        db_snapshot: snapshot,
        markdown_dir: resolved.markdown_dir.clone(),
        meta: meta.clone(),
        db_key_hex: db_key,
        app_version: app.package_info().version.to_string(),
        created_at: now.to_rfc3339(),
    };
    let vault_id = meta.vault_id.clone();

    // Pack ONCE (blocking) — the destination-agnostic archive is written to `archive_path`.
    let app2 = app.clone();
    let archive_path2 = archive_path.clone();
    let pack_result = spawn_blocking_join("backup", move || {
        let st = app2.state::<AppState>();
        let report = |phase, fraction| {
            emit_backup_progress(&app2, BackupEvent::Phase { phase, fraction });
        };
        backup::pack::pack(
            inputs,
            &archive_path2,
            &passphrase,
            report,
            &st.backup_cancel,
        )
    })
    .await?;

    if let Err(e) = pack_result {
        drop(tmp);
        let msg = if state.backup_cancel.load(Ordering::SeqCst) {
            "Backup cancelled.".to_string()
        } else {
            e.to_string()
        };
        emit_backup_progress(
            app,
            BackupEvent::Failed {
                message: msg.clone(),
            },
        );
        return Err(Error::Other(msg));
    }

    // Fan out: push the SAME blob to each target, then (optionally) trim it. Upload progress
    // brackets each destination's slice of 0.0..=1.0 (so a single target reads exactly 0→1).
    let n = targets.len();
    let prefix = crate::backup::naming::archive_prefix(&vault_id);
    let mut any_ok = false;
    let mut succeeded: Vec<&'static str> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    // Kept apart from `failures` deliberately: retention trouble happens INSIDE the per-destination
    // success arm, so it can never mean "this destination failed". Conflating them is what made a
    // clean backup report a failure.
    let mut retention_notes: Vec<crate::backup::RetentionNote> = Vec::new();
    for (i, dest) in targets.iter().enumerate() {
        // Honour Cancel between destinations (F-13): a hit during one upload stops the fan-out
        // before the next starts. With no destination yet succeeded, `any_ok` stays false and the
        // post-loop "Backup cancelled." branch reports it.
        if state.backup_cancel.load(Ordering::SeqCst) {
            break;
        }
        emit_backup_progress(
            app,
            BackupEvent::Phase {
                phase: BackupPhase::Upload,
                fraction: i as f32 / n as f32,
            },
        );
        match dest.upload(app, &archive_path, &archive_name).await {
            Ok(()) => {
                any_ok = true;
                succeeded.push(dest.kind());
                emit_backup_progress(
                    app,
                    BackupEvent::Phase {
                        phase: BackupPhase::Upload,
                        fraction: (i + 1) as f32 / n as f32,
                    },
                );
                if let Some(keep_n) = retention {
                    // A retention problem never fails the backup — the archive is already safely
                    // uploaded — but it must not be invisible either, or old archives pile up in
                    // silence until the reconciliation banner notices.
                    //
                    // These used to be pushed into `failures`, which is the vec the UI renders as
                    // "Backed up, but N destinations failed". So a destination whose upload had
                    // SUCCEEDED was reported as having failed, and the sentence the user read
                    // described something that never happened. They get their own field now:
                    // `over_limit` marks the count fact, which a later listing can heal, apart
                    // from a trim that errored, which it cannot.
                    match dest.apply_retention(keep_n as usize, &prefix).await {
                        Ok(o) if o.skipped > 0 => {
                            retention_notes.push(crate::backup::RetentionNote {
                                kind: dest.kind().to_string(),
                                message: format!(
                                    "{}: {}",
                                    dest.label(),
                                    retention_refusal_message(o.skipped)
                                ),
                                over_limit: true,
                            });
                        }
                        Ok(o) if o.trashed > 0 => {
                            eprintln!(
                                "backup: trimmed {} old archive(s) on {}",
                                o.trashed,
                                dest.label()
                            )
                        }
                        Ok(_) => {}
                        Err(e) => retention_notes.push(crate::backup::RetentionNote {
                            kind: dest.kind().to_string(),
                            message: format!("{}: trimming old backups failed: {e}", dest.label()),
                            // A transport failure, not a count fact — a fresh listing showing the
                            // destination under its limit says nothing about whether the trim
                            // would work now, so this must never be auto-suppressed.
                            over_limit: false,
                        }),
                    }
                }
            }
            Err(e) => failures.push(format!("{}: {e}", dest.label())),
        }
    }
    drop(tmp);

    if any_ok {
        // Stamp last-run for BOTH manual and scheduled backups, so the cadence clock advances (a
        // manual backup "counts") and Settings reflects it. Best-effort — a vault that locked
        // during the upload just leaves the stamp for next time.
        if let Ok(conn) = state.conn() {
            let stamp = now.to_rfc3339();
            let _ =
                crate::db::set_setting(&conn, crate::backup::schedule::LAST_BACKUP_AT_KEY, &stamp);
            // F-22: also stamp each destination that succeeded THIS run under its own key, so a sibling
            // that persistently fails goes visibly stale instead of hiding behind the shared stamp above.
            for kind in &succeeded {
                let _ = crate::db::set_setting(
                    &conn,
                    &crate::backup::schedule::last_backup_at_key(kind),
                    &stamp,
                );
            }
        }
        if !failures.is_empty() {
            eprintln!("backup: some destinations failed: {}", failures.join("; "));
        }
        emit_backup_progress(
            app,
            BackupEvent::Finished {
                report: BackupReport {
                    kind: BackupKind::Backup,
                    vault_id: Some(vault_id.clone()),
                    target_dir: None,
                    created_at: None,
                    // F-22: surface the partial failure so the UI can show a non-blocking banner rather
                    // than a silent success. Empty on a clean run.
                    failed_destinations: failures.clone(),
                    // Separate sentence, separate field: these destinations were backed up fine.
                    retention_notes: retention_notes.clone(),
                },
            },
        );
        Ok(vault_id)
    } else {
        let msg = if state.backup_cancel.load(Ordering::SeqCst) {
            "Backup cancelled.".to_string()
        } else if failures.is_empty() {
            "Backup failed.".to_string()
        } else {
            failures.join("; ")
        };
        emit_backup_progress(
            app,
            BackupEvent::Failed {
                message: msg.clone(),
            },
        );
        Err(Error::Other(msg))
    }
}

/// Build a single backup destination from its stable kind ("proton" | "gdrive") — the same keys
/// `BackupDestination::kind()` reports. Proton needs the located CLI; Google Drive needs the backup
/// account's token key. Shared by the per-destination "Back up now" + reconciliation commands.
fn backup_destination_for(
    kind: &str,
    state: &AppState,
    app: &AppHandle,
) -> Result<BackupDestination> {
    match kind {
        "proton" => Ok(BackupDestination::Proton {
            cli: require_proton_cli(state)?,
        }),
        "gdrive" => Ok(BackupDestination::GoogleDrive {
            token_key: gdrive_backup_token_key(app)?,
        }),
        other => Err(Error::Other(format!("unknown backup destination: {other}"))),
    }
}

/// This vault's archive-name prefix (`pm-backup-<vaultId>-`), so a count/prune only ever considers
/// archives THIS vault created — never another device/vault sharing the same account + folder.
fn current_vault_prefix(app: &AppHandle) -> Result<String> {
    let resolved = vault::resolve(app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    Ok(crate::backup::naming::archive_prefix(&meta.vault_id))
}

/// The current keep-last-N retention, defaulting exactly like the scheduler and Settings UI.
fn backup_retention_n(state: &AppState) -> Result<u32> {
    use crate::backup::schedule::{BACKUP_RETENTION_KEY, DEFAULT_RETENTION_N};
    let conn = state.conn()?;
    Ok(crate::db::get_setting(&conn, BACKUP_RETENTION_KEY)?
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_RETENTION_N))
}

/// Back up this vault to ONE already-connected destination now, using the STORED passphrase and
/// pruning to keep-last-N (like a scheduled run). `destination` is the stable kind ("proton" |
/// "gdrive"). Distinct from `backup_to_proton`/`backup_to_gdrive` (typed passphrase, no prune) — this
/// is the connected-panel "Back up now" that only appears once a passphrase is remembered.
#[tauri::command]
pub async fn backup_now(app: AppHandle, destination: String) -> Result<()> {
    let pass = secrets::get_backup_passphrase()?.ok_or_else(|| {
        Error::Other("turn on \"remember passphrase\" before using Back up now".into())
    })?;
    // Build the destination + read retention under a short-lived state guard, dropped before the
    // long `run_backup` await so nothing non-Send is held across it.
    let (dest, retention_n) = {
        let state = app.state::<AppState>();
        (
            backup_destination_for(&destination, &state, &app)?,
            backup_retention_n(&state)?,
        )
    };
    run_backup(
        &app,
        pass.expose().to_string(),
        vec![dest],
        Some(retention_n),
    )
    .await
    .map(|_| ())
}

/// This vault's backup archive-name prefix (`pm-backup-<vaultId>-`), so the UI can tell THIS vault's
/// archives apart from any other vault sharing the same account/folder when it counts them against
/// keep-last-N for the reconciliation banner. Not sensitive — the same prefix already appears in
/// every archive name shown in the restore list.
#[tauri::command]
pub fn backup_archive_prefix(app: AppHandle) -> Result<String> {
    current_vault_prefix(&app)
}

/// What to tell the user when a destination refused PM write access to `n` of the archives it chose
/// to trim. Kept as one function so the scheduled path and the manual button say the same thing.
fn retention_refusal_message(n: usize) -> String {
    format!(
        "PM can only remove backups it uploaded with the current Google sign-in. \
         {n} older archive{} left in place — delete {} in Google Drive if you want the space back.",
        if n == 1 { "" } else { "s" },
        if n == 1 { "it" } else { "them" },
    )
}

/// Prune this vault's backups at a destination to keep-last-N now — the reconciliation banner's
/// "delete oldest" action. Recoverable (Proton Trash / Drive trash), never a hard delete; only this
/// vault's archives (by prefix) are considered.
///
/// Returns the full outcome rather than a bare count: a Google Drive destination can refuse PM write
/// access to an archive it can nevertheless see and list, and "trimmed 0" alone is indistinguishable
/// from "nothing was over the limit".
#[tauri::command]
pub async fn prune_own_backups(app: AppHandle, destination: String) -> Result<RetentionOutcome> {
    let (dest, prefix, keep_n) = {
        let state = app.state::<AppState>();
        (
            backup_destination_for(&destination, &state, &app)?,
            current_vault_prefix(&app)?,
            backup_retention_n(&state)?,
        )
    };
    dest.apply_retention(keep_n as usize, &prefix).await
}

/// Create an encrypted archive and push it to Proton Drive. Same portable format as a local
/// backup; the temp file never leaves the machine unencrypted and is discarded after upload.
#[tauri::command]
pub async fn backup_to_proton(
    app: AppHandle,
    state: State<'_, AppState>,
    passphrase: String,
) -> Result<()> {
    if passphrase.is_empty() {
        return Err(Error::Other("a backup passphrase is required".into()));
    }
    // M-4: strength floor before the archive (which embeds the raw DB key) leaves the machine.
    vault::kdf::validate_passphrase_strength(&passphrase)?;
    let cli = require_proton_cli(&state)?;
    run_backup(
        &app,
        passphrase,
        vec![BackupDestination::Proton { cli }],
        None,
    )
    .await
    .map(|_| ())
}

/// Download an archive from Proton Drive and restore it into a fresh, validated folder (the
/// live vault is untouched until the user switches, exactly like a local restore). `name` is a
/// bare archive file name from `list_proton_backups`.
#[tauri::command]
pub async fn restore_from_proton(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    passphrase: String,
) -> Result<RestoreSummary> {
    // I-03/L-1: wipe the passphrase plaintext on return (it is consumed by the blocking restore below).
    let passphrase = require_backup_passphrase(passphrase)?;
    let dest = BackupDestination::Proton {
        cli: require_proton_cli(&state)?,
    };
    let _busy = begin_backup_run(&app, &state, BackupPhase::Download)?;
    let target = restore_staging_target(&app)?;

    // The download goes through the destination enum (which threads `backup_cancel` into the CLI
    // child, F-13) rather than calling `proton::download_archive` directly, so Proton and Drive
    // restores now differ only in the destination value. Both steps are reported through
    // `unwrap_restore_result` because they used to sit inside the same blocking task as the restore
    // itself: a failure here must still emit the detached `Failed` event and still read a
    // user-initiated Cancel as "Restore cancelled." rather than as the incidental IO error.
    let dl = unwrap_restore_result(&app, &state, restore_scratch_dir(&dest))?;
    unwrap_restore_result(&app, &state, dest.download(&app, &name, dl.path()).await)?;
    emit_backup_progress(
        &app,
        BackupEvent::Phase {
            phase: BackupPhase::Download,
            fraction: 1.0,
        },
    );

    let app2 = app.clone();
    let target2 = target.clone();
    let local = dl.path().join(&name);
    let result = spawn_blocking_join("restore", move || {
        let st = app2.state::<AppState>();
        let report = |phase, fraction| {
            emit_backup_progress(&app2, BackupEvent::Phase { phase, fraction });
        };
        backup::restore::restore(&local, &passphrase, &target2, report, &st.backup_cancel)
    })
    .await?;
    // Only now: `dl` owns the downloaded archive the task above just read.
    drop(dl);

    let outcome = unwrap_restore_result(&app, &state, result)?;
    Ok(finalize_restore(&app, &state, outcome))
}

/// Reject an empty backup passphrase, and take ownership of the plaintext so it is zeroized on
/// return whichever way the caller leaves (I-03/L-1). Shared by the three restore commands, which
/// are the callers that require an already-existing archive's passphrase — `create_local_backup`
/// deliberately keeps its own wording ("a backup passphrase is required": it is minting one).
fn require_backup_passphrase(passphrase: String) -> Result<zeroize::Zeroizing<String>> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    if passphrase.is_empty() {
        return Err(Error::Other("the backup passphrase is required".into()));
    }
    Ok(passphrase)
}

/// Open a backup or restore run: win the single-flight guard, clear the stale cancel flag, and emit
/// the opening phase at 0.0. Every backup/restore entry point does exactly these three things.
///
/// The guard is RETURNED, never dropped here — bind it (`let _busy = …`) so RAII release still
/// happens at the end of the caller. Binding it to `_` would release `backup_busy` immediately and
/// let two runs race into the same staging tree.
///
/// The refusal names the erase as well as the two backup kinds, because `wipe::wipe_pm_data` holds
/// this same guard for its whole run (so a copy can't finish uploading after "Remove PM data"
/// reported success). Naming only "a backup or restore" would be a false claim in that case.
///
/// `phase` is a parameter because the opening phase is user-visible progress copy and the callers
/// legitimately differ: a local restore opens on `Restore` (there is nothing to download), the two
/// remote restores on `Download`, and the backup paths on `Snapshot`.
fn begin_backup_run<'a>(
    app: &AppHandle,
    state: &'a AppState,
    phase: BackupPhase,
) -> Result<BusyGuard<'a>> {
    let busy = BusyGuard::acquire(&state.backup_busy)
        .ok_or_else(|| Error::Other("a backup, restore or data erase is already running".into()))?;
    state.backup_cancel.store(false, Ordering::SeqCst);
    emit_backup_progress(
        app,
        BackupEvent::Phase {
            phase,
            fraction: 0.0,
        },
    );
    Ok(busy)
}

/// The staging folder a restore run builds into: `<data_dir>/restored-vaults/restore-<UTC stamp>`.
///
/// One place, because `adopt_restored_vault` gates its DESTRUCTIVE re-home (it vacates the default
/// home) on the target still sitting under exactly this prefix, and `wipe::sweep_restore_staging`
/// reaps the same tree at boot. A caller that minted a different shape would silently lose
/// re-homing and leave a decryptable copy behind the GC.
fn restore_staging_target(app: &AppHandle) -> Result<std::path::PathBuf> {
    let data_dir = paths::data_dir(app)?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    Ok(data_dir
        .join(crate::wipe::RESTORE_STAGING_DIR)
        .join(format!("restore-{ts}")))
}

/// A scratch directory for a remote restore to download into, named for the destination so a
/// leftover temp folder is attributable. The archive must outlive the download, so the caller owns
/// the `TempDir` and drops it only after the restore has read the file out of it.
fn restore_scratch_dir(dest: &BackupDestination) -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new()
        .prefix(&format!("pm-restore-{}-", dest.kind()))
        .tempdir()?)
}

/// Unwrap a finished restore step's result: on failure, report a user-initiated cancel as a
/// cancel (not whatever incidental error the pipeline hit when the flag flipped), emit the
/// detached `Failed` event, and surface the error. Shared by all three restore commands.
///
/// Generic over the step's payload because the Proton path routes its scratch-dir and download
/// failures through here too — they used to be `?`s inside the same blocking task as the restore,
/// so this is what keeps that command's error reporting byte-identical after the download moved to
/// the async side.
fn unwrap_restore_result<T>(app: &AppHandle, state: &AppState, result: Result<T>) -> Result<T> {
    result.map_err(|e| {
        let msg = if state.backup_cancel.load(Ordering::SeqCst) {
            "Restore cancelled.".to_string()
        } else {
            e.to_string()
        };
        emit_backup_progress(
            app,
            BackupEvent::Failed {
                message: msg.clone(),
            },
        );
        Error::Other(msg)
    })
}

/// Finish a restore (local file, Proton, or Google Drive): stash the restored key IN MEMORY only
/// (`switch_to_vault` promotes it to the keychain on commit — a restore the user inspects but
/// never switches to can't overwrite the LIVE vault's cached key), build + park the summary so a
/// remounted Backup panel can re-offer "switch to it", and emit the detached `Finished` event.
/// Shared by the three restore commands, which differ only in how they obtain the archive.
fn finalize_restore(
    app: &AppHandle,
    state: &AppState,
    outcome: crate::backup::restore::RestoreOutcome,
) -> RestoreSummary {
    let target_dir = outcome.target_dir.to_string_lossy().to_string();
    if let Ok(mut pending) = state.pending_restore_keys.lock() {
        pending.insert(target_dir.clone(), outcome.db_key_hex);
    }
    let summary = RestoreSummary {
        vault_id: outcome.vault_id.clone(),
        key_mode: outcome.key_mode,
        markdown_encryption: outcome.markdown_encryption,
        app_version: outcome.app_version,
        created_at: outcome.created_at.clone(),
        target_dir,
    };
    if let Ok(mut snap) = state.backup_state.lock() {
        snap.pending_restore = Some(summary.clone());
    }
    emit_backup_progress(
        app,
        BackupEvent::Finished {
            report: BackupReport {
                kind: BackupKind::Restore,
                vault_id: Some(outcome.vault_id),
                target_dir: Some(summary.target_dir.clone()),
                created_at: Some(outcome.created_at),
                failed_destinations: Vec::new(),
                retention_notes: Vec::new(),
            },
        },
    );
    summary
}

// --- Google Drive backup destination (drive.file re-consent + push/pull/list) --------------------

/// The Google Drive backup destination's status for the Settings UI: which account is set up, and
/// whether it has the `drive.file` write grant yet (a fresh re-consent is required — the connector
/// scopes are read-only). `accounts` is the list of connected Drive accounts for the "which
/// account?" picker on first grant.
#[derive(Serialize)]
pub struct GdriveBackupStatus {
    pub account: Option<String>,
    pub has_write_scope: bool,
    pub enabled: bool,
    pub accounts: Vec<crate::drive::DriveAccount>,
}

/// Read the Google Drive backup status from an open connection (shared by the status command and
/// the connect flow, which re-reads after recording the account).
fn read_gdrive_status(conn: &Connection) -> Result<GdriveBackupStatus> {
    use crate::backup::schedule::{
        setting_bool, BACKUP_GDRIVE_ACCOUNT_KEY, BACKUP_GDRIVE_ENABLED_KEY,
    };
    let account =
        crate::db::get_setting(conn, BACKUP_GDRIVE_ACCOUNT_KEY)?.filter(|s| !s.is_empty());
    let has_write_scope = match &account {
        Some(email) => google::token_has_scope(
            &crate::drive::account_token_key(email),
            google::DRIVE_FILE_SCOPE,
        )?,
        None => false,
    };
    Ok(GdriveBackupStatus {
        account,
        has_write_scope,
        enabled: setting_bool(conn, BACKUP_GDRIVE_ENABLED_KEY, false),
        accounts: crate::drive::list_accounts(conn)?,
    })
}

/// Whether a Google account is set up for backup, has the write grant, and is enabled (+ the list
/// of connected Drive accounts for the picker).
#[tauri::command]
pub fn backup_gdrive_status(state: State<'_, AppState>) -> Result<GdriveBackupStatus> {
    let conn = state.conn()?;
    read_gdrive_status(&conn)
}

/// Grant Google Drive backup access: run a fresh OAuth consent for the `drive.file` WRITE scope
/// (the connector scopes are read-only), learn the account it grants, and save the token under that
/// account's existing Drive key — `include_granted_scopes` UNIONS `drive.file` with any existing
/// `drive.readonly` there, so the read connector keeps working. Records the account and enables
/// Google backups. Also works as a first-connect when no Google account exists yet. If `email` is
/// given, the signed-in account must match it (so the picker's choice is honored).
#[tauri::command]
pub async fn backup_gdrive_connect(
    app: AppHandle,
    email: Option<String>,
) -> Result<GdriveBackupStatus> {
    use crate::backup::schedule::{BACKUP_GDRIVE_ACCOUNT_KEY, BACKUP_GDRIVE_ENABLED_KEY};
    // Opens the browser; unions the write scope with any existing read grant on the chosen account.
    let token = google::run_consent(google::DRIVE_FILE_SCOPE, "Google Drive backup").await?;
    let (learned_email, _name) = crate::drive::about_user(&token).await?;
    if let Some(expected) = &email {
        if !expected.eq_ignore_ascii_case(&learned_email) {
            return Err(Error::Other(format!(
                "You chose {expected} for backup but signed in as {learned_email}. \
                 Pick the same account."
            )));
        }
    }
    google::save_token(&crate::drive::account_token_key(&learned_email), &token)?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    crate::db::set_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY, &learned_email)?;
    crate::db::set_bool(&conn, BACKUP_GDRIVE_ENABLED_KEY, true)?;
    read_gdrive_status(&conn)
}

/// Stop backing up to Google Drive: disable it and forget the chosen account. The OAuth token is
/// deleted ONLY if the account isn't also a read connector (otherwise the connector still needs it
/// — the unioned scope can't be narrowed without a full re-consent, so we leave it in place).
#[tauri::command]
pub fn backup_gdrive_disconnect(state: State<'_, AppState>) -> Result<()> {
    use crate::backup::schedule::{BACKUP_GDRIVE_ACCOUNT_KEY, BACKUP_GDRIVE_ENABLED_KEY};
    let conn = state.conn()?;
    let account =
        crate::db::get_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY)?.filter(|s| !s.is_empty());
    crate::db::set_bool(&conn, BACKUP_GDRIVE_ENABLED_KEY, false)?;
    crate::db::set_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY, "")?;
    if let Some(email) = account {
        let is_read_connector = crate::drive::list_accounts(&conn)?
            .iter()
            .any(|a| a.email.eq_ignore_ascii_case(&email));
        if !is_read_connector {
            secrets::clear_google_token_for(&crate::drive::account_token_key(&email))?;
        }
    }
    Ok(())
}

/// The keychain token key for the Google account set up for backup, or a friendly error if none is.
/// Reads the DB and drops the lock before the caller awaits (rule #4).
fn gdrive_backup_token_key(app: &AppHandle) -> Result<String> {
    use crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let email = crate::db::get_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::Other(
                "No Google account is set up for backup. Grant access in Settings → Backup.".into(),
            )
        })?;
    Ok(crate::drive::account_token_key(&email))
}

/// List PM's encrypted archives already on Google Drive (newest first), for the restore picker.
#[tauri::command]
pub async fn list_gdrive_backups(
    app: AppHandle,
) -> Result<Vec<crate::backup::naming::BackupEntry>> {
    let token_key = gdrive_backup_token_key(&app)?;
    BackupDestination::GoogleDrive { token_key }.list().await
}

/// Create an encrypted archive and push it to Google Drive (the account set up for backup). Same
/// portable format + detached progress as the Proton path.
#[tauri::command]
pub async fn backup_to_gdrive(app: AppHandle, passphrase: String) -> Result<()> {
    if passphrase.is_empty() {
        return Err(Error::Other("a backup passphrase is required".into()));
    }
    // M-4: strength floor before the archive (which embeds the raw DB key) leaves the machine.
    vault::kdf::validate_passphrase_strength(&passphrase)?;
    let token_key = gdrive_backup_token_key(&app)?;
    run_backup(
        &app,
        passphrase,
        vec![BackupDestination::GoogleDrive { token_key }],
        None,
    )
    .await
    .map(|_| ())
}

/// Download an archive from Google Drive (by name) and restore it into a fresh, validated folder
/// (the live vault is untouched until the user switches, exactly like the Proton/local restores).
#[tauri::command]
pub async fn restore_from_gdrive(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    passphrase: String,
) -> Result<RestoreSummary> {
    // I-03/L-1: wipe the passphrase plaintext on return (it is consumed by the blocking restore below).
    let passphrase = require_backup_passphrase(passphrase)?;
    let dest = BackupDestination::GoogleDrive {
        token_key: gdrive_backup_token_key(&app)?,
    };
    let _busy = begin_backup_run(&app, &state, BackupPhase::Download)?;
    let target = restore_staging_target(&app)?;

    // Pull the archive into a scratch dir (async — the Drive download is native async) that
    // outlives the blocking restore below.
    //
    // The two failure tails below are deliberately NOT the ones the Proton command uses, and both
    // divergences are pre-existing: a scratch-dir failure here returns with no terminal `Failed`
    // event (leaving the panel on its last phase), and the download failure reports the raw error
    // even when the user pressed Cancel. Unifying them would change what this command emits, so it
    // is left for a fix that can carry a changelog line rather than folded into a refactor.
    let dl = restore_scratch_dir(&dest)?;
    if let Err(e) = dest.download(&app, &name, dl.path()).await {
        let msg = e.to_string();
        emit_backup_progress(
            &app,
            BackupEvent::Failed {
                message: msg.clone(),
            },
        );
        return Err(Error::Other(msg));
    }
    emit_backup_progress(
        &app,
        BackupEvent::Phase {
            phase: BackupPhase::Download,
            fraction: 1.0,
        },
    );

    let app2 = app.clone();
    let target2 = target.clone();
    let local = dl.path().join(&name);
    let result = spawn_blocking_join("restore", move || {
        let st = app2.state::<AppState>();
        let report = |phase, fraction| {
            emit_backup_progress(&app2, BackupEvent::Phase { phase, fraction });
        };
        backup::restore::restore(&local, &passphrase, &target2, report, &st.backup_cancel)
    })
    .await?;
    drop(dl);

    let outcome = unwrap_restore_result(&app, &state, result)?;
    Ok(finalize_restore(&app, &state, outcome))
}

/// Enable/disable each backup destination for scheduled runs. Enabling Google Drive requires a
/// granted account (mirrors the passphrase guard on the schedule) so the scheduler never sees
/// "gdrive enabled" with nothing to back up to.
#[tauri::command]
pub fn set_backup_destinations(
    state: State<'_, AppState>,
    proton_enabled: bool,
    gdrive_enabled: bool,
) -> Result<()> {
    use crate::backup::schedule::{
        BACKUP_GDRIVE_ACCOUNT_KEY, BACKUP_GDRIVE_ENABLED_KEY, BACKUP_PROTON_ENABLED_KEY,
    };
    let conn = state.conn()?;
    if gdrive_enabled {
        let granted = match crate::db::get_setting(&conn, BACKUP_GDRIVE_ACCOUNT_KEY)?
            .filter(|s| !s.is_empty())
        {
            Some(email) => google::token_has_scope(
                &crate::drive::account_token_key(&email),
                google::DRIVE_FILE_SCOPE,
            )?,
            None => false,
        };
        if !granted {
            return Err(Error::Other(
                "Grant Google Drive backup access before enabling it.".into(),
            ));
        }
    }
    crate::db::set_bool(&conn, BACKUP_PROTON_ENABLED_KEY, proton_enabled)?;
    crate::db::set_bool(&conn, BACKUP_GDRIVE_ENABLED_KEY, gdrive_enabled)?;
    Ok(())
}

#[cfg(test)]
mod restore_adoption_tests {
    use super::home_is_pristine;
    use crate::vault;

    #[test]
    fn home_is_pristine_frees_an_empty_slot_but_never_clobbers_a_passphrase_home() {
        // A free (no-vault) home slot is pristine — a restore may be adopted into it.
        let empty = tempfile::tempdir().unwrap();
        assert!(home_is_pristine(empty.path()).unwrap());

        // A passphrase ("shareable") home vault is a deliberate, real vault — refused before any DB
        // open (so this guard needs no keychain), so a restore never overwrites it.
        let pass = tempfile::tempdir().unwrap();
        let mut meta = vault::VaultMeta::new_device();
        meta.key_mode = vault::KeyMode::Passphrase;
        vault::store_meta(pass.path(), &meta).unwrap();
        assert!(!home_is_pristine(pass.path()).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::shared::temp_db;

    /// The predicate standing between a restore and an unbacked-up `delete_vault_artifacts`, and the
    /// two ways it can be wrong. Reading `documents` alone missed everything a user can build before
    /// importing a file; reading "any row" in the widened tables would be worse still — the migration
    /// seeds make it permanently false, so re-homing would silently never happen again.
    #[test]
    fn a_vault_is_only_pristine_when_the_user_has_put_nothing_in_it() {
        let (_dir, conn) = temp_db();
        assert!(
            !db_holds_user_data(&conn),
            "a freshly migrated store is empty despite the seeded embedder settings, the Unsorted \
             inbox and its self-alias"
        );

        // One case per table, each on its own store, so nothing rides on another's rows. Foreign
        // keys are off for the duration: `project_milestones` would otherwise need a `projects` row
        // that already answers the question on its own, and the point is to prove each table is in
        // the predicate independently.
        let cases: &[(&str, &str)] = &[
            (
                "documents",
                "INSERT INTO documents(vault_path, title, content_hash) VALUES ('a.md', 't', 'h')",
            ),
            ("projects", "INSERT INTO projects(name) VALUES ('Atlas')"),
            (
                "project_milestones",
                "INSERT INTO project_milestones(project_name, label, due_date) \
                 VALUES ('Atlas', 'pitch', '2026-08-01')",
            ),
            (
                "flags",
                "INSERT INTO flags(anchor_kind, anchor, type) VALUES ('milestone', '1', 'overdue')",
            ),
            (
                "preferences",
                "INSERT INTO preferences(scope, value) VALUES ('global', 'strong tea')",
            ),
            (
                "connector_sources",
                "INSERT INTO connector_sources(id, provider, service, label) \
                 VALUES ('gdrive:a@b.c', 'google', 'drive', 'a@b.c')",
            ),
            (
                "conversations",
                "INSERT INTO conversations(title) VALUES ('chat')",
            ),
            (
                "entities beyond the seeded inbox",
                "INSERT INTO entities(type, canonical_name) VALUES ('person', 'Ramit')",
            ),
            (
                "the pinboard, which lives only in settings",
                "INSERT OR REPLACE INTO settings(key, value) VALUES ('pinboard', '- a note')",
            ),
        ];
        for (what, sql) in cases {
            let (_d, c) = temp_db();
            c.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
            // A schema drift would make this INSERT fail and the case vacuously pass, so assert it.
            c.execute(sql, []).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert!(db_holds_user_data(&c), "{what} is the user's own work");
        }

        // And the carve-outs really are carve-outs: writing the seeded rows again changes nothing.
        let (_d, c) = temp_db();
        c.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES ('embedding_model', 'x')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT OR IGNORE INTO entities(type, canonical_name) VALUES ('project', 'Unsorted')",
            [],
        )
        .unwrap();
        assert!(
            !db_holds_user_data(&c),
            "boot-written settings and the seeded inbox are not user intent"
        );
    }
}
