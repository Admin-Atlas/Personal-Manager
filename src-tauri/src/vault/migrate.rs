// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The one vault migration routine (spec §6). Every mode change — make shareable,
//! make private, change passphrase, move location — is expressed as a [`MigrationPlan`]
//! and run by a single ordered, crash-aware routine. That gives one place to enforce
//! the "shared ⇒ encrypted at rest" invariant and to sequence the one genuinely
//! dangerous step (the non-transactional `PRAGMA rekey`) behind a recovery journal.
//!
//! This file is the foundation: the plan + its validation, the recovery journal, and
//! the copy-verify-delete relocate primitive. The orchestration that drives them in
//! order — checkpoint, back up, rekey, convert Markdown, relocate, ACL, then flip the
//! metadata/pointer/keychain last — is layered on top of these pieces.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{KeyMode, MarkdownEncryption};
use crate::error::{Error, Result};

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
    /// Enforce the security invariants *before* any destructive step runs. The crucial
    /// one (spec §3): a shareable (passphrase) vault MUST encrypt its Markdown at rest —
    /// once it can be opened from another account, folder isolation no longer protects
    /// the notes. Also require a non-empty passphrase whenever the target is shareable.
    pub fn validate(&self) -> Result<()> {
        if self.target_key_mode == KeyMode::Passphrase {
            if self.target_markdown != MarkdownEncryption::XChaCha20Poly1305 {
                return Err(Error::Other(
                    "a shareable vault must encrypt its Markdown at rest".into(),
                ));
            }
            let has_passphrase = self
                .new_passphrase
                .as_deref()
                .map(|p| !p.trim().is_empty())
                .unwrap_or(false);
            if !has_passphrase {
                return Err(Error::Other(
                    "a passphrase is required for a shareable vault".into(),
                ));
            }
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
    /// why a `VACUUM INTO` backup is taken first (`backup_db`).
    Rekeying,
    /// Re-encrypting / decrypting the Markdown to match the new policy.
    Markdown,
    /// Copying the vault to a new location (copy-verify-delete).
    Relocating,
    /// Writing the new metadata + pointer + keychain entry; the point of no return.
    Finalizing,
}

/// On-disk record of an in-flight migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationJournal {
    pub stage: MigrationStage,
    pub started_at: String,
    /// Path to the `VACUUM INTO` snapshot of the pre-rekey DB (for recovery).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub backup_db: Option<PathBuf>,
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

// --- relocate (copy-verify-delete) ------------------------------------------------

/// Move a vault tree from `src` to `dst` by copying every file, verifying each copy's
/// length, and only then removing the source. Used instead of a rename so a move can
/// cross volumes (a rename can't) and so a failure mid-copy leaves the source intact —
/// the destination is the disposable side until the copy is fully verified.
pub fn relocate_tree(src: &Path, dst: &Path) -> Result<()> {
    copy_tree_verified(src, dst)?;
    std::fs::remove_dir_all(src)?;
    Ok(())
}

/// Recursively copy `src` into `dst`, checking that each destination file ends up the
/// same length as its source (a cheap integrity check that the copy completed).
fn copy_tree_verified(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_tree_verified(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
            let src_len = std::fs::metadata(&path)?.len();
            let dst_len = std::fs::metadata(&target)?.len();
            if src_len != dst_len {
                return Err(Error::Other(format!(
                    "copy verification failed for {} ({src_len} vs {dst_len} bytes)",
                    path.display()
                )));
            }
        }
    }
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
    fn shareable_target_must_encrypt_markdown_and_have_a_passphrase() {
        // The security invariant: passphrase mode + plaintext markdown is rejected.
        assert!(
            plan(KeyMode::Passphrase, Some("pw"), MarkdownEncryption::None)
                .validate()
                .is_err()
        );
        // ...and a missing/blank passphrase is rejected.
        assert!(plan(
            KeyMode::Passphrase,
            None,
            MarkdownEncryption::XChaCha20Poly1305
        )
        .validate()
        .is_err());
        assert!(plan(
            KeyMode::Passphrase,
            Some("   "),
            MarkdownEncryption::XChaCha20Poly1305
        )
        .validate()
        .is_err());
        // A valid shareable plan passes.
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
            backup_db: Some(dir.path().join("backup.sqlite")),
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
    fn relocate_copies_verifies_and_removes_the_source() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("old");
        let dst = root.path().join("new");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("pm.sqlite"), b"db-bytes").unwrap();
        std::fs::write(src.join("sub").join("note.md.pmenc"), b"ciphertext").unwrap();

        relocate_tree(&src, &dst).unwrap();

        assert!(!src.exists(), "source removed after a verified copy");
        assert_eq!(std::fs::read(dst.join("pm.sqlite")).unwrap(), b"db-bytes");
        assert_eq!(
            std::fs::read(dst.join("sub").join("note.md.pmenc")).unwrap(),
            b"ciphertext"
        );
    }
}
