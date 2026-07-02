// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `backup-manifest.json` that rides *inside* the encrypted layer of a `.pmbackup`.
//! It carries the one thing the cleartext header must never hold — the source vault's
//! raw DB key (= hex of the master) — so a restore is single-secret (only the backup
//! passphrase) and portable to any machine. It also records provenance the restore
//! uses to validate the archive against its own `vault-meta.json`.

use serde::{Deserialize, Serialize};

/// Manifest schema version (bumped only on a breaking manifest change).
pub const SCHEMA: u32 = 1;
/// The reserved bundle entry name for the manifest; read into memory (never to disk).
pub const MANIFEST_ENTRY: &str = "backup-manifest.json";

/// Provenance + the embedded key, serialized into the encrypted bundle. Not `Serialize`
/// via `Secret` because it must round-trip verbatim; the plaintext exists only briefly
/// in memory (pack) or inside the auto-cleaned staging dir (restore).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub schema: u32,
    pub source_vault_id: String,
    /// `"device"` or `"passphrase"` (the source vault's key mode at backup time).
    pub source_key_mode: String,
    /// The 64-hex SQLCipher key (= hex of the master). The crux of portability.
    pub db_key_hex: String,
    /// `"none"` or `"xchacha20poly1305"` — the source's Markdown-at-rest policy.
    pub source_markdown_encryption: String,
    /// App version that produced the archive (for diagnostics / future migrations).
    pub app_version: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips() {
        let m = BackupManifest {
            schema: SCHEMA,
            source_vault_id: "vault-123".into(),
            source_key_mode: "device".into(),
            db_key_hex: "00".repeat(32),
            source_markdown_encryption: "none".into(),
            app_version: "2.64.0-alpha".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
        };
        let bytes = serde_json::to_vec(&m).unwrap();
        let back: BackupManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.db_key_hex.len(), 64);
    }
}
