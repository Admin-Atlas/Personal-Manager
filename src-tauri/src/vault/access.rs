// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The linked-accounts sidecar (`vault-access.json`): the non-secret list of OS
//! principals (account names or SIDs) the owner has granted filesystem access to a
//! shared vault folder. It exists so a later move can re-apply the grants —
//! `restrict_to_owner` strips inheritance and would otherwise wipe every linked
//! account's ACE. Deliberately a sibling file rather than a `vault-meta.json` field:
//! per-user installs mean two profiles can run different PM versions against the same
//! folder, and an older build's meta-MAC repair would silently strip a field it
//! doesn't know, whereas it simply ignores an extra file. Advisory, like the ACLs it
//! feeds — encryption is the real protection, and tampering with this file already
//! requires the folder access it describes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Filename of the linked-accounts sidecar, stored inside the vault folder.
pub const ACCESS_FILENAME: &str = "vault-access.json";

/// The on-disk `vault-access.json`. Bound to its vault by id so a file copied into a
/// different vault's folder is ignored rather than granting stale principals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultAccess {
    pub schema: u32,
    pub vault_id: String,
    pub principals: Vec<String>,
}

impl VaultAccess {
    pub fn new(vault_id: &str) -> Self {
        Self {
            schema: 1,
            vault_id: vault_id.to_string(),
            principals: Vec::new(),
        }
    }
}

fn access_path(vault_root: &Path) -> PathBuf {
    vault_root.join(ACCESS_FILENAME)
}

/// Read the sidecar if present AND belonging to `expected_vault_id`; a missing file or
/// a vault-id mismatch (a file copied from another vault) both read as `None`.
pub fn load(vault_root: &Path, expected_vault_id: &str) -> Result<Option<VaultAccess>> {
    let path = access_path(vault_root);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let access: VaultAccess = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Other(format!("{ACCESS_FILENAME} is unreadable: {e}")))?;
            Ok((access.vault_id == expected_vault_id).then_some(access))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write the sidecar atomically (temp file in the same dir, then rename).
pub fn store(vault_root: &Path, access: &VaultAccess) -> Result<()> {
    std::fs::create_dir_all(vault_root)?;
    let path = access_path(vault_root);
    let json = serde_json::to_vec_pretty(access)
        .map_err(|e| Error::Other(format!("could not encode {ACCESS_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The linked principals for a vault, best-effort: an unreadable or foreign sidecar
/// reads as "none linked" (the grants themselves still live in the filesystem ACL).
pub fn principals(vault_root: &Path, vault_id: &str) -> Vec<String> {
    load(vault_root, vault_id)
        .ok()
        .flatten()
        .map(|a| a.principals)
        .unwrap_or_default()
}

/// Case-insensitive identity for a principal: Windows account names and SIDs both
/// compare case-insensitively, and pasted values arrive with stray whitespace.
fn normalized(principal: &str) -> String {
    principal.trim().to_ascii_lowercase()
}

/// Add a principal to the list, trimmed and deduplicated (case-insensitive); an empty
/// or already-present principal leaves the list unchanged. Pure.
pub fn merge_principal(principals: &[String], account: &str) -> Vec<String> {
    let mut out: Vec<String> = principals.to_vec();
    let candidate = account.trim();
    if candidate.is_empty() {
        return out;
    }
    let key = normalized(candidate);
    if !out.iter().any(|p| normalized(p) == key) {
        out.push(candidate.to_string());
    }
    out
}

/// Remove a principal from the list (case-insensitive, trimmed). Pure.
pub fn remove_principal(principals: &[String], account: &str) -> Vec<String> {
    let key = normalized(account);
    principals
        .iter()
        .filter(|p| normalized(p) != key)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sidecar_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "vault-1").unwrap(), None);
    }

    #[test]
    fn store_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut access = VaultAccess::new("vault-1");
        access.principals = vec!["S-1-5-21-111-222-333-1002".into(), "PC\\alice".into()];
        store(dir.path(), &access).unwrap();
        assert_eq!(load(dir.path(), "vault-1").unwrap(), Some(access));
    }

    #[test]
    fn a_foreign_vaults_sidecar_is_ignored() {
        // A vault-access.json copied in from another vault must not grant its principals here.
        let dir = tempfile::tempdir().unwrap();
        let mut access = VaultAccess::new("other-vault");
        access.principals = vec!["PC\\mallory".into()];
        store(dir.path(), &access).unwrap();
        assert_eq!(load(dir.path(), "vault-1").unwrap(), None);
        assert!(principals(dir.path(), "vault-1").is_empty());
    }

    #[test]
    fn merge_trims_dedupes_case_insensitively_and_skips_empty() {
        let start = vec!["PC\\Alice".to_string()];
        // Same account, different case + whitespace: no duplicate.
        assert_eq!(merge_principal(&start, "  pc\\alice "), start);
        // A SID appends, keeping the pasted (trimmed) form.
        let with_sid = merge_principal(&start, " S-1-5-21-111-222-333-1002 ");
        assert_eq!(
            with_sid,
            vec![
                "PC\\Alice".to_string(),
                "S-1-5-21-111-222-333-1002".to_string()
            ]
        );
        // Empty input is a no-op, never an empty-string principal.
        assert_eq!(merge_principal(&with_sid, "   "), with_sid);
    }

    #[test]
    fn remove_is_case_insensitive_and_keeps_others() {
        let list = vec![
            "PC\\Alice".to_string(),
            "S-1-5-21-111-222-333-1002".to_string(),
        ];
        assert_eq!(
            remove_principal(&list, "pc\\ALICE"),
            vec!["S-1-5-21-111-222-333-1002".to_string()]
        );
        assert_eq!(remove_principal(&list, "PC\\nobody"), list);
    }
}
