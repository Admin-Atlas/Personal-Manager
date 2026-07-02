// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Destination-neutral naming, validation, and retention selection for `.pmbackup` archives.
//!
//! These are the pure pieces shared by every backup destination (Proton Drive CLI,
//! Google Drive REST, …): how an archive is named, whether a bare name is safe to splice
//! into a remote/local path, and — given a listing — which archives to trim for keep-last-N
//! retention. The naming scheme is deliberately identical across destinations, so a vault
//! backed up to two places carries the same, attributable, chronologically-sortable names.

use serde::Serialize;

/// Archive extension, also the retention/listing filter (only PM's own files are touched).
pub(crate) const ARCHIVE_EXT: &str = ".pmbackup";

/// A backup archive found in a remote folder — surfaced to the UI's restore list. Shared by
/// every destination (Proton, Google Drive), so the restore picker renders one shape.
#[derive(Serialize)]
pub struct BackupEntry {
    pub name: String,
    /// Cleartext size in bytes, when the destination reported it.
    pub size: Option<u64>,
}

/// This vault's stable archive-name prefix, so retention only ever considers archives THIS vault
/// created — never another device/vault sharing the same account + folder. The vault id is
/// reduced to `[A-Za-z0-9]` so it is a safe path segment.
pub(crate) fn archive_prefix(vault_id: &str) -> String {
    let id: String = vault_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    format!("pm-backup-{id}-")
}

/// The archive file name for a backup of `vault_id` taken at `stamp` (a compact, filesystem- and
/// URL-safe UTC stamp `YYYYMMDDTHHMMSSZ`). Carries the vault id so archives are attributable and
/// retention is per-vault; the trailing stamp keeps same-vault names chronologically sortable.
pub(crate) fn archive_name(vault_id: &str, stamp: &str) -> String {
    format!("{}{stamp}{ARCHIVE_EXT}", archive_prefix(vault_id))
}

/// Whether `name` is a safe bare archive file name to interpolate into a remote/local path:
/// non-empty, no path separators or traversal, and our own extension. Guards a name the UI hands
/// back to a restore command before it's spliced into a CLI path argument or a REST query.
pub(crate) fn valid_archive_name(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(ARCHIVE_EXT)
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.contains('\0')
}

/// Which archive names to trim to keep only the newest `keep_n`. Pure + testable: names end in a
/// `<UTC-stamp>.pmbackup` suffix, so a reverse lexical sort is reverse-chronological, and
/// everything past the first `keep_n` is stale. `keep_n` is clamped to `>= 1` HERE (not only in
/// callers) so this can never, on its own, select every archive for deletion.
pub(crate) fn select_for_deletion(names: &[String], keep_n: usize) -> Vec<String> {
    let keep_n = keep_n.max(1);
    let mut sorted = names.to_vec();
    sorted.sort_by(|a, b| b.cmp(a)); // newest (lexically greatest) first
    sorted.into_iter().skip(keep_n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_keeps_newest_n_and_never_wipes_all() {
        let names = vec![
            "pm-backup-20260101T000000Z.pmbackup".to_string(),
            "pm-backup-20260303T000000Z.pmbackup".to_string(),
            "pm-backup-20260202T000000Z.pmbackup".to_string(),
        ];
        // Keep the 2 newest → only the oldest (Jan) is trashed.
        assert_eq!(
            select_for_deletion(&names, 2),
            vec!["pm-backup-20260101T000000Z.pmbackup".to_string()]
        );
        // keep_n >= count → nothing trashed.
        assert!(select_for_deletion(&names, 3).is_empty());
        assert!(select_for_deletion(&names, 9).is_empty());
        // keep 1 → the two oldest go; the newest (Mar) is retained.
        let doomed = select_for_deletion(&names, 1);
        assert_eq!(doomed.len(), 2);
        assert!(!doomed.contains(&"pm-backup-20260303T000000Z.pmbackup".to_string()));
        // keep_n == 0 is clamped to 1 — never a total wipe.
        assert_eq!(select_for_deletion(&names, 0).len(), 2);
    }

    #[test]
    fn archive_prefix_is_per_vault_and_sanitized() {
        assert_eq!(archive_prefix("abc-123"), "pm-backup-abc123-");
        let name = archive_name("vaultA", "20260101T000000Z");
        assert_eq!(name, "pm-backup-vaultA-20260101T000000Z.pmbackup");
        assert!(name.starts_with(&archive_prefix("vaultA")));
        assert!(!name.starts_with(&archive_prefix("vaultB")));
        assert!(valid_archive_name(&name));
    }

    #[test]
    fn archive_name_is_sortable_and_valid() {
        let name = archive_name("v", "20260702T161659Z");
        assert_eq!(name, "pm-backup-v-20260702T161659Z.pmbackup");
        assert!(valid_archive_name(&name));
    }

    #[test]
    fn valid_archive_name_rejects_paths_and_traversal() {
        assert!(!valid_archive_name(""));
        assert!(!valid_archive_name("notes.txt"));
        assert!(!valid_archive_name("../secret.pmbackup"));
        assert!(!valid_archive_name("a/b.pmbackup"));
        assert!(!valid_archive_name("a\\b.pmbackup"));
        assert!(!valid_archive_name("x\0.pmbackup"));
        assert!(valid_archive_name("pm-backup-20260101T000000Z.pmbackup"));
    }
}
