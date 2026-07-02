// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The one vault migration routine (spec §6). Every mode change — make shareable,
//! make private, change passphrase, move location — is expressed as a [`MigrationPlan`]
//! and run by a single ordered, crash-aware routine. That gives one place to enforce
//! the "shared ⇒ encrypted at rest" invariant and to sequence the one genuinely
//! dangerous step (the non-transactional `PRAGMA rekey`) behind a recovery journal.
//!
//! The order is: checkpoint the WAL, take a full pre-migration backup, rekey in place,
//! convert the Markdown, then (optionally) relocate — flipping the metadata, pointer,
//! and keychain last. [`recover`] repairs an interruption on the next launch: it rolls
//! the in-place phase back to its backup, or lets the pointer decide whether a partial
//! move should be finished or discarded. The vault's *identity* is never half-written,
//! so a crash always leaves an openable vault.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::{
    load_meta, master_from_db_key_hex, meta_path, pointer, prepare_shareable, resolve, store_meta,
    KeyMode, MarkdownCipher, MarkdownEncryption, MarkdownPolicy, VaultMeta,
};
use crate::error::{Error, Result};
use crate::secret::Secret;
use crate::{db, ingest, paths, secrets, AppState, VaultRuntime};

/// The on-disk components of a vault within its root (mirrors `resolve_layout`): the
/// encrypted DB and the Markdown subfolder. The metadata file ([`super::META_FILENAME`])
/// is the third. Lock/journal files are per-profile or ephemeral and are not moved.
const DB_FILENAME: &str = "pm.sqlite";
const MARKDOWN_SUBDIR: &str = "vault";

/// A requested vault transition. The four spec transitions are all expressed as plans:
/// make shareable (Device to Passphrase, encrypt), make private (Passphrase to Device,
/// decrypt), change passphrase (new passphrase), and move (location only). One plan can
/// combine changes, e.g. make shareable and relocate at once.
#[derive(Debug, Clone)]
pub struct MigrationPlan {
    pub target_key_mode: KeyMode,
    /// Required (non-empty) when the target is Passphrase; ignored for Device.
    pub new_passphrase: Option<String>,
    pub target_markdown: MarkdownEncryption,
    /// New vault root, or `None` to stay in place.
    pub target_location: Option<PathBuf>,
}

impl MigrationPlan {
    /// Enforce the security invariant *before* any destructive step runs (spec §3): a
    /// shareable (passphrase) vault MUST encrypt its Markdown at rest — once it can be
    /// opened from another account, folder isolation no longer protects the notes.
    ///
    /// Passphrase *presence* is checked later, in the orchestration, which knows the
    /// current mode — so a move of an already-shareable vault (which keeps its existing
    /// key and needs no new passphrase) is allowed.
    pub fn validate(&self) -> Result<()> {
        if self.target_key_mode == KeyMode::Passphrase
            && self.target_markdown != MarkdownEncryption::XChaCha20Poly1305
        {
            return Err(Error::Other(
                "a shareable vault must encrypt its Markdown at rest".into(),
            ));
        }
        Ok(())
    }
}

// --- recovery journal -------------------------------------------------------------

/// Filename of the migration journal, written in the vault root while a migration is in
/// flight so an interrupted one can be detected (and repaired) on the next launch.
pub const JOURNAL_FILENAME: &str = "vault.migration.json";

/// The ordered stages of a migration. The journal records the stage currently in
/// progress; the metadata/pointer/keychain are flipped only after `Finalizing`, so a
/// crash before that leaves the *old* vault identity intact and openable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStage {
    /// The destructive in-place `PRAGMA rekey` — the one unrecoverable spot, which is
    /// why a full backup is taken first (`backup_dir`).
    Rekeying,
    /// Re-encrypting / decrypting the Markdown to match the new policy.
    Markdown,
    /// Copying the vault to a new location (copy-verify-delete).
    Relocating,
    /// Writing the new metadata + pointer + keychain entry; the point of no return.
    Finalizing,
}

/// On-disk record of an in-flight migration. Lives in the per-profile data dir (next to
/// the pointer), not inside the vault, so boot can always find it even when the
/// migration is relocating the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationJournal {
    pub stage: MigrationStage,
    pub started_at: String,
    /// The pre-migration vault root — where a full backup is restored to on rollback.
    pub from_root: PathBuf,
    /// A complete pre-migration backup (DB snapshot + Markdown + meta). Present while the
    /// in-place key/Markdown phase is at risk; `None` once that phase has committed (only
    /// the location move, which keeps the source intact, remains).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub backup_dir: Option<PathBuf>,
    /// The destination root, when the migration also relocates the vault.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_location: Option<PathBuf>,
}

fn journal_path(vault_root: &Path) -> PathBuf {
    vault_root.join(JOURNAL_FILENAME)
}

/// Read the migration journal if one is present (a migration was interrupted).
pub fn read_journal(vault_root: &Path) -> Result<Option<MigrationJournal>> {
    match std::fs::read(journal_path(vault_root)) {
        Ok(bytes) => {
            let journal = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Other(format!("{JOURNAL_FILENAME} is unreadable: {e}")))?;
            Ok(Some(journal))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write the migration journal atomically (temp file + rename), so the journal itself
/// can never be observed half-written.
pub fn write_journal(vault_root: &Path, journal: &MigrationJournal) -> Result<()> {
    std::fs::create_dir_all(vault_root)?;
    let path = journal_path(vault_root);
    let json = serde_json::to_vec_pretty(journal)
        .map_err(|e| Error::Other(format!("could not encode {JOURNAL_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove the journal once a migration finishes (or after a successful repair).
pub fn clear_journal(vault_root: &Path) -> Result<()> {
    match std::fs::remove_file(journal_path(vault_root)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

// --- copy-verify-delete primitives ------------------------------------------------

/// Copy one file, then check the destination ended up the same length as the source — a
/// cheap integrity check that the copy completed. Parent directories are created.
fn copy_file_verified(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    let src_len = std::fs::metadata(src)?.len();
    let dst_len = std::fs::metadata(dst)?.len();
    if src_len != dst_len {
        return Err(Error::Other(format!(
            "copy verification failed for {} ({src_len} vs {dst_len} bytes)",
            src.display()
        )));
    }
    Ok(())
}

/// Recursively copy `src` into `dst`, verifying each file's length (see
/// [`copy_file_verified`]). A missing source directory is treated as empty. Shared with the
/// backup restore path as the cross-volume fallback when a staging rename can't apply.
pub(crate) fn copy_tree_verified(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    if !src.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_tree_verified(&path, &target)?;
        } else {
            copy_file_verified(&path, &target)?;
        }
    }
    Ok(())
}

/// Copy a vault's artifacts (DB, Markdown tree, metadata) from one root to another,
/// verifying each copy. The source is left intact — the caller removes it only after the
/// move commits — so an interrupted copy never harms the live vault. Used instead of a
/// rename so a move can cross volumes.
pub(crate) fn copy_vault_artifacts(from_root: &Path, to_root: &Path) -> Result<()> {
    std::fs::create_dir_all(to_root)?;
    copy_file_verified(&from_root.join(DB_FILENAME), &to_root.join(DB_FILENAME))?;
    copy_tree_verified(
        &from_root.join(MARKDOWN_SUBDIR),
        &to_root.join(MARKDOWN_SUBDIR),
    )?;
    let meta = meta_path(from_root);
    if meta.exists() {
        copy_file_verified(&meta, &meta_path(to_root))?;
    }
    Ok(())
}

/// Remove a vault's artifacts from a root (after a move has committed, or to clear the
/// destination before a restore). Best-effort: a leftover file is a harmless orphan. The
/// WAL/SHM sidecars are normally gone after a clean close, but are swept too just in case.
pub(crate) fn delete_vault_artifacts(root: &Path) {
    let _ = std::fs::remove_file(root.join(DB_FILENAME));
    let _ = std::fs::remove_file(root.join(format!("{DB_FILENAME}-wal")));
    let _ = std::fs::remove_file(root.join(format!("{DB_FILENAME}-shm")));
    let _ = std::fs::remove_dir_all(root.join(MARKDOWN_SUBDIR));
    let _ = std::fs::remove_file(meta_path(root));
}

/// Restore a full backup over a vault root, discarding whatever an interrupted migration
/// left there (the live DB/Markdown may be half-rekeyed/converted). The destination
/// artifacts are cleared first so a rename-during-convert can't leave both forms behind.
fn restore_vault_from_backup(backup: &Path, from_root: &Path) -> Result<()> {
    delete_vault_artifacts(from_root);
    copy_vault_artifacts(backup, from_root)
}

// --- orchestration ----------------------------------------------------------------

/// Take a consistent SQLCipher snapshot of the open store into `dest` (which must not
/// exist), preserving encryption and folding in the WAL. Shared by migration, the local
/// export, and the encrypted backup so the single-quote escaping lives in exactly one place.
pub(crate) fn vacuum_into(conn: &rusqlite::Connection, dest: &Path) -> Result<()> {
    // VACUUM INTO takes a literal path, not a bound parameter; escape any single quote.
    let escaped = dest.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
    Ok(())
}

/// The current SQLCipher key (64-hex) for a vault. Delegates to the shared resolver
/// [`super::current_db_key`] so a restored/relocated DEVICE vault — whose real key lives
/// in the per-vault keychain cache, not this machine's global device key — is migrated
/// with the key its DB is actually encrypted under (mirrors `open_at_boot`). A passphrase
/// vault that hasn't been unlocked yields the "must be unlocked" error, as before.
fn current_key_hex(meta: &VaultMeta) -> Result<Secret> {
    super::current_db_key(meta)?
        .ok_or_else(|| Error::Other("the vault must be unlocked before it can be migrated".into()))
}

/// Resolve a plan to the new SQLCipher key (64-hex) and the new metadata, preserving the
/// vault id. Derives a fresh passphrase key (make-shareable / change-passphrase), reuses
/// the device key (make-private), or keeps the current key (a move with no key change).
fn plan_new_key_and_meta(
    old_meta: &VaultMeta,
    plan: &MigrationPlan,
) -> Result<(Secret, VaultMeta)> {
    match plan.target_key_mode {
        KeyMode::Passphrase => {
            match plan
                .new_passphrase
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                // New passphrase set: derive a new key + meta, keeping the vault id.
                // prepare_shareable also marks the Markdown encrypted (the invariant).
                Some(pass) => Ok(prepare_shareable(old_meta, pass).map(|(m, k)| (k, m))?),
                // No new passphrase: a move of an already-shareable vault — keep the key.
                None => {
                    if old_meta.key_mode != KeyMode::Passphrase {
                        return Err(Error::Other(
                            "a passphrase is required to make this vault shareable".into(),
                        ));
                    }
                    Ok((current_key_hex(old_meta)?, old_meta.clone()))
                }
            }
        }
        KeyMode::Device => {
            // Make-private (or a move of a device vault): the random device keychain key.
            let key = secrets::get_or_create_db_key()?;
            let mut meta = old_meta.clone();
            meta.key_mode = KeyMode::Device;
            meta.kdf = None;
            meta.verifier = None;
            meta.markdown = MarkdownPolicy {
                encryption: plan.target_markdown,
                subkey: meta.markdown.subkey,
            };
            Ok((key, meta))
        }
    }
}

/// Bring this profile's keychain into line with the new key: cache the derived key for a
/// passphrase vault, or drop any cached key for a device vault (which uses the keychain
/// device key directly). Done after the journal is cleared, so a crash here costs at most
/// one extra passphrase prompt, never a key/DB mismatch.
fn update_keychain(new_meta: &VaultMeta, new_key: &Secret) -> Result<()> {
    match new_meta.key_mode {
        KeyMode::Passphrase => secrets::set_cached_vault_key(&new_meta.vault_id, new_key.expose()),
        KeyMode::Device => {
            let _ = secrets::clear_cached_vault_key(&new_meta.vault_id);
            Ok(())
        }
    }
}

/// Run a vault migration: the single ordered routine behind every mode change (spec §6).
/// Validates the invariant, then performs the in-place key/Markdown change (backed up for
/// rollback) and finally the optional relocation (source kept until the pointer commits),
/// flipping the metadata/pointer/keychain last. An interruption is repaired on the next
/// launch by [`recover`].
pub fn migrate_vault(app: &AppHandle, plan: MigrationPlan) -> Result<()> {
    plan.validate()?;
    let state = app.state::<AppState>();
    let state = state.inner();
    let data_dir = paths::data_dir(app)?;
    let resolved = resolve(app)?;
    let old_meta = load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata to migrate".into()))?;

    let old_key = current_key_hex(&old_meta)?;
    let (new_key, new_meta) = plan_new_key_and_meta(&old_meta, &plan)?;

    let old_master = master_from_db_key_hex(old_key.expose())?;
    let old_cipher = MarkdownCipher::from_meta(&old_meta, &old_master);
    let new_master = master_from_db_key_hex(new_key.expose())?;
    let new_cipher = MarkdownCipher::from_meta(&new_meta, &new_master);

    let key_changes = old_key.expose() != new_key.expose();
    // The in-place phase runs when the key changes (which also moves the Markdown subkey)
    // or the Markdown policy changes — either way the files must be re-encoded.
    let needs_inplace = key_changes || old_meta.markdown.encryption != new_meta.markdown.encryption;

    // ---- Phase A: in-place key + Markdown change (the risky, fully-backed-up part) ----
    if needs_inplace {
        {
            let conn = state.conn()?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        let backup = data_dir.join("vault-migration-backup");
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::create_dir_all(&backup)?;
        {
            let conn = state.conn()?;
            vacuum_into(&conn, &backup.join(DB_FILENAME))?;
        }
        copy_tree_verified(&resolved.markdown_dir, &backup.join(MARKDOWN_SUBDIR))?;
        let meta_src = meta_path(&resolved.vault_root);
        if meta_src.exists() {
            copy_file_verified(&meta_src, &meta_path(&backup))?;
        }
        let mut journal = MigrationJournal {
            stage: MigrationStage::Rekeying,
            started_at: chrono::Utc::now().to_rfc3339(),
            from_root: resolved.vault_root.clone(),
            backup_dir: Some(backup.clone()),
            target_location: plan.target_location.clone(),
        };
        write_journal(&data_dir, &journal)?;

        if key_changes {
            let conn = state.conn()?;
            db::rekey(&conn, new_key.expose())?;
        }

        journal.stage = MigrationStage::Markdown;
        write_journal(&data_dir, &journal)?;
        {
            let mut conn = state.conn()?;
            let tx = conn.transaction()?;
            ingest::convert_markdown(&tx, &resolved.markdown_dir, &old_cipher, &new_cipher)?;
            tx.commit()?;
        }

        // Commit the in-place phase. Metadata first (so the on-disk identity matches the
        // new key), then clear the rollback point, then the keychain — see update_keychain.
        journal.stage = MigrationStage::Finalizing;
        write_journal(&data_dir, &journal)?;
        store_meta(&resolved.vault_root, &new_meta)?;
        clear_journal(&data_dir)?;
        let _ = std::fs::remove_dir_all(&backup);
        update_keychain(&new_meta, &new_key)?;

        // The connection is still open + valid at this root; just swap the session runtime (the
        // new master moves both the Markdown subkey and the rules-file subkey). set_vault_runtime
        // reconciles the rules file under the new key.
        state.set_vault_runtime(VaultRuntime::build(&resolved, &new_meta, &new_master))?;
    }

    // ---- Phase B: relocate (no key change; source kept until the pointer flips) ----
    if let Some(target) = plan.target_location.as_ref() {
        if target != &resolved.vault_root {
            relocate(
                state,
                &data_dir,
                &resolved.vault_root,
                target,
                &new_meta,
                &new_key,
            )?;
        }
    }

    // Defence in depth: when a shareable vault is moved into a (potentially shared)
    // folder, lock that folder down to its owner. A vault that stays in the per-profile
    // data dir is already OS-isolated and needs no ACL. Encryption is the real
    // protection, so a failure here is only a warning.
    if new_meta.key_mode == KeyMode::Passphrase {
        if let Some(final_root) = plan.target_location.as_ref() {
            if let Err(e) = super::acl::restrict_to_owner(final_root, &[]) {
                eprintln!(
                    "vault: could not apply folder ACL ({e}); encryption still protects the vault"
                );
            }
        }
    }
    Ok(())
}

/// The location-move half of a migration: copy the vault to `to_root`, flip the pointer
/// (the commit point), then drop the old copy and reopen at the new root. No key change
/// happens here, so the source is a safe fallback until the pointer commits.
fn relocate(
    state: &AppState,
    data_dir: &Path,
    from_root: &Path,
    to_root: &Path,
    new_meta: &VaultMeta,
    new_key: &Secret,
) -> Result<()> {
    write_journal(
        data_dir,
        &MigrationJournal {
            stage: MigrationStage::Relocating,
            started_at: chrono::Utc::now().to_rfc3339(),
            from_root: from_root.to_path_buf(),
            backup_dir: None,
            target_location: Some(to_root.to_path_buf()),
        },
    )?;
    // Fold the WAL into the main DB so the copied file is complete (a move with no key
    // change skips Phase A's checkpoint), then close the connection to unlock the file.
    {
        let conn = state.conn()?;
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
    drop(state.take_conn()?);
    copy_vault_artifacts(from_root, to_root)?;
    // Commit the move: point this profile at the new root, then clear the journal.
    pointer::store(data_dir, &pointer::VaultPointer::new(to_root.to_path_buf()))?;
    clear_journal(data_dir)?;
    delete_vault_artifacts(from_root);
    // Reopen at the new location with the (possibly new) key, building the session runtime (both
    // Markdown + rules ciphers) from the master; open_session reconciles the rules file there.
    let conn = db::open(&to_root.join(DB_FILENAME), new_key.expose())?;
    let to_resolved = super::ResolvedVault {
        vault_root: to_root.to_path_buf(),
        db_path: to_root.join(DB_FILENAME),
        markdown_dir: to_root.join(MARKDOWN_SUBDIR),
    };
    let new_master = master_from_db_key_hex(new_key.expose())?;
    state.open_session(
        conn,
        VaultRuntime::build(&to_resolved, new_meta, &new_master),
    )?;
    Ok(())
}

/// Repair an interrupted migration on launch (called before the store is opened). With a
/// journal present, either roll the in-place phase back to its full backup, or — if only
/// the move was in flight — let the pointer decide whether to finish cleanup or discard
/// the partial copy. Either way the vault ends in a consistent, openable state.
pub fn recover(app: &AppHandle) -> Result<()> {
    let data_dir = paths::data_dir(app)?;
    let Some(journal) = read_journal(&data_dir)? else {
        return Ok(());
    };
    match journal.stage {
        // The in-place key/Markdown phase didn't commit (or crashed mid-commit): restore
        // the full backup, returning the vault to exactly its pre-migration state.
        MigrationStage::Rekeying | MigrationStage::Markdown | MigrationStage::Finalizing => {
            if let Some(backup) = journal.backup_dir.as_ref() {
                if backup.exists() {
                    restore_vault_from_backup(backup, &journal.from_root)?;
                    let _ = std::fs::remove_dir_all(backup);
                }
            }
            if let Some(target) = journal.target_location.as_ref() {
                if target != &journal.from_root {
                    let _ = std::fs::remove_dir_all(target);
                }
            }
        }
        // The in-place phase already committed; only the move was in flight. The pointer
        // is the source of truth for whether it finished.
        MigrationStage::Relocating => {
            let now = resolve(app)?.vault_root;
            match journal.target_location.as_ref() {
                Some(target) if &now == target => delete_vault_artifacts(&journal.from_root),
                Some(target) => {
                    let _ = std::fs::remove_dir_all(target);
                }
                None => {}
            }
        }
    }
    clear_journal(&data_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(mode: KeyMode, pass: Option<&str>, md: MarkdownEncryption) -> MigrationPlan {
        MigrationPlan {
            target_key_mode: mode,
            new_passphrase: pass.map(String::from),
            target_markdown: md,
            target_location: None,
        }
    }

    #[test]
    fn shareable_target_must_encrypt_markdown() {
        // The security invariant: passphrase mode with plaintext Markdown is rejected.
        assert!(
            plan(KeyMode::Passphrase, Some("pw"), MarkdownEncryption::None)
                .validate()
                .is_err()
        );
        // Passphrase mode with encryption on is valid — even without a *new* passphrase
        // (that's a move of an already-shareable vault; presence is checked downstream).
        assert!(plan(
            KeyMode::Passphrase,
            None,
            MarkdownEncryption::XChaCha20Poly1305
        )
        .validate()
        .is_ok());
        assert!(plan(
            KeyMode::Passphrase,
            Some("pw"),
            MarkdownEncryption::XChaCha20Poly1305
        )
        .validate()
        .is_ok());
    }

    #[test]
    fn device_target_has_no_extra_requirements() {
        // Going private (device) doesn't require a passphrase, with or without markdown.
        assert!(plan(KeyMode::Device, None, MarkdownEncryption::None)
            .validate()
            .is_ok());
        assert!(
            plan(KeyMode::Device, None, MarkdownEncryption::XChaCha20Poly1305)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn journal_round_trips_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_journal(dir.path()).unwrap().is_none());
        let j = MigrationJournal {
            stage: MigrationStage::Rekeying,
            started_at: "2026-06-24T00:00:00Z".into(),
            from_root: dir.path().join("vault"),
            backup_dir: Some(dir.path().join("backup")),
            target_location: None,
        };
        write_journal(dir.path(), &j).unwrap();
        assert_eq!(read_journal(dir.path()).unwrap().as_ref(), Some(&j));
        clear_journal(dir.path()).unwrap();
        assert!(read_journal(dir.path()).unwrap().is_none());
        // Clearing again is a no-op, not an error.
        clear_journal(dir.path()).unwrap();
    }

    #[test]
    fn copy_vault_artifacts_copies_db_markdown_and_meta_keeping_source() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        std::fs::create_dir_all(from.join(MARKDOWN_SUBDIR)).unwrap();
        std::fs::write(from.join(DB_FILENAME), b"db-bytes").unwrap();
        std::fs::write(meta_path(&from), b"{}").unwrap();
        std::fs::write(
            from.join(MARKDOWN_SUBDIR).join("note.md.pmenc"),
            b"ciphertext",
        )
        .unwrap();

        copy_vault_artifacts(&from, &to).unwrap();

        // Source is intact (the move only deletes it after the pointer commits)...
        assert!(from.join(DB_FILENAME).exists());
        // ...and every artifact landed at the destination.
        assert_eq!(std::fs::read(to.join(DB_FILENAME)).unwrap(), b"db-bytes");
        assert_eq!(
            std::fs::read(to.join(MARKDOWN_SUBDIR).join("note.md.pmenc")).unwrap(),
            b"ciphertext"
        );
        assert!(meta_path(&to).exists());

        // delete_vault_artifacts removes exactly those, leaving other files alone.
        std::fs::write(from.join("unrelated.txt"), b"keep").unwrap();
        delete_vault_artifacts(&from);
        assert!(!from.join(DB_FILENAME).exists());
        assert!(!from.join(MARKDOWN_SUBDIR).exists());
        assert!(from.join("unrelated.txt").exists());
    }
}
