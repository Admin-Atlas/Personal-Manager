// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared-vault advertisements: how a second Windows account *discovers* that a shared
//! vault exists at all. When a shareable vault lands in (or is re-linked from) a folder
//! outside the owner's profile, PM drops one small, non-secret marker per vault under a
//! world-readable fixed location (`%ProgramData%\Personal Manager\shared-vaults\<vault_id>.json`);
//! at first launch on another account, PM lists these and offers to join. Purely
//! advisory: adopting re-validates the real `vault-meta.json` and the passphrase is the
//! actual gate, so a tampered or stale marker buys an attacker nothing — at worst the
//! join offer points at a folder that no longer answers. One file per vault because
//! ProgramData's default ACLs give each creating user CREATOR-OWNER rights over their
//! own files, so two owners never fight over a shared list. No equivalent world-shared
//! default exists on Linux/macOS, so discovery is Windows-only there — the manual
//! "open an existing shared vault" folder pick still works everywhere.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One advertised shared vault. Non-secret by construction: the folder path, a label
/// for the join screen, and the owner's OS account name (already public to every
/// account on the machine via `C:\Users`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedVaultAd {
    pub schema: u32,
    pub vault_id: String,
    pub vault_root: PathBuf,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub owner: Option<String>,
    pub updated_at: String,
    /// A TOMBSTONE marker: when set (RFC3339), the owner deliberately DELETED this shared
    /// vault. It stays in place as positive evidence so a joiner — whose pointed folder is
    /// now gone — can tell "the owner deleted it" (switch back to a local vault, with a
    /// notice) apart from "the drive is unplugged" (offer Retry). Never shown as a join
    /// offer. Additive + backward-compatible: an old ad without the field reads as `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deleted_at: Option<String>,
}

impl SharedVaultAd {
    /// Build the advertisement for a vault at `vault_root`: labelled by its folder
    /// name, owned by the current OS account.
    pub fn for_vault(vault_id: &str, vault_root: &Path) -> Self {
        let label = vault_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Shared vault".to_string());
        let owner = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .ok();
        Self {
            schema: 1,
            vault_id: vault_id.to_string(),
            vault_root: vault_root.to_path_buf(),
            label,
            owner,
            updated_at: chrono::Utc::now().to_rfc3339(),
            deleted_at: None,
        }
    }

    /// Build a TOMBSTONE ad for a vault the owner just deleted: same identity, with
    /// `deleted_at` stamped now. Overwrites the live ad (see [`publish`], which now treats
    /// a tombstone as changed content), so a joiner's next launch finds the deletion.
    pub fn tombstone(vault_id: &str, vault_root: &Path) -> Self {
        Self {
            deleted_at: Some(chrono::Utc::now().to_rfc3339()),
            ..Self::for_vault(vault_id, vault_root)
        }
    }

    /// Whether two ads say the same thing (ignoring the timestamp), so a republish of
    /// unchanged facts can skip the write — a joiner's best-effort self-heal would
    /// otherwise fail on the owner-owned file every boot. `deleted_at` IS compared, so a
    /// tombstone always counts as changed content and overwrites a live ad.
    fn same_content(&self, other: &Self) -> bool {
        self.vault_id == other.vault_id
            && self.vault_root == other.vault_root
            && self.label == other.label
            && self.owner == other.owner
            && self.deleted_at == other.deleted_at
    }
}

/// The machine-wide PM base every account can reach (`%ProgramData%\Personal Manager`)
/// — where suggested shared-vault folders and the advertisements live. `None` on
/// platforms without a sane world-shared default (Linux/macOS).
pub fn shared_base_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        Some(base.join("Personal Manager"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The fixed, world-readable advertisements folder, or `None` on platforms without one
/// (see [`shared_base_dir`]; discovery is skipped there).
pub fn ads_dir() -> Option<PathBuf> {
    shared_base_dir().map(|base| base.join("shared-vaults"))
}

fn ad_path(ads_dir: &Path, vault_id: &str) -> PathBuf {
    ads_dir.join(format!("{vault_id}.json"))
}

/// Write (or refresh) a vault's advertisement, atomically. Skips the write when an
/// identical ad is already present — see [`SharedVaultAd::same_content`].
pub fn publish(ads_dir: &Path, ad: &SharedVaultAd) -> Result<()> {
    std::fs::create_dir_all(ads_dir)?;
    let path = ad_path(ads_dir, &ad.vault_id);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(existing) = serde_json::from_slice::<SharedVaultAd>(&bytes) {
            if existing.same_content(ad) {
                return Ok(());
            }
        }
    }
    let json = serde_json::to_vec_pretty(ad)
        .map_err(|e| Error::Other(format!("could not encode the vault advertisement: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove a vault's advertisement (make-private, or the owner cleaning up). A missing
/// file is fine — retraction is idempotent.
pub fn retract(ads_dir: &Path, vault_id: &str) -> Result<()> {
    match std::fs::remove_file(ad_path(ads_dir, vault_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Every readable advertisement in the folder. Best-effort: a missing folder is an
/// empty list, and an unparseable file is skipped rather than failing the lot.
pub fn list(ads_dir: &Path) -> Vec<SharedVaultAd> {
    let Ok(entries) = std::fs::read_dir(ads_dir) else {
        return Vec::new();
    };
    let mut ads = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(ad) = serde_json::from_slice::<SharedVaultAd>(&bytes) {
                ads.push(ad);
            }
        }
    }
    ads.sort_by(|a, b| a.label.cmp(&b.label));
    ads
}

/// Which advertised vaults this profile could actually join: not the vault it already
/// uses, and only folders that still hold vault metadata (`root_has_meta` — injected so
/// the policy tests without a filesystem). Stale ads are filtered, not deleted: only
/// their owner can remove them, and adopt re-validates anyway. Pure.
pub fn filter_adoptable(
    ads: Vec<SharedVaultAd>,
    current_vault_id: Option<&str>,
    root_has_meta: impl Fn(&Path) -> bool,
) -> Vec<SharedVaultAd> {
    ads.into_iter()
        .filter(|ad| ad.deleted_at.is_none()) // a tombstoned vault is never a join offer
        .filter(|ad| Some(ad.vault_id.as_str()) != current_vault_id)
        .filter(|ad| root_has_meta(&ad.vault_root))
        .collect()
}

/// Whether a folder this profile points at was DELETED by its owner — a tombstone ad names
/// it AND no live ad has since re-shared the same path (a re-share supersedes the tombstone,
/// so a fresh live vault there is a normal join, not a deletion). Pure, so the joiner-boot
/// decision unit-tests without the ProgramData folder. The joiner matches by PATH because,
/// once the folder is gone, it can't read the vault id out of it any more.
pub fn deletion_tombstone_for<'a>(
    ads: &'a [SharedVaultAd],
    pointed_root: &Path,
) -> Option<&'a SharedVaultAd> {
    let at_root = |ad: &&SharedVaultAd| ad.vault_root == pointed_root;
    let superseded = ads.iter().filter(at_root).any(|ad| ad.deleted_at.is_none());
    if superseded {
        return None;
    }
    ads.iter()
        .filter(at_root)
        .find(|ad| ad.deleted_at.is_some())
}

/// The first free location for a new shared vault: `base\name`, then `base\name 2`,
/// `base\name 3`, … while `occupied` says a *different* vault already sits there. Pure.
pub fn next_free_location(base: &Path, name: &str, occupied: impl Fn(&Path) -> bool) -> PathBuf {
    let first = base.join(name);
    if !occupied(&first) {
        return first;
    }
    let mut n = 2u32;
    loop {
        let candidate = base.join(format!("{name} {n}"));
        if !occupied(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad(id: &str, root: &str) -> SharedVaultAd {
        SharedVaultAd {
            schema: 1,
            vault_id: id.to_string(),
            vault_root: PathBuf::from(root),
            label: format!("label-{id}"),
            owner: Some("owner-a".into()),
            updated_at: "2026-07-13T00:00:00Z".into(),
            deleted_at: None,
        }
    }

    fn tombstone_ad(id: &str, root: &str) -> SharedVaultAd {
        SharedVaultAd {
            deleted_at: Some("2026-07-13T01:00:00Z".into()),
            ..ad(id, root)
        }
    }

    #[test]
    fn publish_list_retract_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), &ad("v1", "/shared/one")).unwrap();
        publish(dir.path(), &ad("v2", "/shared/two")).unwrap();
        let listed = list(dir.path());
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].vault_id, "v1");
        retract(dir.path(), "v1").unwrap();
        assert_eq!(list(dir.path()).len(), 1);
        // Retracting again (or a never-published id) is a no-op, not an error.
        retract(dir.path(), "v1").unwrap();
    }

    #[test]
    fn republishing_identical_content_leaves_the_file_untouched() {
        // The joiner's best-effort self-heal republishes every boot; unchanged facts
        // must skip the write so it never fails on the owner-owned file.
        let dir = tempfile::tempdir().unwrap();
        let first = ad("v1", "/shared/one");
        publish(dir.path(), &first).unwrap();
        let before = std::fs::metadata(dir.path().join("v1.json"))
            .unwrap()
            .modified()
            .unwrap();
        let mut refreshed = first.clone();
        refreshed.updated_at = "2026-07-14T00:00:00Z".into(); // timestamp alone ≠ new content
        publish(dir.path(), &refreshed).unwrap();
        let after = std::fs::metadata(dir.path().join("v1.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_missing_ads_folder_lists_empty_and_garbage_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(&dir.path().join("nope")).is_empty());
        std::fs::write(dir.path().join("junk.json"), b"not json").unwrap();
        publish(dir.path(), &ad("v1", "/shared/one")).unwrap();
        assert_eq!(list(dir.path()).len(), 1);
    }

    #[test]
    fn filter_drops_own_vault_and_stale_roots() {
        let ads = vec![ad("mine", "/a"), ad("theirs", "/b"), ad("gone", "/c")];
        let out = filter_adoptable(ads, Some("mine"), |root| root != Path::new("/c"));
        assert_eq!(
            out.iter().map(|a| a.vault_id.as_str()).collect::<Vec<_>>(),
            vec!["theirs"]
        );
        // A fresh profile (no current vault) is offered everything still standing.
        let ads = vec![ad("mine", "/a"), ad("gone", "/c")];
        let out = filter_adoptable(ads, None, |root| root != Path::new("/c"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].vault_id, "mine");
    }

    #[test]
    fn a_tombstone_overwrites_the_live_ad_and_is_never_offered() {
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), &ad("v1", "/shared/one")).unwrap();
        // The owner deletes: a tombstone (same vault_id) overwrites the live ad, even
        // though everything but deleted_at matches (same_content now compares deleted_at).
        publish(
            dir.path(),
            &SharedVaultAd::tombstone("v1", Path::new("/shared/one")),
        )
        .unwrap();
        let listed = list(dir.path());
        assert_eq!(listed.len(), 1);
        assert!(listed[0].deleted_at.is_some(), "the ad is now a tombstone");
        // A tombstoned vault is filtered out of join offers even though its root "has meta".
        assert!(filter_adoptable(listed, None, |_| true).is_empty());
    }

    #[test]
    fn deletion_tombstone_matches_by_root_and_a_reshare_supersedes_it() {
        let root = "/shared/one";
        // A tombstone for the pointed root is a positive "owner deleted it" signal.
        let ads = vec![tombstone_ad("old", root)];
        assert!(deletion_tombstone_for(&ads, Path::new(root)).is_some());
        // No tombstone at a different root.
        assert!(deletion_tombstone_for(&ads, Path::new("/shared/other")).is_none());
        // A later LIVE ad re-sharing the same path supersedes the tombstone (a fresh vault
        // there is a normal join, not a deletion notice).
        let ads = vec![tombstone_ad("old", root), ad("new", root)];
        assert!(deletion_tombstone_for(&ads, Path::new(root)).is_none());
    }

    #[test]
    fn next_free_location_suffixes_past_occupied_folders() {
        let base = Path::new("/ProgramData/Personal Manager");
        let free = next_free_location(base, "Shared Vault", |_| false);
        assert_eq!(free, base.join("Shared Vault"));
        let taken: Vec<PathBuf> = vec![base.join("Shared Vault"), base.join("Shared Vault 2")];
        let free = next_free_location(base, "Shared Vault", |p| taken.contains(&p.to_path_buf()));
        assert_eq!(free, base.join("Shared Vault 3"));
    }
}
