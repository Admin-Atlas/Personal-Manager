// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vault key model (spec §2). A vault's non-secret identity + crypto policy live
//! in `vault-meta.json` next to `pm.sqlite`: a stable id, the SQLCipher cipher
//! profile, and — for shareable (passphrase) vaults — the Argon2id salt + params and
//! a wrong-passphrase verifier. The passphrase and the derived key are NEVER written
//! here. Device-only vaults (the default) keep their random key in the OS keychain
//! exactly as before; the meta just records the mode + a vault id, so transitions to
//! shareable later have no special case.

// Phase 1 foundation: several APIs below (passphrase derivation, the Markdown
// subkey, the device-master helper) are first *wired in* by later build items
// (open-existing, shareable creation, Markdown crypto). Drop this allow once
// Build 2–6 land and exercise them on a non-test path.
#![allow(dead_code)]

pub mod kdf;
pub mod pointer;
pub mod verifier;

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use zeroize::Zeroizing;

use crate::db;
use crate::error::{Error, Result};
use crate::paths;
use crate::secret::Secret;
use crate::secrets;
use kdf::{KdfParams, KEY_LEN};
use pointer::VaultPointer;
use verifier::Verifier;

/// Filename of the per-vault, non-secret metadata, stored inside the vault folder.
pub const META_FILENAME: &str = "vault-meta.json";
/// Reverse-DNS app id recorded in the meta (matches the keychain service + bundle id).
pub const APP_ID: &str = "org.itsatlas.pm";
/// Current `vault-meta.json` schema version.
const META_SCHEMA: u32 = 1;
/// BLAKE3 derive_key context for the Markdown-at-rest subkey — distinct from every
/// other use of the master key, so the Markdown key never equals the DB key.
const MARKDOWN_SUBKEY_CONTEXT: &str = "org.itsatlas.pm markdown-at-rest subkey v1";
/// Recorded in the meta so every profile agrees on how the Markdown subkey is made.
const MARKDOWN_SUBKEY_SCHEME: &str = "blake3-derive-key";
/// Target Argon2id derivation time when creating a shareable vault (~mid of the
/// 250–500 ms band; spec §2.2). Calibrated once at creation, then stored + reused.
const CALIBRATE_TARGET_MS: u64 = 350;

/// How a vault's SQLCipher key is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    /// Random key in the OS keychain (today's default; bound to one profile).
    Device,
    /// Key derived from a passphrase via Argon2id (openable from any profile/machine).
    Passphrase,
}

/// Whether the Markdown vault files are ciphertext at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkdownEncryption {
    None,
    XChaCha20Poly1305,
}

/// The SQLCipher cipher profile, recorded so a future SQLCipher bump can't silently
/// change the at-rest format. Mirrors the PRAGMAs in `db::open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbCipher {
    pub cipher_page_size: u32,
    pub kdf_iter: u32,
    pub hmac_algorithm: String,
    pub kdf_algorithm: String,
}

impl Default for DbCipher {
    fn default() -> Self {
        Self {
            cipher_page_size: 4096,
            kdf_iter: 256_000,
            hmac_algorithm: "HMAC_SHA512".to_string(),
            kdf_algorithm: "PBKDF2_HMAC_SHA512".to_string(),
        }
    }
}

/// Argon2id cost params plus their salt (both non-secret). Serializes flat to match
/// the spec's `vault-meta.json` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfBlock {
    #[serde(flatten)]
    pub params: KdfParams,
    pub salt_b64: String,
}

/// The Markdown-at-rest policy, recorded so every profile encrypts/decrypts the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownPolicy {
    pub encryption: MarkdownEncryption,
    /// How the Markdown subkey is derived from the master.
    pub subkey: String,
}

impl Default for MarkdownPolicy {
    fn default() -> Self {
        Self {
            encryption: MarkdownEncryption::None,
            subkey: MARKDOWN_SUBKEY_SCHEME.to_string(),
        }
    }
}

/// The on-disk `vault-meta.json`. Non-secret by construction — safe to sit in a
/// shared folder (spec §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultMeta {
    pub schema: u32,
    pub vault_id: String,
    pub created_at: String,
    pub app: String,
    pub key_mode: KeyMode,
    #[serde(default)]
    pub db_cipher: DbCipher,
    /// Present only for passphrase mode.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kdf: Option<KdfBlock>,
    /// Present only for passphrase mode.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub verifier: Option<Verifier>,
    #[serde(default)]
    pub markdown: MarkdownPolicy,
}

impl VaultMeta {
    /// Metadata for a fresh device-only vault (the default; first launch writes this).
    pub fn new_device() -> Self {
        Self {
            schema: META_SCHEMA,
            vault_id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            app: APP_ID.to_string(),
            key_mode: KeyMode::Device,
            db_cipher: DbCipher::default(),
            kdf: None,
            verifier: None,
            markdown: MarkdownPolicy::default(),
        }
    }
}

/// Path to a vault's metadata file inside its folder.
pub fn meta_path(vault_root: &Path) -> PathBuf {
    vault_root.join(META_FILENAME)
}

/// Read `vault-meta.json` if present. `None` means no metadata yet (a brand-new
/// vault folder); shipping before any user data exists, that's the only no-meta case.
pub fn load_meta(vault_root: &Path) -> Result<Option<VaultMeta>> {
    let path = meta_path(vault_root);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let meta: VaultMeta = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Other(format!("{META_FILENAME} is unreadable: {e}")))?;
            Ok(Some(meta))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write `vault-meta.json` atomically (temp file in the same dir, then rename), so a
/// crash mid-write can never leave a half-written metadata file.
pub fn store_meta(vault_root: &Path, meta: &VaultMeta) -> Result<()> {
    std::fs::create_dir_all(vault_root)?;
    let path = meta_path(vault_root);
    let json = serde_json::to_vec_pretty(meta)
        .map_err(|e| Error::Other(format!("could not encode {META_FILENAME}: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Ensure a vault folder has metadata, creating device-mode metadata on first run.
/// Idempotent: returns the existing meta untouched if present. This is what makes
/// every vault — even the zero-friction default — uniform from creation (spec §6).
pub fn ensure_device_meta(vault_root: &Path) -> Result<VaultMeta> {
    if let Some(meta) = load_meta(vault_root)? {
        return Ok(meta);
    }
    let meta = VaultMeta::new_device();
    store_meta(vault_root, &meta)?;
    Ok(meta)
}

/// The resolved on-disk locations of a vault, after consulting the per-profile
/// pointer. For the default (no pointer) these are exactly today's paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVault {
    /// The portable folder holding pm.sqlite + vault-meta.json + the Markdown.
    pub vault_root: PathBuf,
    /// The encrypted SQLite database.
    pub db_path: PathBuf,
    /// The Markdown vault (source of truth) — a subfolder of the root.
    pub markdown_dir: PathBuf,
}

/// Pure layout rule: where the DB, metadata, and Markdown sit given a profile data
/// dir and an optional pointer. Side-effect-free so it can be unit-tested.
pub fn resolve_layout(data_dir: &Path, pointer: Option<&VaultPointer>) -> ResolvedVault {
    let vault_root = pointer
        .map(|p| p.vault_root.clone())
        .unwrap_or_else(|| data_dir.to_path_buf());
    let db_path = vault_root.join("pm.sqlite");
    let markdown_dir = vault_root.join("vault");
    ResolvedVault {
        vault_root,
        db_path,
        markdown_dir,
    }
}

/// Resolve (and create) this profile's vault locations: read the pointer, fall back
/// to the default data dir, and ensure the root + Markdown folders exist. The vault
/// metadata lives at `vault_root/vault-meta.json` (next to the DB).
pub fn resolve(app: &AppHandle) -> Result<ResolvedVault> {
    let data_dir = paths::data_dir(app)?;
    let pointer = pointer::load(&data_dir)?;
    let resolved = resolve_layout(&data_dir, pointer.as_ref());
    std::fs::create_dir_all(&resolved.vault_root)?;
    std::fs::create_dir_all(&resolved.markdown_dir)?;
    Ok(resolved)
}

/// The 64-char lowercase-hex form of a master key, ready for `db::open`'s
/// `PRAGMA key = "x'…'"` path. Wrapped in `Secret` so it inherits zeroize + redaction.
pub fn db_key_hex(master: &[u8; KEY_LEN]) -> Secret {
    Secret::from(hex::encode(master))
}

/// Recover the raw 32-byte master from a keychain DB key (64 hex chars). For
/// device-mode vaults, whose master *is* the random keychain key — used when
/// deriving the Markdown subkey.
pub fn master_from_db_key_hex(hex_key: &str) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let bytes =
        hex::decode(hex_key).map_err(|e| Error::Other(format!("invalid DB key hex: {e}")))?;
    let arr: [u8; KEY_LEN] = bytes
        .try_into()
        .map_err(|_| Error::Other("DB key must be 32 bytes".into()))?;
    Ok(Zeroizing::new(arr))
}

/// Derive the master key for a passphrase (shareable) vault from its stored meta.
pub fn derive_master_from_passphrase(
    meta: &VaultMeta,
    passphrase: &str,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let block = meta
        .kdf
        .as_ref()
        .ok_or_else(|| Error::Other("vault is not in passphrase mode".into()))?;
    let salt = B64
        .decode(&block.salt_b64)
        .map_err(|e| Error::Other(format!("corrupt KDF salt: {e}")))?;
    kdf::derive_master(passphrase, &salt, &block.params)
}

/// Derive the Markdown-at-rest subkey from the master. Distinct from the DB key by a
/// dedicated BLAKE3 derive_key context (spec §3.3), so the two keys are independent.
pub fn markdown_subkey(master: &[u8; KEY_LEN]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake3::derive_key(MARKDOWN_SUBKEY_CONTEXT, master))
}

/// Build the metadata + master for a NEW shareable vault using already-chosen cost
/// params (the calibration is split out so tests can pass cheap params).
fn build_passphrase_meta(
    passphrase: &str,
    params: KdfParams,
) -> Result<(VaultMeta, Zeroizing<[u8; KEY_LEN]>)> {
    let mut salt = [0u8; kdf::SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    let master = kdf::derive_master(passphrase, &salt, &params)?;
    let verifier = verifier::build(&master)?;
    let meta = VaultMeta {
        schema: META_SCHEMA,
        vault_id: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        app: APP_ID.to_string(),
        key_mode: KeyMode::Passphrase,
        db_cipher: DbCipher::default(),
        kdf: Some(KdfBlock {
            params,
            salt_b64: B64.encode(salt),
        }),
        verifier: Some(verifier),
        // Markdown-at-rest encryption is layered on in Build 6; until then a freshly
        // created shareable vault keeps plaintext Markdown (spec build order: creation
        // precedes the encryption engine). Build 6 turns it on for shared vaults.
        markdown: MarkdownPolicy::default(),
    };
    Ok((meta, master))
}

/// Mint metadata for a NEW shareable (passphrase) vault: calibrate Argon2id to
/// ~`calibrate_target_ms`, pick a random salt, derive the master, build the
/// verifier, and require Markdown encryption. Returns the meta plus the freshly
/// derived master (so the caller can rekey the DB without deriving twice).
pub fn new_passphrase(
    passphrase: &str,
    calibrate_target_ms: u64,
) -> Result<(VaultMeta, Zeroizing<[u8; KEY_LEN]>)> {
    build_passphrase_meta(passphrase, kdf::calibrate(calibrate_target_ms))
}

/// Build the metadata + DB key for converting the current (device) vault into a
/// shareable, passphrase-derived one. Keeps the existing vault id + creation time —
/// it's the same vault, re-keyed — so its keychain cache and any future links stay
/// stable. The caller re-keys the open store with the returned key (PRAGMA rekey) and
/// writes the metadata.
pub fn prepare_shareable(old_meta: &VaultMeta, passphrase: &str) -> Result<(VaultMeta, Secret)> {
    let (mut meta, master) = new_passphrase(passphrase, CALIBRATE_TARGET_MS)?;
    meta.vault_id = old_meta.vault_id.clone();
    meta.created_at = old_meta.created_at.clone();
    Ok((meta, db_key_hex(&master)))
}

/// Open a passphrase (shareable) vault: derive the master, verify it against the
/// stored verifier (a clean "wrong passphrase" before SQLCipher's opaque error), then
/// open the store. Returns the connection plus the 64-hex DB key so the caller can
/// cache it in this profile's keychain. Keychain-free itself, so it unit-tests.
pub fn open_with_passphrase(
    resolved: &ResolvedVault,
    meta: &VaultMeta,
    passphrase: &str,
) -> Result<(Connection, Secret)> {
    let master = derive_master_from_passphrase(meta, passphrase)?;
    let verifier = meta
        .verifier
        .as_ref()
        .ok_or_else(|| Error::Other("vault metadata is missing its verifier".into()))?;
    if !verifier::check(verifier, &master)? {
        return Err(Error::Other("wrong passphrase".into()));
    }
    let key = db_key_hex(&master);
    let conn = db::open(&resolved.db_path, key.expose())?;
    Ok((conn, key))
}

/// Decide how to open a vault at boot from its metadata. Device vaults open with the
/// keychain key (today's path). Passphrase vaults open only if this profile has the
/// derived key cached; otherwise return `None` so the store stays locked and the UI
/// prompts for the passphrase.
pub fn open_at_boot(resolved: &ResolvedVault, meta: &VaultMeta) -> Result<Option<Connection>> {
    match meta.key_mode {
        KeyMode::Device => {
            let key = secrets::get_or_create_db_key()?;
            Ok(Some(db::open(&resolved.db_path, key.expose())?))
        }
        KeyMode::Passphrase => match secrets::get_cached_vault_key(&meta.vault_id)? {
            Some(cached) => Ok(Some(db::open(&resolved.db_path, cached.expose())?)),
            None => Ok(None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap Argon2id params so tests don't allocate hundreds of MiB.
    fn cheap_params() -> KdfParams {
        KdfParams {
            algorithm: "argon2id".to_string(),
            version: 0x13,
            m_cost_kib: 64,
            t_cost: 1,
            p_cost: 1,
            key_len: KEY_LEN as u32,
        }
    }

    #[test]
    fn device_meta_round_trips_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = ensure_device_meta(dir.path()).unwrap();
        assert_eq!(m1.key_mode, KeyMode::Device);
        assert!(m1.kdf.is_none() && m1.verifier.is_none());
        assert_eq!(m1.markdown.encryption, MarkdownEncryption::None);
        // A second call returns the SAME vault id — it must not overwrite.
        let m2 = ensure_device_meta(dir.path()).unwrap();
        assert_eq!(m1.vault_id, m2.vault_id);
        // ...and it really is valid JSON on disk.
        assert_eq!(load_meta(dir.path()).unwrap().unwrap(), m1);
    }

    #[test]
    fn passphrase_derives_a_stable_key_across_runs() {
        // The core promise: same passphrase + stored params/salt -> identical key,
        // so a second profile/machine derives the same 64-hex SQLCipher key.
        let pass = "correct horse battery staple";
        let (meta, master1) = build_passphrase_meta(pass, cheap_params()).unwrap();
        let master2 = derive_master_from_passphrase(&meta, pass).unwrap();
        assert_eq!(master1.as_slice(), master2.as_slice());
        assert_eq!(db_key_hex(&master1).expose(), db_key_hex(&master2).expose());
        assert_eq!(db_key_hex(&master1).expose().len(), 64);
    }

    #[test]
    fn wrong_passphrase_fails_the_verifier_before_any_db_open() {
        let (meta, _master) = build_passphrase_meta("right-passphrase", cheap_params()).unwrap();
        let v = meta.verifier.as_ref().unwrap();
        let good = derive_master_from_passphrase(&meta, "right-passphrase").unwrap();
        let bad = derive_master_from_passphrase(&meta, "wrong-passphrase").unwrap();
        assert!(verifier::check(v, &good).unwrap());
        assert!(!verifier::check(v, &bad).unwrap());
    }

    #[test]
    fn markdown_subkey_is_deterministic_and_differs_from_db_key() {
        let (_, master) = build_passphrase_meta("pw", cheap_params()).unwrap();
        let k1 = markdown_subkey(&master);
        let k2 = markdown_subkey(&master);
        assert_eq!(k1.as_slice(), k2.as_slice());
        // The subkey must NOT equal the raw master (which is the DB key material).
        assert_ne!(k1.as_slice(), master.as_slice());
    }

    #[test]
    fn passphrase_meta_is_non_secret_and_round_trips() {
        let pass = "ZZ-distinctive-secret-passphrase-QQ";
        let (meta, _) = build_passphrase_meta(pass, cheap_params()).unwrap();
        let json = serde_json::to_string_pretty(&meta).unwrap();
        let back: VaultMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, back);
        assert!(json.contains("argon2id"));
        assert!(json.contains("blake3-derive-key"));
        assert!(json.contains("\"key_mode\": \"passphrase\""));
        // The passphrase itself must never be written to the metadata file.
        assert!(
            !json.contains(pass),
            "passphrase must never appear in vault-meta.json"
        );
    }

    #[test]
    fn device_master_recovers_from_hex_and_subkey_works() {
        let hex_key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let master = master_from_db_key_hex(hex_key).unwrap();
        assert_eq!(db_key_hex(&master).expose(), hex_key);
        // A device vault can still derive a Markdown subkey from its keychain key.
        let sub = markdown_subkey(&master);
        assert_ne!(sub.as_slice(), master.as_slice());
    }

    #[test]
    fn calibrate_returns_usable_params() {
        let params = kdf::calibrate(1);
        assert_eq!(params.algorithm, "argon2id");
        assert_eq!(params.key_len, 32);
        let salt = [7u8; kdf::SALT_LEN];
        let k = kdf::derive_master("x", &salt, &params).unwrap();
        assert_eq!(k.len(), 32);
    }

    #[test]
    fn layout_defaults_to_the_data_dir_when_no_pointer() {
        let data = std::path::Path::new("/profile/data");
        let r = resolve_layout(data, None);
        assert_eq!(r.vault_root, data.to_path_buf());
        assert_eq!(r.db_path, data.join("pm.sqlite"));
        assert_eq!(r.markdown_dir, data.join("vault"));
    }

    #[test]
    fn layout_follows_the_pointer_when_present() {
        let shared = std::path::PathBuf::from("/shared/pm-vault");
        let ptr = pointer::VaultPointer::new(shared.clone());
        let r = resolve_layout(std::path::Path::new("/profile/data"), Some(&ptr));
        assert_eq!(r.vault_root, shared);
        assert_eq!(r.db_path, shared.join("pm.sqlite"));
        assert_eq!(r.markdown_dir, shared.join("vault"));
    }

    #[test]
    fn open_with_passphrase_round_trips_and_rejects_wrong_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = ResolvedVault {
            vault_root: dir.path().to_path_buf(),
            db_path: dir.path().join("pm.sqlite"),
            markdown_dir: dir.path().join("vault"),
        };
        let (meta, master) = build_passphrase_meta("open-sesame", cheap_params()).unwrap();
        store_meta(&resolved.vault_root, &meta).unwrap();
        // Create the encrypted store with the derived key (as creation/rekey would).
        {
            let key = db_key_hex(&master);
            crate::db::open(&resolved.db_path, key.expose()).unwrap();
        }
        // The right passphrase opens it and yields the same key.
        let (conn, key2) = open_with_passphrase(&resolved, &meta, "open-sesame").unwrap();
        assert_eq!(key2.expose(), db_key_hex(&master).expose());
        drop(conn);
        // The wrong passphrase fails at the verifier, before SQLCipher.
        assert!(open_with_passphrase(&resolved, &meta, "wrong").is_err());
    }
}
