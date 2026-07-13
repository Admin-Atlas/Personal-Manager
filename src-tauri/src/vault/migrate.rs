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
    access, advert, load_meta, master_from_db_key_hex, meta_path, pointer, prepare_shareable,
    resolve, store_meta, KeyMode, MarkdownCipher, MarkdownEncryption, MarkdownPolicy, VaultMeta,
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
#[derive(Clone)]
pub struct MigrationPlan {
    pub target_key_mode: KeyMode,
    /// Required (non-empty) when the target is Passphrase; ignored for Device. Held in a `Zeroizing`
    /// so the passphrase plaintext is wiped from memory when the plan drops (I-03).
    pub new_passphrase: Option<zeroize::Zeroizing<String>>,
    pub target_markdown: MarkdownEncryption,
    /// New vault root, or `None` to stay in place.
    pub target_location: Option<PathBuf>,
}

// Manual Debug (I-03): redact the passphrase so it can never leak into a log line, while keeping the
// rest of the plan inspectable. The derived Debug this replaces would have printed the secret in full.
impl std::fmt::Debug for MigrationPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationPlan")
            .field("target_key_mode", &self.target_key_mode)
            .field(
                "new_passphrase",
                &self.new_passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("target_markdown", &self.target_markdown)
            .field("target_location", &self.target_location)
            .finish()
    }
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

/// Filename of the migration journal. It lives in the per-profile **data dir** (next to the
/// pointer), NOT inside the vault, so boot can always find it even while a migration is
/// relocating the vault — an interrupted migration is detected (and repaired) on the next launch.
pub const JOURNAL_FILENAME: &str = "vault.migration.json";

/// Folder under the data dir holding the full, DECRYPTABLE pre-migration backup (DB snapshot +
/// Markdown + meta) taken before the destructive in-place rekey — the rollback point. Removed once
/// that phase commits, on rollback, and by a data wipe (`wipe.rs`).
pub const MIGRATION_BACKUP_DIR: &str = "vault-migration-backup";

/// The pre-migration backup folder for a given profile data dir. One expression, reused by the
/// migration itself and by the wipe that must clear it (B1-4).
pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(MIGRATION_BACKUP_DIR)
}

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

fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join(JOURNAL_FILENAME)
}

/// Read the migration journal if one is present (a migration was interrupted).
pub fn read_journal(data_dir: &Path) -> Result<Option<MigrationJournal>> {
    match std::fs::read(journal_path(data_dir)) {
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
pub fn write_journal(data_dir: &Path, journal: &MigrationJournal) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = journal_path(data_dir);
    let json = serde_json::to_vec_pretty(journal)
        .map_err(|e| Error::Other(format!("could not encode {JOURNAL_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove the journal once a migration finishes (or after a successful repair).
pub fn clear_journal(data_dir: &Path) -> Result<()> {
    match std::fs::remove_file(journal_path(data_dir)) {
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
    // The linked-accounts sidecar travels with the vault so a link made before a move
    // survives it (the move's ACL lockdown re-applies these principals).
    let access_file = from_root.join(access::ACCESS_FILENAME);
    if access_file.exists() {
        copy_file_verified(&access_file, &to_root.join(access::ACCESS_FILENAME))?;
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
    let _ = std::fs::remove_file(root.join(access::ACCESS_FILENAME));
}

/// A tiny provenance marker PM drops into a relocation destination while a move is in flight, so
/// [`recover`] can positively tell a copy it created from a folder the user picked that already
/// held their files. Removed once the move commits.
const VAULT_MARKER: &str = ".pm-vault";

/// Discard a migration's partial destination copy WITHOUT ever removing a folder that isn't provably
/// ours. The [`VAULT_MARKER`] is checked FIRST: no marker ⇒ we touch nothing at all — critically,
/// because a crashed Phase-A migration's journal still names the target even though nothing was ever
/// written there, and a DIFFERENT account may have since put its own live vault in that well-known
/// folder (e.g. the default shared location). Stripping artifacts before the marker check would
/// destroy that other account's data. Only once the marker proves PM created this copy do we strip
/// our artifacts and remove the (now-empty, non-symlink) container. A stranded partial copy is inert
/// (the pointer commits elsewhere), so leaving one is always the safe failure.
fn discard_partial_target(target: &Path) {
    let marker = target.join(VAULT_MARKER);
    if !marker.exists() {
        return; // no marker ⇒ can't prove this is ours ⇒ leave it entirely (may be another vault)
    }
    // Never follow a symlink/reparse point out of the intended location before deleting.
    match std::fs::symlink_metadata(target) {
        Ok(m) if !m.file_type().is_symlink() => {}
        _ => return,
    }
    // Proven ours: strip only the vault artifacts we would have written, drop the marker, then
    // remove the container iff that left it empty (any other files the user kept there survive).
    delete_vault_artifacts(target);
    if std::fs::remove_file(&marker).is_err() {
        return;
    }
    if let Ok(mut entries) = std::fs::read_dir(target) {
        if entries.next().is_none() {
            let _ = std::fs::remove_dir(target);
        }
    }
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

/// What already sits at a relocation target, from the caller's `load_meta` look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// Nothing there (or no metadata) — safe to move in.
    Vacant,
    /// The target holds THIS vault (a wizard re-run or a crash-leftover partial copy) —
    /// safe to overwrite; Phase B and the `VAULT_MARKER` recovery already handle partials.
    SameVault,
    /// The target holds a DIFFERENT vault — moving in would silently overwrite it.
    ForeignVault,
    /// The target can't be checked (unreadable metadata / access denied) — refuse
    /// rather than risk clobbering something unseen.
    Unreadable,
}

/// Classify a relocation target from the result of `load_meta(target)`. Pure — the
/// guard that stops two accounts from silently overwriting each other's vault by
/// moving into the same folder.
pub fn relocation_target_state(
    looked: &Result<Option<VaultMeta>>,
    own_vault_id: &str,
) -> TargetState {
    match looked {
        Err(_) => TargetState::Unreadable,
        Ok(None) => TargetState::Vacant,
        Ok(Some(m)) if m.vault_id == own_vault_id => TargetState::SameVault,
        Ok(Some(_)) => TargetState::ForeignVault,
    }
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
                .map(|s| s.trim())
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
/// launch by [`recover`]. Returns non-fatal warnings (folder ACL / discovery-marker
/// failures) for the UI to show — encryption still protects the vault when they fire.
pub fn migrate_vault(app: &AppHandle, plan: MigrationPlan) -> Result<Vec<String>> {
    plan.validate()?;
    let state = app.state::<AppState>();
    let state = state.inner();
    let data_dir = paths::data_dir(app)?;
    let resolved = resolve(app)?;
    let old_meta = load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata to migrate".into()))?;

    // Collision guard, before any destructive step: never move onto a DIFFERENT vault
    // (the second mover would silently overwrite the first's DB + metadata).
    if let Some(target) = plan.target_location.as_ref() {
        if target != &resolved.vault_root {
            match relocation_target_state(&load_meta(target), &old_meta.vault_id) {
                TargetState::ForeignVault => {
                    return Err(Error::Other(
                        "that folder already holds a different PM vault — join it from this \
                         account instead of moving this vault onto it"
                            .into(),
                    ));
                }
                TargetState::Unreadable => {
                    return Err(Error::Other(
                        "that folder holds vault files PM can't read, so it can't be safely \
                         moved into — pick another folder"
                            .into(),
                    ));
                }
                TargetState::Vacant | TargetState::SameVault => {}
            }
        }
    }

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
        let backup = backup_dir(&data_dir);
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
        // Back up the linked-accounts sidecar too, so a rollback (which restores from this backup
        // and first clears the destination via delete_vault_artifacts — now including the sidecar)
        // doesn't lose the linked principals.
        let access_src = resolved.vault_root.join(access::ACCESS_FILENAME);
        if access_src.exists() {
            copy_file_verified(&access_src, &backup.join(access::ACCESS_FILENAME))?;
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

        // Commit the in-place phase. Write the new on-disk identity, then swap the live session onto
        // the new key BEFORE the two best-effort cleanups below — so a failure in either can't leave
        // the running session on the OLD cipher, which would keep writing Markdown under the previous
        // key until the next restart. (The connection is still open + valid at this root; the new
        // master moves both the Markdown and rules-file subkeys, and set_vault_runtime reconciles the
        // rules file under the new key.)
        journal.stage = MigrationStage::Finalizing;
        write_journal(&data_dir, &journal)?;
        store_meta(&resolved.vault_root, &new_meta)?;
        state.set_vault_runtime(VaultRuntime::build(&resolved, &new_meta, &new_master))?;
        // Remove the decryptable pre-migration backup BEFORE clearing the journal: a crash between
        // the two must not strand a full recoverable copy that recover() (which keys off the journal)
        // would then never see. Until the journal is cleared, recover() still finishes the cleanup.
        let _ = std::fs::remove_dir_all(&backup);
        clear_journal(&data_dir)?;
        // The keychain cache is a convenience only; a failure costs at most one extra passphrase
        // prompt (see update_keychain), so it must never abort a migration that has already committed.
        if let Err(e) = update_keychain(&new_meta, &new_key) {
            eprintln!(
                "vault: could not cache the new vault key ({e}); you may be prompted to unlock once"
            );
        }
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

    let mut warnings = Vec::new();
    let final_root = plan
        .target_location
        .clone()
        .unwrap_or_else(|| resolved.vault_root.clone());

    // Defence in depth: when a shareable vault is moved into a (potentially shared)
    // folder, lock that folder down to its owner PLUS every linked account — the
    // sidecar travelled with the copy, so a link made before the move survives it. A
    // vault that stays in the per-profile data dir is already OS-isolated and needs no
    // ACL. Encryption is the real protection, so a failure here is only a warning.
    if new_meta.key_mode == KeyMode::Passphrase {
        if let Some(target) = plan.target_location.as_ref() {
            let linked = access::principals(target, &new_meta.vault_id);
            if let Err(e) = super::acl::restrict_to_owner(target, &linked) {
                eprintln!(
                    "vault: could not apply folder ACL ({e}); encryption still protects the vault"
                );
                warnings.push(format!(
                    "PM couldn't set the folder permissions ({e}). Your data is still \
                     encrypted; linked accounts may need the folder shared by hand."
                ));
            }
        }
        // Advertise a shareable vault that lives OUTSIDE this profile's private folder,
        // so another account's fresh install can discover and offer it. Non-secret and
        // best-effort; the passphrase stays the real gate.
        if !final_root.starts_with(&data_dir) {
            if let Some(ads) = advert::ads_dir() {
                let ad = advert::SharedVaultAd::for_vault(&new_meta.vault_id, &final_root);
                if let Err(e) = advert::publish(&ads, &ad) {
                    warnings.push(format!(
                        "PM couldn't announce this vault to other accounts ({e}). They can \
                         still join it by picking the folder by hand."
                    ));
                }
            }
        }
    } else if old_meta.key_mode == KeyMode::Passphrase {
        // A genuine make-private (was Passphrase, now Device) — NOT a plain move of an
        // already-device vault, which must not trigger any of this owner-lockdown / advert
        // cleanup (it had no share to undo, and the "your notes are now unencrypted" warning
        // below would be nonsense for a vault whose Markdown was always plaintext).
        // Withdraw the discovery marker and the linked-accounts sidecar — there is nothing
        // left for another account to join.
        if let Some(ads) = advert::ads_dir() {
            let _ = advert::retract(&ads, &new_meta.vault_id);
        }
        // A vault made private while it still sits in a SHARED folder is the dangerous case: the
        // Markdown was just decrypted back to plaintext in place, and previously-linked accounts
        // still hold their folder ACEs — so their access must be revoked or they could now read the
        // owner's notes in cleartext. Two steps are needed, because `restrict_to_owner` alone can't
        // do it: on Windows its `/grant:r me admins` only REPLACES the principals it names, so a
        // linked account's explicit (inheritable) ACE survives. So (1) strip inheritance + grant
        // owner-only, then (2) explicitly `revoke_access` each previously-linked principal by name —
        // read from the sidecar BEFORE it's deleted below. A vault already in the per-profile dir is
        // OS-isolated and needs nothing. Best-effort — warn, never fail the migration that already
        // committed (the DB re-key is the real protection either way).
        if !final_root.starts_with(&data_dir) {
            let linked = access::principals(&final_root, &new_meta.vault_id);
            let mut acl_failed = super::acl::restrict_to_owner(&final_root, &[]).err();
            for principal in &linked {
                if let Err(e) = super::acl::revoke_access(&final_root, principal) {
                    acl_failed.get_or_insert(e);
                }
            }
            if let Some(e) = acl_failed {
                eprintln!("vault: could not re-lock the now-private folder ({e})");
                warnings.push(format!(
                    "PM couldn't lock the folder back down to just you ({e}). Your notes are now \
                     unencrypted in a shared folder — move the vault somewhere private, or fix the \
                     folder's permissions by hand."
                ));
            }
        }
        let _ = std::fs::remove_file(final_root.join(access::ACCESS_FILENAME));
    }
    Ok(warnings)
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
    // Re-check the target right before writing into it: the collision guard in `migrate_vault`
    // ran before Phase A (seconds-to-minutes of rekey/convert ago), so a concurrent wizard on
    // another account could have dropped its own vault here since. Refuse rather than overwrite —
    // narrows the check-to-copy TOCTOU to this final instant. (A truly simultaneous same-folder
    // race is still possible but astronomically unlikely: `suggest_shared_vault_location`
    // auto-suffixes past an occupied folder, so the default flow never picks the same one.)
    match relocation_target_state(&load_meta(to_root), &new_meta.vault_id) {
        TargetState::ForeignVault | TargetState::Unreadable => {
            return Err(Error::Other(
                "another PM vault appeared in that folder while this move was preparing — nothing \
                 was overwritten; pick a different folder and try again"
                    .into(),
            ));
        }
        TargetState::Vacant | TargetState::SameVault => {}
    }
    copy_vault_artifacts(from_root, to_root)?;
    // Drop a provenance marker so that, if the move is interrupted before it commits, recover() can
    // positively identify this destination copy as PM's own rather than inferring it from contents.
    let _ = std::fs::write(to_root.join(VAULT_MARKER), b"pm-vault\n");
    // Commit the move: point this profile at the new root, then clear the journal.
    pointer::store(data_dir, &pointer::VaultPointer::new(to_root.to_path_buf()))?;
    clear_journal(data_dir)?;
    delete_vault_artifacts(from_root);
    // The move has committed — this is now the live vault, no longer a discardable partial copy.
    let _ = std::fs::remove_file(to_root.join(VAULT_MARKER));
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
                    // Phase B never ran in this arm, so we created nothing at `target`; discard only
                    // acts if our marker is present (it won't be), leaving a user-picked folder alone.
                    discard_partial_target(target);
                }
            }
        }
        // The in-place phase already committed; only the move was in flight. The pointer
        // is the source of truth for whether it finished.
        MigrationStage::Relocating => {
            let now = resolve(app)?.vault_root;
            match journal.target_location.as_ref() {
                Some(target) if &now == target => {
                    delete_vault_artifacts(&journal.from_root);
                    // Committed to `target`; drop any leftover in-flight marker from the live vault.
                    let _ = std::fs::remove_file(target.join(VAULT_MARKER));
                }
                Some(target) => {
                    // The move didn't commit to `target`; discard the partial copy we made there
                    // (marker-gated, so a user's folder is never bulk-removed).
                    discard_partial_target(target);
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
            new_passphrase: pass.map(|p| zeroize::Zeroizing::new(p.to_string())),
            target_markdown: md,
            target_location: None,
        }
    }

    #[test]
    fn debug_redacts_the_passphrase() {
        // I-03: a plan must never print its passphrase, even if a `{:?}` ends up in a log line — the
        // derived Debug this replaces would have leaked it verbatim.
        let p = plan(
            KeyMode::Passphrase,
            Some("hunter2-super-secret"),
            MarkdownEncryption::XChaCha20Poly1305,
        );
        let rendered = format!("{p:?}");
        assert!(
            !rendered.contains("hunter2"),
            "the passphrase must be redacted in Debug output: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
        // A plan with no passphrase renders it as None, not "<redacted>".
        let none = format!(
            "{:?}",
            plan(KeyMode::Device, None, MarkdownEncryption::None)
        );
        assert!(none.contains("new_passphrase: None"));
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
    fn clearing_the_journal_at_the_wrong_dir_strands_it() {
        // B1-4 regression: the journal lives at the DATA DIR, not the vault root. For a relocated
        // vault the two differ, so clearing it at the vault root (the old `wipe` bug) is a silent
        // no-op that leaves a stale journal to drive `recover()` on the next launch. This pins the
        // path-sensitivity the wipe fix relies on: only clearing at the data dir removes it.
        let data_dir = tempfile::tempdir().unwrap();
        let vault_root = tempfile::tempdir().unwrap(); // a relocated vault's root ≠ the data dir
        let j = MigrationJournal {
            stage: MigrationStage::Rekeying,
            started_at: "2026-06-24T00:00:00Z".into(),
            from_root: vault_root.path().to_path_buf(),
            backup_dir: Some(backup_dir(data_dir.path())),
            target_location: None,
        };
        write_journal(data_dir.path(), &j).unwrap();

        clear_journal(vault_root.path()).unwrap(); // the old bug: clears the wrong directory
        assert!(
            read_journal(data_dir.path()).unwrap().is_some(),
            "clearing at the vault root leaves the data-dir journal behind"
        );

        clear_journal(data_dir.path()).unwrap(); // the fix: clear at the data dir
        assert!(
            read_journal(data_dir.path()).unwrap().is_none(),
            "clearing at the data dir removes the journal"
        );
    }

    #[test]
    fn relocation_target_state_classifies_all_four_ways() {
        // The collision-guard matrix: vacant and same-vault targets are safe; a foreign
        // vault or an unreadable target must refuse the move.
        let own = "vault-own";
        let mut foreign = VaultMeta::new_device();
        foreign.vault_id = "vault-foreign".into();
        let mut same = VaultMeta::new_device();
        same.vault_id = own.into();

        assert_eq!(relocation_target_state(&Ok(None), own), TargetState::Vacant);
        assert_eq!(
            relocation_target_state(&Ok(Some(same)), own),
            TargetState::SameVault
        );
        assert_eq!(
            relocation_target_state(&Ok(Some(foreign)), own),
            TargetState::ForeignVault
        );
        assert_eq!(
            relocation_target_state(&Err(Error::Other("access denied".into())), own),
            TargetState::Unreadable
        );
    }

    #[test]
    fn vault_artifacts_carry_the_access_sidecar() {
        // The linked-accounts sidecar must travel with a move (so the lockdown can
        // re-apply the principals) and be removed with the rest of the artifacts.
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join(DB_FILENAME), b"db").unwrap();
        std::fs::write(from.join(access::ACCESS_FILENAME), b"{}").unwrap();

        copy_vault_artifacts(&from, &to).unwrap();
        assert!(to.join(access::ACCESS_FILENAME).exists());

        delete_vault_artifacts(&to);
        assert!(!to.join(access::ACCESS_FILENAME).exists());
    }

    #[test]
    fn backup_dir_hangs_off_the_data_dir() {
        // The backup folder is a single reused expression under the data dir (migration + wipe).
        let data_dir = tempfile::tempdir().unwrap();
        assert_eq!(
            backup_dir(data_dir.path()),
            data_dir.path().join(MIGRATION_BACKUP_DIR)
        );
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

    #[test]
    fn vacuum_into_snapshots_every_committed_row_folding_the_wal() {
        // `vacuum_into` is the one snapshot helper the export + both backup paths route through; it
        // must produce a standalone copy holding every committed row even when the source keeps fresh
        // pages in a `-wal` sidecar. Open a WAL-mode DB, write rows, snapshot, and reopen the snapshot
        // (a plain single file) to prove the WAL was folded in.
        let dir = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("live.sqlite")).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO t (v) VALUES ('a'), ('b'), ('c');",
        )
        .unwrap();

        // VACUUM INTO refuses a pre-existing target, so aim at a fresh (empty) directory.
        let snap_dir = dir.path().join("snap");
        std::fs::create_dir_all(&snap_dir).unwrap();
        let dest = snap_dir.join(DB_FILENAME);
        vacuum_into(&conn, &dest).unwrap();

        let snap = rusqlite::Connection::open(&dest).unwrap();
        let n: i64 = snap
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 3,
            "the snapshot must contain every committed row, WAL included"
        );
    }

    #[test]
    fn vacuum_into_escapes_single_quotes_in_the_dest_path() {
        // `VACUUM INTO` takes a literal SQL string, so a single quote in the destination path would
        // break out of the quoting without escaping. A dir named `o'brien` exercises that — the
        // snapshot must still be written, not fail on malformed SQL.
        let dir = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("live.sqlite")).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();

        let quoted = dir.path().join("o'brien");
        std::fs::create_dir_all(&quoted).unwrap();
        let dest = quoted.join(DB_FILENAME);
        vacuum_into(&conn, &dest).unwrap();
        assert!(
            dest.exists(),
            "a dest path with a single quote must still produce a snapshot"
        );
    }
}
