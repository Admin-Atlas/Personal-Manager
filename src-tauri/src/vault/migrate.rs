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

// The on-disk components of a vault within its root (the ones a move must carry): the encrypted DB
// and the Markdown subfolder, plus the metadata file. All three names come from `vault/mod.rs`,
// which owns the layout rule — this module used to declare its own private copies of the first two,
// so a rename here and a rename in `resolve_layout` could silently disagree. Lock/journal files are
// per-profile or ephemeral and are not moved.
use super::{
    access, advert, load_meta, master_from_db_key_hex, meta_path, pointer, prepare_shareable,
    resolve, store_meta, KeyMode, MarkdownCipher, MarkdownEncryption, MarkdownPolicy, OwnerOnRekey,
    VaultMeta, DB_FILENAME, MARKDOWN_DIRNAME,
};
use crate::error::{Error, Result};
use crate::secret::Secret;
use crate::{db, ingest, paths, secrets, AppState, VaultRuntime};

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
    /// What this transition does to the vault's recorded owner. Every caller states it, and only a
    /// confirmed takeover says anything but [`OwnerOnRekey::Keep`] — see [`prepare_shareable`].
    /// Read ONLY on the new-passphrase path; a move / make-private / no-re-key plan reaches
    /// [`private_meta`] or clones the old meta, both of which decide ownership for themselves.
    pub owner: OwnerOnRekey,
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
            .field("owner", &self.owner)
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

/// Every non-DB, non-Markdown file that belongs to a vault root, as one list so the copy and
/// the delete below can never drift apart again — the drift this list exists to prevent left
/// `entities.pmrules` and `index-only.pmindex` behind at the old root on every move, while the
/// backup packer and the wipe both correctly treated them as vault members. Both are encrypted
/// sidecars whose AAD binds the vault id and stem, not the path, so they survive a relocation.
///
/// `pub(crate)` because the user-facing teardown ([`crate::wipe`]) is a fourth caller and was
/// keeping its own copy of the list — which is exactly the drift this list exists to prevent.
pub(crate) fn vault_sidecar_files(root: &Path) -> Vec<PathBuf> {
    vec![
        meta_path(root),
        // The linked-accounts sidecar travels with the vault so a link made before a move
        // survives it (the move's ACL lockdown re-applies these principals).
        root.join(access::ACCESS_FILENAME),
        // The portable entity rules and the index-only manifest are vault members (they are
        // packed into a .pmbackup and removed by the wipe). Leaving them at the old root
        // stranded ciphertext an ex-joiner could still read — and they are the reason a
        // "deleted" shared folder could never actually be removed.
        root.join(crate::entities::RULES_FILENAME),
        root.join(crate::index_only::MANIFEST_FILENAME),
    ]
}

/// Copy a vault's artifacts (DB, Markdown tree, metadata, sidecars) from one root to another,
/// verifying each copy. The source is left intact — the caller removes it only after the
/// move commits — so an interrupted copy never harms the live vault. Used instead of a
/// rename so a move can cross volumes.
pub(crate) fn copy_vault_artifacts(from_root: &Path, to_root: &Path) -> Result<()> {
    std::fs::create_dir_all(to_root)?;
    copy_file_verified(&from_root.join(DB_FILENAME), &to_root.join(DB_FILENAME))?;
    copy_tree_verified(
        &from_root.join(MARKDOWN_DIRNAME),
        &to_root.join(MARKDOWN_DIRNAME),
    )?;
    for from in vault_sidecar_files(from_root) {
        if from.exists() {
            // Every sidecar is optional: a vault that has never minted an entity has no rules
            // file, and reconcile-on-open would rebuild both from the DB mirror anyway. Copying
            // them keeps the OLD root clean, which is the half that matters.
            let name = from
                .file_name()
                .ok_or_else(|| Error::Other("vault sidecar has no filename".into()))?;
            copy_file_verified(&from, &to_root.join(name))?;
        }
    }
    Ok(())
}

/// Remove a vault's artifacts from a root (after a move has committed, or to clear the
/// destination before a restore). Best-effort: a leftover file is a harmless orphan. The
/// WAL/SHM sidecars are normally gone after a clean close, but are swept too just in case.
///
/// Deliberately does NOT remove `vault.lock`: this also runs from boot recovery paths
/// (restore, discard-partial-target) where another instance may legitimately hold the lock on
/// a shared root, and deleting a foreign lock invites split-brain. The baton request/ack files
/// are ephemeral signalling and do go (mirroring the wipe).
pub(crate) fn delete_vault_artifacts(root: &Path) {
    let _ = std::fs::remove_file(root.join(DB_FILENAME));
    let _ = std::fs::remove_file(root.join(format!("{DB_FILENAME}-wal")));
    let _ = std::fs::remove_file(root.join(format!("{DB_FILENAME}-shm")));
    let _ = std::fs::remove_dir_all(root.join(MARKDOWN_DIRNAME));
    for file in vault_sidecar_files(root) {
        let _ = std::fs::remove_file(file);
    }
    let _ = super::lock::clear_baton_files(root);
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
    // Proven ours: reset any lockdown we applied (the verify-then-commit move locks the
    // destination down BEFORE copying, so a partial copy discarded at boot may carry a
    // restrictive DACL — the folder's owner can always reset it), then strip only the vault
    // artifacts we would have written, drop the marker, and remove the container iff that
    // left it empty (any other files the user kept there survive).
    let _ = super::acl::reset_inheritance(target);
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

/// The passphrase a plan hands to the KDF: **exactly what the user typed**, or `None` when the
/// plan carries no passphrase at all (a move of an already-shareable vault, which keeps its key).
///
/// The one rule this encodes is `kdf.rs` policy Rule 1 — no normalization, ever. Until 3.19.1 this
/// site trimmed *before* deriving while every read path hashed raw, keying a vault to a string its
/// owner could never retype. The trim survives only in the predicate, where it answers "is there a
/// passphrase at all" and never touches the bytes: a whitespace-only string still means "no re-key",
/// as it always did. Kept pure and separate so the rule is locked by a test that needs no key
/// derivation — a test one layer below the decision is precisely what let the original fix ship green.
pub(crate) fn new_passphrase_for(plan: &MigrationPlan) -> Option<&str> {
    plan.new_passphrase
        .as_deref()
        // Zeroizing<String> -> &String -> &str. A type conversion ONLY: the bug this function
        // exists to prevent looked exactly like this line but read `.map(|s| s.trim())`.
        .map(String::as_str)
        .filter(|p| !p.trim().is_empty())
}

/// The Device-mode metadata for a make-private (or a device-vault move): the vault keeps its id but
/// sheds every passphrase-vault attribute. Pure (no keychain), so the field resets are unit-testable.
///
/// Clearing `owner_sid` is the important one: a device vault has no sharing owner (matches
/// [`VaultMeta::new_device`]), so a stale SID a formerly-shareable vault carried is dropped here. Left
/// in place it lingers meaninglessly and — after a cross-machine restore — could gate connector setup
/// against a foreign account's SID (`require_vault_owner`). There is no share left to own once Device.
/// The [`OwnershipTransfer`](super::OwnershipTransfer) record goes with it, for the same reason and in
/// the same breath: it names two SIDs of a sharing arrangement that no longer exists.
fn private_meta(old_meta: &VaultMeta, target_markdown: MarkdownEncryption) -> VaultMeta {
    let mut meta = old_meta.clone();
    meta.key_mode = KeyMode::Device;
    meta.kdf = None;
    meta.verifier = None;
    meta.owner_sid = None;
    meta.ownership_transfer = None;
    // Drop the source vault's MAC so the next authenticated open stamps a fresh one (uniform with
    // `build_passphrase_meta`, which also leaves it `None`). Cloning the old tag would leave a MAC
    // that no longer covers the mutated fields — read on the next open as "altered outside PM".
    meta.meta_mac = None;
    meta.markdown = MarkdownPolicy {
        encryption: target_markdown,
        subkey: meta.markdown.subkey,
    };
    meta
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
            match new_passphrase_for(plan) {
                // New passphrase set: derive a new key + meta, keeping the vault id and (unless the
                // plan says a takeover was confirmed) the vault's recorded owner.
                // prepare_shareable also marks the Markdown encrypted (the invariant).
                Some(pass) => {
                    Ok(prepare_shareable(old_meta, pass, plan.owner).map(|(m, k)| (k, m))?)
                }
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
            // A vault CONVERTING from passphrase (make-private) gets a fresh random device key. A vault
            // ALREADY device (a device-vault move/adopt — e.g. re-homing a restored device backup) keeps
            // the key its DB is actually encrypted under, so a relocation never needlessly re-keys the
            // whole store. On the same machine `get_or_create_db_key` returns that same key anyway; the
            // difference bites a RESTORED device vault, whose key is the embedded source key, not this
            // machine's device key.
            let key = if old_meta.key_mode == KeyMode::Device {
                current_key_hex(old_meta)?
            } else {
                secrets::get_or_create_db_key()?
            };
            Ok((key, private_meta(old_meta, plan.target_markdown)))
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
    // Never while a rebuild is walking the vault (#371). A migration re-keys every Markdown file and can
    // relocate the whole root out from under a pass that is mid-walk — reading files by a path that stops
    // existing, and re-encrypting the very files it is reading. This is the one guarded seam every mode
    // change funnels through, so it covers make-shareable, change-passphrase and relocate alike.
    if state.rebuild_running() {
        return Err(Error::Other(
            "PM is rebuilding the search index right now, and it's reading every file in your vault. \
             Wait for it to finish (the Documents tab shows its progress), then change your vault again."
                .into(),
        ));
    }
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

    // Pre-flight the destination BEFORE any mutation (verify-then-commit): reject a location
    // a shared vault can't live on (network / non-ACL filesystem, spec §498) and dress-
    // rehearse the exact owner lockdown in a throwaway subfolder — so a machine whose policy
    // would strand the owner fails HERE, with nothing touched, instead of after the move has
    // committed. Gated to the case that actually locks the folder down: a passphrase result
    // moving OUT of the profile dir, on a platform that enforces ACLs.
    if let Some(target) = plan.target_location.as_ref() {
        if target != &resolved.vault_root
            && plan.target_key_mode == KeyMode::Passphrase
            && !target.starts_with(&data_dir)
            && super::acl::lockdown_supported()
        {
            super::preflight::preflight_share_target(target)?;
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
        copy_tree_verified(&resolved.markdown_dir, &backup.join(MARKDOWN_DIRNAME))?;
        let meta_src = meta_path(&resolved.vault_root);
        if meta_src.exists() {
            copy_file_verified(&meta_src, &meta_path(&backup))?;
        }
        // Back up the encrypted sidecars too, so a rollback (which restores from this backup and
        // first clears the destination via delete_vault_artifacts — which includes them) doesn't
        // lose the linked principals, the entity rules, or the index-only classifications. The last
        // two are re-encrypted below, so without them here a rollback would restore a vault on the
        // OLD key beside sidecars written under the NEW one.
        for name in [
            access::ACCESS_FILENAME,
            crate::entities::RULES_FILENAME,
            crate::index_only::MANIFEST_FILENAME,
        ] {
            let src = resolved.vault_root.join(name);
            if src.exists() {
                copy_file_verified(&src, &backup.join(name))?;
            }
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
            // The WHOLE vault, not just the Markdown: the opt-in saved photo originals are encrypted
            // with the same subkey, so they move with it or they stay stranded under the old key
            // forever — the copy the user kept precisely so they could delete the original. Both
            // halves are idempotent, so an interrupted migration re-runs cleanly; a failure here
            // leaves the journal at `Markdown` with the backup intact (`copy_tree_verified` is
            // recursive, so it already holds `photos/`), which is what `recover` restores from.
            ingest::convert_vault_files(&tx, &resolved.markdown_dir, &old_cipher, &new_cipher)?;
            tx.commit()?;
        }

        // The two portable sidecars move with the key as well. They are encrypted under a subkey of
        // the vault master, so a key change leaves them unreadable, and the boot-time heal would then
        // rewrite the manifest from the DB mirror alone — silently dropping any classification the
        // file holds that the mirror doesn't (#517). Converting them here means that heal never has
        // to run. Both helpers are idempotent, so an interrupted migration re-runs cleanly.
        crate::entities::reencrypt_rules_file(
            &resolved.vault_root,
            &crate::entities::RulesCipher::from_master(&old_meta.vault_id, &old_master),
            &crate::entities::RulesCipher::from_master(&new_meta.vault_id, &new_master),
        )?;
        crate::index_only::reencrypt_manifest(
            &resolved.vault_root,
            &crate::index_only::ManifestCipher::from_master(&old_meta.vault_id, &old_master),
            &crate::index_only::ManifestCipher::from_master(&new_meta.vault_id, &new_master),
        )?;

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

    let mut warnings = Vec::new();
    let final_root = plan
        .target_location
        .clone()
        .unwrap_or_else(|| resolved.vault_root.clone());

    // ---- Phase B: relocate (source kept until the pointer flips; lockdown BEFORE commit) ----
    // The owner lockdown used to run here, AFTER the move committed, as a swallowed warning —
    // the exact bug: a lockdown that stripped the owner's own access left the vault bricked and
    // no rollback possible. It now happens fatally inside `relocate`, on the near-empty
    // destination before the pointer flips, with an effective-access probe; a failure aborts
    // the move with the source vault intact.
    if let Some(target) = plan.target_location.as_ref() {
        if target != &resolved.vault_root {
            let lockdown = new_meta.key_mode == KeyMode::Passphrase
                && !target.starts_with(&data_dir)
                && super::acl::lockdown_supported();
            relocate(
                state,
                &data_dir,
                &resolved.vault_root,
                target,
                &new_meta,
                &new_key,
                lockdown,
            )?;
        }
    }

    if new_meta.key_mode == KeyMode::Passphrase {
        // Advertise a shareable vault that lives OUTSIDE this profile's private folder,
        // so another account's fresh install can discover and offer it. Non-secret and
        // best-effort; the passphrase stays the real gate. (The owner lockdown is now a
        // fatal pre-commit step inside `relocate`, not a post-commit warning here.)
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

/// The location-move half of a migration, verify-then-commit. Ordered so the destination
/// is proven usable by THIS process before anything commits: journal, close the source
/// connection, then (in [`perform_relocation`]) re-check the target, lock it down on the
/// near-empty dir, probe access, copy, probe again, open the real DB — and only then flip
/// the pointer (the commit) and drop the source. A pre-commit failure aborts with the
/// source vault intact and openable, and this reopens it so the session survives.
#[allow(clippy::too_many_arguments)]
fn relocate(
    state: &AppState,
    data_dir: &Path,
    from_root: &Path,
    to_root: &Path,
    new_meta: &VaultMeta,
    new_key: &Secret,
    lockdown: bool,
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

    // The lockdown closure applies the owner + linked-account ACL to the destination.
    // Linked principals are read from the SOURCE sidecar (it hasn't been copied yet); the
    // inheritable ACEs then propagate to every file the copy lands there. `lockdown == false`
    // (a device move, an in-profile target, or a platform with no ACL primitive) makes it a
    // no-op.
    let linked = if lockdown {
        access::principals(from_root, &new_meta.vault_id)
    } else {
        Vec::new()
    };
    let apply_lockdown = |dir: &Path| -> Result<()> {
        if lockdown {
            super::acl::restrict_to_owner(dir, &linked)
        } else {
            Ok(())
        }
    };

    match perform_relocation(
        data_dir,
        from_root,
        to_root,
        new_meta,
        new_key,
        &apply_lockdown,
    ) {
        Ok(conn) => {
            let to_resolved = super::ResolvedVault {
                vault_root: to_root.to_path_buf(),
                db_path: to_root.join(DB_FILENAME),
                markdown_dir: to_root.join(MARKDOWN_DIRNAME),
            };
            let new_master = master_from_db_key_hex(new_key.expose())?;
            state.open_session(
                conn,
                VaultRuntime::build(&to_resolved, new_meta, &new_master),
            )?;
            Ok(())
        }
        Err(e) => {
            // Nothing committed. Reopen the SOURCE so the session survives the failed move —
            // Phase A already committed at `from_root`, so it's the live, openable vault
            // (shareable but still in the profile folder, which the wizard's Move step
            // finishes later). Best-effort: a failure here just means a restart is needed.
            if let (Ok(conn), Ok(new_master)) = (
                db::open(&from_root.join(DB_FILENAME), new_key.expose()),
                master_from_db_key_hex(new_key.expose()),
            ) {
                let from_resolved = super::ResolvedVault {
                    vault_root: from_root.to_path_buf(),
                    db_path: from_root.join(DB_FILENAME),
                    markdown_dir: from_root.join(MARKDOWN_DIRNAME),
                };
                let _ = state.open_session(
                    conn,
                    VaultRuntime::build(&from_resolved, new_meta, &new_master),
                );
            }
            Err(e)
        }
    }
}

/// The testable core of a relocate (between closing the source connection and reopening at
/// the destination): re-check the target, lock it down, probe, copy, probe, open the real
/// DB, then commit the pointer and drop the source. Returns the opened destination
/// connection once committed. On any PRE-commit failure it resets + discards the partial
/// destination and clears the journal — the source is untouched and the pointer never
/// moved. Takes paths + a lockdown closure (not `AppState`), so it exercises in a tempdir.
fn perform_relocation(
    data_dir: &Path,
    from_root: &Path,
    to_root: &Path,
    new_meta: &VaultMeta,
    new_key: &Secret,
    apply_lockdown: &dyn Fn(&Path) -> Result<()>,
) -> Result<rusqlite::Connection> {
    // Re-check the target FIRST — the collision guard ran before Phase A (a rekey/convert
    // ago), so a concurrent wizard could have dropped its own vault here since. A foreign /
    // unreadable target is returned WITHOUT any cleanup touching it (it isn't ours), so we
    // can never strip a bystander vault's ACLs or files.
    match relocation_target_state(&load_meta(to_root), &new_meta.vault_id) {
        TargetState::ForeignVault | TargetState::Unreadable => {
            let _ = clear_journal(data_dir);
            return Err(Error::Other(
                "another PM vault appeared in that folder while this move was preparing — nothing \
                 was overwritten; pick a different folder and try again"
                    .into(),
            ));
        }
        TargetState::Vacant | TargetState::SameVault => {}
    }

    // Everything from here writes into `to_root`, which the marker now proves is ours — so
    // the shared error cleanup below may safely reset + discard it.
    let attempt = || -> Result<rusqlite::Connection> {
        std::fs::create_dir_all(to_root)
            .map_err(crate::error::io_at("prepare the vault folder", to_root))?;
        // Drop the provenance marker BEFORE the copy, so even an interrupted copy is
        // positively discardable by recover() (or the abort path here).
        std::fs::write(to_root.join(VAULT_MARKER), b"pm-vault\n")
            .map_err(crate::error::io_at("prepare the vault folder", to_root))?;
        // Lock the near-empty destination down to its owner (FATAL now), then prove THIS
        // process can still open a handle into it — before copying a possibly-large vault.
        apply_lockdown(to_root)?;
        super::preflight::probe_dir_access(to_root)?;
        // Copy, the full effective-access probe, then the real SQLCipher open — the deepest
        // pre-commit proof that the vault is genuinely usable at the destination.
        copy_vault_artifacts(from_root, to_root)?;
        super::preflight::probe_vault_access(to_root)?;
        let conn = db::open(&to_root.join(DB_FILENAME), new_key.expose())?;
        // Commit: flip the pointer. After this the move is done.
        pointer::store(data_dir, &pointer::VaultPointer::new(to_root.to_path_buf()))?;
        Ok(conn)
    };

    match attempt() {
        Ok(conn) => {
            // Committed. EVERYTHING here is best-effort — the pointer already flipped, so a
            // failure now must not turn a successful move into a reported error (which would
            // route the caller into the abort path and reopen the already-deleted source).
            // `clear_journal` therefore swallows its error like its two siblings below (a stale
            // journal is harmless: recover()'s Relocating→now==target arm finishes the cleanup
            // on the next launch).
            delete_vault_artifacts(from_root);
            let _ = std::fs::remove_file(to_root.join(VAULT_MARKER));
            let _ = clear_journal(data_dir);
            Ok(conn)
        }
        Err(e) => {
            // Nothing committed. Undo the destination we started: reset a possibly-botched
            // lockdown so the partial copy is deletable, discard it (marker-gated), and clear
            // the journal (the pointer never moved, so a leftover would only cause a spurious
            // repair prompt on next boot).
            let _ = super::acl::reset_inheritance(to_root);
            discard_partial_target(to_root);
            let _ = clear_journal(data_dir);
            Err(e)
        }
    }
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
            // The default everywhere: a re-key carries its owner forward. `Claim` is reached only
            // through the confirmed-takeover branch of `change_vault_passphrase`.
            owner: OwnerOnRekey::Keep,
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
    fn a_new_passphrase_is_keyed_to_the_exact_bytes_the_user_typed() {
        // The lockout #298 was meant to close and didn't: this site trimmed before deriving
        // while every read path (unlock, adopt, restore) hashes raw, so a vault created or
        // re-keyed with a padded passphrase was keyed to a string its owner could never
        // retype. Nothing on disk records which form built a key, and the creator's cached
        // key hides it until someone actually types the passphrase — a second account
        // joining, or an unlock after "forget passphrase here". kdf.rs's Rule-1 test guards
        // `derive_master` itself and so cannot see a trim in a CALLER; this one can.
        //
        // Deliberately expensive (~6s measured): it drives the REAL create path, so it pays a
        // full `kdf::calibrate` — a dozen-odd Argon2 derivations each targeting 350ms — plus two
        // more to check the result. `new_passphrase_for` locks the same rule in microseconds; this
        // one exists to prove the wiring underneath it (that prepare_shareable really keys the meta
        // and verifier to those bytes), which is worth the seconds for the vault's master key.
        let padded = "  correct horse battery staple  ";
        let dir = tempfile::tempdir().unwrap();
        let old = super::super::ensure_device_meta(dir.path()).unwrap();
        let (key, meta) = plan_new_key_and_meta(
            &old,
            &plan(
                KeyMode::Passphrase,
                Some(padded),
                MarkdownEncryption::XChaCha20Poly1305,
            ),
        )
        .unwrap();

        // What unlock does, verbatim: derive from the exact bytes, check the verifier.
        let typed = crate::vault::derive_master_from_passphrase(&meta, padded).unwrap();
        assert!(
            crate::vault::verifier::check(meta.verifier.as_ref().unwrap(), &typed).unwrap(),
            "the passphrase the user typed must open the vault it just created"
        );
        assert_eq!(key.expose(), crate::vault::db_key_hex(&typed).expose());

        // ...and the trimmed form — what the bug keyed it to — must NOT be the key.
        let trimmed = crate::vault::derive_master_from_passphrase(&meta, padded.trim()).unwrap();
        assert_ne!(key.expose(), crate::vault::db_key_hex(&trimmed).expose());

        // The re-key keeps the vault's identity (prepare_shareable's contract).
        assert_eq!(meta.vault_id, old.vault_id);
    }

    #[test]
    fn private_meta_sheds_shareable_attributes_including_owner_sid() {
        // Making a vault private (make-private, or a cross-machine restore adopted as private) must
        // drop every passphrase-vault attribute — crucially `owner_sid`, a machine/account-scoped SID
        // that off its origin account would gate connector setup against a foreign owner. Pure, so it
        // needs no keychain.
        let mut shareable = VaultMeta::new_device();
        shareable.key_mode = KeyMode::Passphrase;
        shareable.owner_sid = Some("S-1-5-21-1111111111-2222222222-3333333333-1001".into());
        shareable.ownership_transfer = Some(super::super::OwnershipTransfer {
            from_sid: Some("S-1-5-21-1111111111-2222222222-3333333333-1002".into()),
            to_sid: shareable.owner_sid.clone(),
            at: "2026-01-02T03:04:05+00:00".into(),
        });
        shareable.markdown.encryption = MarkdownEncryption::XChaCha20Poly1305;

        let priv_meta = private_meta(&shareable, MarkdownEncryption::None);
        assert_eq!(priv_meta.key_mode, KeyMode::Device);
        assert_eq!(
            priv_meta.owner_sid, None,
            "a device vault carries no owner SID"
        );
        assert_eq!(
            priv_meta.ownership_transfer, None,
            "and no record of a takeover of a share that no longer exists"
        );
        assert!(priv_meta.kdf.is_none());
        assert!(priv_meta.verifier.is_none());
        assert!(
            priv_meta.meta_mac.is_none(),
            "the cloned MAC is dropped so the next open stamps a fresh one"
        );
        assert_eq!(priv_meta.markdown.encryption, MarkdownEncryption::None);
        assert_eq!(
            priv_meta.vault_id, shareable.vault_id,
            "the vault identity is preserved"
        );
    }

    #[test]
    fn new_passphrase_for_hands_over_the_exact_bytes_and_blank_means_no_re_key() {
        // kdf.rs Rule 1 at the one site that broke it, locked without deriving a key. The first
        // assertion is the whole regression: padding must reach the KDF, not a trimmed copy.
        let md = MarkdownEncryption::XChaCha20Poly1305;
        let padded = plan(KeyMode::Passphrase, Some("  correct horse  "), md);
        assert_eq!(new_passphrase_for(&padded), Some("  correct horse  "));

        // The trim that survives: it classifies, it never edits. Blank in any form means "no new
        // passphrase" (a move with no re-key), exactly as before the fix.
        assert_eq!(
            new_passphrase_for(&plan(KeyMode::Passphrase, Some("   "), md)),
            None
        );
        assert_eq!(
            new_passphrase_for(&plan(KeyMode::Passphrase, Some(""), md)),
            None
        );
        assert_eq!(
            new_passphrase_for(&plan(KeyMode::Passphrase, None, md)),
            None
        );
    }

    #[test]
    fn a_device_vault_with_a_blank_passphrase_is_told_to_supply_one() {
        // Not the "no re-key" branch (that one needs a Passphrase old_meta and a keychain): this
        // pins the OTHER half of the None arm — a device vault asked to go shareable with nothing
        // to derive from must refuse, never silently key itself to "   " or to the device key.
        let dir = tempfile::tempdir().unwrap();
        let device = super::super::ensure_device_meta(dir.path()).unwrap();
        let err = plan_new_key_and_meta(
            &device,
            &plan(
                KeyMode::Passphrase,
                Some("   "),
                MarkdownEncryption::XChaCha20Poly1305,
            ),
        )
        .unwrap_err();
        assert!(err.to_string().contains("passphrase is required"));
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
    fn vault_artifacts_carry_every_sidecar() {
        // A move must take the WHOLE artifact set, and a delete must remove it. The set drifted
        // once: the two encrypted sidecars were vault members to the backup packer and the wipe
        // but not to these two helpers, so every move stranded readable ciphertext at the old
        // root. Anything the backup packs, these must carry.
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join(DB_FILENAME), b"db").unwrap();
        std::fs::write(from.join(access::ACCESS_FILENAME), b"{}").unwrap();
        std::fs::write(from.join(crate::entities::RULES_FILENAME), b"rules").unwrap();
        std::fs::write(from.join(crate::index_only::MANIFEST_FILENAME), b"index").unwrap();

        copy_vault_artifacts(&from, &to).unwrap();
        for name in [
            access::ACCESS_FILENAME,
            crate::entities::RULES_FILENAME,
            crate::index_only::MANIFEST_FILENAME,
        ] {
            assert!(to.join(name).exists(), "{name} must travel with the vault");
        }

        // The source is untouched until the caller commits the move.
        assert!(from.join(crate::entities::RULES_FILENAME).exists());

        delete_vault_artifacts(&from);
        for name in [
            access::ACCESS_FILENAME,
            crate::entities::RULES_FILENAME,
            crate::index_only::MANIFEST_FILENAME,
        ] {
            assert!(
                !from.join(name).exists(),
                "{name} must not be stranded at the old root"
            );
        }
    }

    #[test]
    fn a_vault_with_no_sidecars_still_moves() {
        // Every sidecar is optional (a vault that never minted an entity has no rules file).
        // A missing one must not fail the move — copy_file_verified would error on absent input.
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join(DB_FILENAME), b"db").unwrap();

        copy_vault_artifacts(&from, &to).unwrap();
        assert!(to.join(DB_FILENAME).exists());
        assert!(!to.join(crate::entities::RULES_FILENAME).exists());
    }

    #[test]
    fn delete_vault_artifacts_spares_the_writer_lock() {
        // The lock is NOT ours to delete: this helper also runs from boot recovery, where
        // another instance may hold the lock on a shared root. Removing a foreign lock would
        // invite two writers into one vault. The ephemeral baton files do go.
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(DB_FILENAME), b"db").unwrap();
        std::fs::write(root.path().join("vault.lock"), b"held").unwrap();

        delete_vault_artifacts(root.path());
        assert!(
            root.path().join("vault.lock").exists(),
            "a foreign writer lock must survive an artifact delete"
        );
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

    /// Build a minimal, openable device vault at `root` keyed with `key_hex`, for the
    /// perform_relocation round-trip tests (a real SQLCipher DB + meta + Markdown dir).
    fn build_test_vault(root: &Path, key_hex: &str) -> VaultMeta {
        std::fs::create_dir_all(root.join(MARKDOWN_DIRNAME)).unwrap();
        let conn = db::open(&root.join(DB_FILENAME), key_hex).unwrap();
        conn.execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (7);")
            .unwrap();
        drop(conn);
        let meta = VaultMeta::new_device();
        store_meta(root, &meta).unwrap();
        meta
    }

    #[test]
    fn perform_relocation_commits_then_drops_the_source() {
        // The verify-then-commit happy path: with a no-op lockdown the destination is
        // built, probed, opened, and committed — pointer flipped, source removed, marker
        // and journal gone, and the destination opens with the same key.
        let dd = tempfile::tempdir().unwrap();
        let src = dd.path().join("src");
        let dst = dd.path().join("dst");
        let key = "ab".repeat(32); // 64 hex chars
        let meta = build_test_vault(&src, &key);
        let secret = Secret::from(key.clone());
        write_journal(
            dd.path(),
            &MigrationJournal {
                stage: MigrationStage::Relocating,
                started_at: "2026-07-13T00:00:00Z".into(),
                from_root: src.clone(),
                backup_dir: None,
                target_location: Some(dst.clone()),
            },
        )
        .unwrap();

        let noop = |_: &Path| -> Result<()> { Ok(()) };
        let conn =
            perform_relocation(dd.path(), &src, &dst, &meta, &secret, &noop).expect("commit");
        drop(conn);

        assert_eq!(
            pointer::load(dd.path()).unwrap().unwrap().vault_root,
            dst,
            "the pointer commits to the destination"
        );
        assert!(!src.join(DB_FILENAME).exists(), "the source is dropped");
        assert!(!dst.join(VAULT_MARKER).exists(), "the marker is cleared");
        assert!(
            read_journal(dd.path()).unwrap().is_none(),
            "journal cleared"
        );
        assert!(
            db::open(&dst.join(DB_FILENAME), &key).is_ok(),
            "the destination vault opens"
        );
    }

    #[test]
    fn perform_relocation_aborts_on_lockdown_failure_leaving_source_intact() {
        // The abort path: a fatal lockdown failure before the commit must leave the SOURCE
        // untouched and openable, the pointer unmoved, the partial destination discarded,
        // and the journal cleared — nothing half-committed.
        let dd = tempfile::tempdir().unwrap();
        let src = dd.path().join("src");
        let dst = dd.path().join("dst");
        let key = "cd".repeat(32);
        let meta = build_test_vault(&src, &key);
        let secret = Secret::from(key.clone());
        write_journal(
            dd.path(),
            &MigrationJournal {
                stage: MigrationStage::Relocating,
                started_at: "2026-07-13T00:00:00Z".into(),
                from_root: src.clone(),
                backup_dir: None,
                target_location: Some(dst.clone()),
            },
        )
        .unwrap();

        let boom = |_: &Path| -> Result<()> { Err(Error::Other("lockdown refused".into())) };
        let err = perform_relocation(dd.path(), &src, &dst, &meta, &secret, &boom);
        assert!(err.is_err(), "a lockdown failure aborts the move");

        assert!(
            db::open(&src.join(DB_FILENAME), &key).is_ok(),
            "the source vault is intact and still opens"
        );
        assert!(
            pointer::load(dd.path()).unwrap().is_none(),
            "the pointer never moved"
        );
        assert!(
            !dst.join(DB_FILENAME).exists(),
            "the partial destination is discarded"
        );
        assert!(
            read_journal(dd.path()).unwrap().is_none(),
            "journal cleared"
        );
    }

    #[test]
    fn perform_relocation_refuses_a_foreign_target_without_touching_it() {
        // A foreign vault that appears at the destination between the collision guard and the
        // copy must be left completely alone — no reset, no discard, source intact.
        let dd = tempfile::tempdir().unwrap();
        let src = dd.path().join("src");
        let dst = dd.path().join("dst");
        let key = "ef".repeat(32);
        let meta = build_test_vault(&src, &key);
        // A DIFFERENT vault already sits at the destination.
        let mut foreign = VaultMeta::new_device();
        foreign.vault_id = "someone-elses-vault".into();
        std::fs::create_dir_all(&dst).unwrap();
        store_meta(&dst, &foreign).unwrap();
        std::fs::write(dst.join(DB_FILENAME), b"foreign-db").unwrap();
        let secret = Secret::from(key.clone());

        let noop = |_: &Path| -> Result<()> { Ok(()) };
        let err = perform_relocation(dd.path(), &src, &dst, &meta, &secret, &noop);
        assert!(err.is_err(), "a foreign target is refused");
        assert_eq!(
            std::fs::read(dst.join(DB_FILENAME)).unwrap(),
            b"foreign-db",
            "the bystander vault's files are untouched"
        );
        assert!(
            db::open(&src.join(DB_FILENAME), &key).is_ok(),
            "the source is intact"
        );
        assert!(pointer::load(dd.path()).unwrap().is_none());
    }

    #[test]
    fn copy_vault_artifacts_copies_db_markdown_and_meta_keeping_source() {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("old");
        let to = root.path().join("new");
        std::fs::create_dir_all(from.join(MARKDOWN_DIRNAME)).unwrap();
        std::fs::write(from.join(DB_FILENAME), b"db-bytes").unwrap();
        std::fs::write(meta_path(&from), b"{}").unwrap();
        std::fs::write(
            from.join(MARKDOWN_DIRNAME).join("note.md.pmenc"),
            b"ciphertext",
        )
        .unwrap();

        copy_vault_artifacts(&from, &to).unwrap();

        // Source is intact (the move only deletes it after the pointer commits)...
        assert!(from.join(DB_FILENAME).exists());
        // ...and every artifact landed at the destination.
        assert_eq!(std::fs::read(to.join(DB_FILENAME)).unwrap(), b"db-bytes");
        assert_eq!(
            std::fs::read(to.join(MARKDOWN_DIRNAME).join("note.md.pmenc")).unwrap(),
            b"ciphertext"
        );
        assert!(meta_path(&to).exists());

        // delete_vault_artifacts removes exactly those, leaving other files alone.
        std::fs::write(from.join("unrelated.txt"), b"keep").unwrap();
        delete_vault_artifacts(&from);
        assert!(!from.join(DB_FILENAME).exists());
        assert!(!from.join(MARKDOWN_DIRNAME).exists());
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
