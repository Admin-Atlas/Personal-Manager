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

pub mod acl;
pub mod crypto;
pub mod kdf;
pub mod lock;
pub mod migrate;
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

/// A fresh array of CSPRNG bytes (salts, etc.). Centralised so a zeroed
/// initializer never flows into a KDF/MAC as the live salt: the buffer is
/// overwritten by the OS RNG and handed back as a function result, so callers
/// receive randomness rather than a hard-coded literal.
pub(crate) fn random_array<const N: usize>() -> Result<[u8; N]> {
    // Build the buffer with `array::from_fn` rather than a `[0u8; N]` literal. The
    // bytes are immediately overwritten by the OS CSPRNG below; avoiding the constant
    // array literal also keeps static analysis from mistaking the zeroed placeholder
    // for the live salt (it does not model the in-place RNG fill as an overwrite).
    let mut buf: [u8; N] = std::array::from_fn(|_| 0);
    getrandom::fill(&mut buf).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    Ok(buf)
}

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

/// The at-rest filename suffix for an encrypted Markdown file. Plaintext files keep the
/// bare `.md`; encrypted ones gain `.pmenc`, so a shared folder visibly shows which
/// files are ciphertext (and editors won't try to render the binary as Markdown).
pub const ENCRYPTED_SUFFIX: &str = ".pmenc";

/// Policy-aware Markdown reader/writer for the active vault. Bundles the vault id (the
/// AAD binding), the at-rest encryption policy, and — when encryption is on — the
/// Markdown subkey (a BLAKE3 subkey of the master, never the DB key). One is built
/// whenever the store opens and kept in `AppState`, so every ingest and metadata
/// rewrite encrypts/decrypts the same way. Cloneable (the key clone is zeroized on
/// drop) so a command can snapshot it and do file IO without holding a lock.
#[derive(Clone)]
pub struct MarkdownCipher {
    vault_id: String,
    encryption: MarkdownEncryption,
    /// Present iff `encryption` is on; absent for plaintext (device) vaults.
    subkey: Option<Zeroizing<[u8; 32]>>,
}

impl MarkdownCipher {
    /// A plaintext cipher (no encryption) — the device-vault default.
    pub fn plaintext(vault_id: &str) -> Self {
        Self {
            vault_id: vault_id.to_string(),
            encryption: MarkdownEncryption::None,
            subkey: None,
        }
    }

    /// Build the cipher for a vault from its metadata + the resolved master key. The
    /// Markdown subkey is derived only when the policy calls for encryption, so a
    /// device vault never holds a Markdown key it won't use.
    pub fn from_meta(meta: &VaultMeta, master: &[u8; KEY_LEN]) -> Self {
        let subkey = match meta.markdown.encryption {
            MarkdownEncryption::None => None,
            MarkdownEncryption::XChaCha20Poly1305 => Some(markdown_subkey(master)),
        };
        Self {
            vault_id: meta.vault_id.clone(),
            encryption: meta.markdown.encryption,
            subkey,
        }
    }

    /// Whether this vault encrypts Markdown at rest.
    pub fn encryption_on(&self) -> bool {
        self.encryption != MarkdownEncryption::None
    }

    /// Whether two ciphers read and write a file identically (same vault id, policy, and
    /// key). Used by the migration converter to skip a file already in the exact target
    /// form — but never to skip a re-encode when the key changed (a passphrase change
    /// keeps the name but moves the subkey). These are our own keys, so plain equality is
    /// fine (no attacker-controlled timing oracle).
    pub(crate) fn same_key_as(&self, other: &Self) -> bool {
        self.vault_id == other.vault_id
            && self.encryption == other.encryption
            && match (&self.subkey, &other.subkey) {
                (Some(a), Some(b)) => a.as_slice() == b.as_slice(),
                (None, None) => true,
                _ => false,
            }
    }

    /// The on-disk filename for a logical `<name>.md`: bare under plaintext,
    /// `<name>.md.pmenc` under encryption.
    pub fn on_disk_name(&self, logical_md_name: &str) -> String {
        if self.encryption_on() {
            format!("{logical_md_name}{ENCRYPTED_SUFFIX}")
        } else {
            logical_md_name.to_string()
        }
    }

    /// The logical `<name>.md` for an on-disk filename (drops a trailing `.pmenc`).
    pub fn logical_name(on_disk_name: &str) -> String {
        on_disk_name
            .strip_suffix(ENCRYPTED_SUFFIX)
            .unwrap_or(on_disk_name)
            .to_string()
    }

    /// AAD stem binding a ciphertext to its logical file: the on-disk name without the
    /// `.pmenc` suffix, so encrypt and decrypt of the same file always agree while a
    /// file copied to another name or vault fails authentication.
    fn aad_stem(path: &Path) -> String {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::logical_name(&name)
    }

    /// Decode on-disk bytes to Markdown text, by magic: an encrypted container is
    /// decrypted (requires the subkey); anything else is treated as plaintext UTF-8.
    /// This is what tolerates a folder mid-migration (some plaintext, some ciphertext).
    pub fn decode(&self, bytes: &[u8], path: &Path) -> Result<String> {
        if crypto::is_encrypted(bytes) {
            let key = self.subkey.as_ref().ok_or_else(|| {
                Error::Other("this vault file is encrypted but no Markdown key is loaded".into())
            })?;
            let plain = crypto::decrypt(bytes, key, &self.vault_id, &Self::aad_stem(path))?;
            String::from_utf8(plain)
                .map_err(|_| Error::Other("decrypted Markdown was not valid UTF-8".into()))
        } else {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| Error::Other("Markdown file was not valid UTF-8".into()))
        }
    }

    /// Read + decode a Markdown file from disk (see [`decode`](Self::decode)).
    pub fn read(&self, path: &Path) -> Result<String> {
        let bytes = std::fs::read(path)?;
        self.decode(&bytes, path)
    }

    /// The raw on-disk bytes, undecoded — used to snapshot a file for rollback so a
    /// restore writes back exactly what was there (ciphertext stays ciphertext).
    pub fn read_raw(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(std::fs::read(path)?)
    }

    /// Encode Markdown text for writing to `path`, by policy: encrypted into a container
    /// (AAD-bound to the path's logical name) when encryption is on, else bytes as-is.
    pub fn encode_for(&self, path: &Path, content: &str) -> Result<Vec<u8>> {
        if self.encryption_on() {
            let key = self.subkey.as_ref().ok_or_else(|| {
                Error::Other("Markdown encryption is on but no key is loaded".into())
            })?;
            crypto::encrypt(
                content.as_bytes(),
                key,
                &self.vault_id,
                &Self::aad_stem(path),
            )
        } else {
            Ok(content.as_bytes().to_vec())
        }
    }

    /// Encode + write a Markdown file to `path` per policy.
    pub fn write_to(&self, path: &Path, content: &str) -> Result<()> {
        let bytes = self.encode_for(path, content)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

/// Build the metadata + master for a NEW shareable vault using already-chosen cost
/// params (the calibration is split out so tests can pass cheap params).
fn build_passphrase_meta(
    passphrase: &str,
    params: KdfParams,
) -> Result<(VaultMeta, Zeroizing<[u8; KEY_LEN]>)> {
    let salt: [u8; kdf::SALT_LEN] = random_array()?;
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
        // A shareable vault is, by definition, reachable from other accounts on the
        // machine, so folder isolation no longer protects the Markdown — encryption at
        // rest is mandatory (spec §3). Recorded here so every profile that opens the
        // vault agrees to encrypt/decrypt with the same subkey scheme.
        markdown: MarkdownPolicy {
            encryption: MarkdownEncryption::XChaCha20Poly1305,
            subkey: MARKDOWN_SUBKEY_SCHEME.to_string(),
        },
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
/// prompts for the passphrase. On success also returns the policy-aware Markdown
/// cipher, so the caller can serve the active session's ingest/rewrite IO.
pub fn open_at_boot(
    resolved: &ResolvedVault,
    meta: &VaultMeta,
) -> Result<Option<(Connection, MarkdownCipher)>> {
    let key = match meta.key_mode {
        KeyMode::Device => secrets::get_or_create_db_key()?,
        KeyMode::Passphrase => match secrets::get_cached_vault_key(&meta.vault_id)? {
            Some(cached) => cached,
            None => return Ok(None),
        },
    };
    let conn = db::open(&resolved.db_path, key.expose())?;
    let master = master_from_db_key_hex(key.expose())?;
    let cipher = MarkdownCipher::from_meta(meta, &master);
    Ok(Some((conn, cipher)))
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
        // A shareable vault must declare Markdown-at-rest encryption (spec §3).
        assert_eq!(
            meta.markdown.encryption,
            MarkdownEncryption::XChaCha20Poly1305
        );
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
        let salt: [u8; kdf::SALT_LEN] = super::random_array().unwrap();
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

    /// An encrypted cipher with a fixed subkey (no KDF needed for the file-IO tests).
    fn enc_cipher() -> MarkdownCipher {
        MarkdownCipher {
            vault_id: "vault-1".to_string(),
            encryption: MarkdownEncryption::XChaCha20Poly1305,
            subkey: Some(Zeroizing::new([3u8; 32])),
        }
    }

    #[test]
    fn cipher_from_meta_derives_key_only_when_encryption_on() {
        let (mut meta, master) = build_passphrase_meta("pw", cheap_params()).unwrap();
        meta.markdown.encryption = MarkdownEncryption::None;
        let off = MarkdownCipher::from_meta(&meta, &master);
        assert!(!off.encryption_on() && off.subkey.is_none());
        meta.markdown.encryption = MarkdownEncryption::XChaCha20Poly1305;
        let on = MarkdownCipher::from_meta(&meta, &master);
        assert!(on.encryption_on() && on.subkey.is_some());
    }

    #[test]
    fn encrypted_cipher_round_trips_and_writes_ciphertext_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let c = enc_cipher();
        assert!(c.encryption_on());
        assert_eq!(c.on_disk_name("note-abc.md"), "note-abc.md.pmenc");

        let path = dir.path().join(c.on_disk_name("note-abc.md"));
        c.write_to(&path, "# Secret\nbody text").unwrap();

        let raw = std::fs::read(&path).unwrap();
        assert!(
            crypto::is_encrypted(&raw),
            "on-disk file must be a container"
        );
        assert!(
            !raw.windows(6).any(|w| w == b"Secret"),
            "plaintext must not be present on disk"
        );
        assert_eq!(c.read(&path).unwrap(), "# Secret\nbody text");
        assert_eq!(
            MarkdownCipher::logical_name("note-abc.md.pmenc"),
            "note-abc.md"
        );
    }

    #[test]
    fn plaintext_cipher_writes_and_reads_raw() {
        let dir = tempfile::tempdir().unwrap();
        let c = MarkdownCipher::plaintext("vault-1");
        assert!(!c.encryption_on());
        assert_eq!(c.on_disk_name("note.md"), "note.md");

        let path = dir.path().join("note.md");
        c.write_to(&path, "plain body").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "plain body");
        assert_eq!(c.read(&path).unwrap(), "plain body");
    }

    #[test]
    fn encrypted_cipher_still_reads_a_leftover_plaintext_file() {
        // Mixed folder mid-migration: an encrypted-policy cipher must still read a
        // plaintext file that hasn't been converted yet (read-by-magic).
        let dir = tempfile::tempdir().unwrap();
        let c = enc_cipher();
        let path = dir.path().join("legacy.md");
        std::fs::write(&path, "still plaintext").unwrap();
        assert_eq!(c.read(&path).unwrap(), "still plaintext");
    }

    #[test]
    fn export_plaintext_decrypts_the_whole_vault() {
        // The escape hatch: an encrypted vault exports to a clean plaintext `.md` tree,
        // proving the user is never locked in.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let c = enc_cipher();
        c.write_to(&vault.join(c.on_disk_name("a.md")), "# A\nalpha")
            .unwrap();
        c.write_to(&vault.join(c.on_disk_name("b.md")), "# B\nbeta")
            .unwrap();
        // A stray non-Markdown file is ignored by the export.
        std::fs::write(vault.join("notes.txt"), "ignore me").unwrap();

        let dest = dir.path().join("export");
        let n = crate::ingest::export_plaintext(&vault, &c, &dest).unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            std::fs::read_to_string(dest.join("a.md")).unwrap(),
            "# A\nalpha"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("b.md")).unwrap(),
            "# B\nbeta"
        );
        assert!(!dest.join("a.md.pmenc").exists(), "no ciphertext suffix");
        assert!(!dest.join("notes.txt").exists(), "non-markdown skipped");
    }

    #[test]
    fn convert_markdown_encrypts_in_place_and_updates_vault_path() {
        // The device → shareable transition's Markdown half: a plaintext file becomes
        // a renamed ciphertext file and its DB row follows.
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), key).unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('a.md', 'A', 'h')",
            [],
        )
        .unwrap();

        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join("a.md"), "# A\nplaintext body").unwrap();

        let c = enc_cipher();
        assert_eq!(
            crate::ingest::convert_markdown(&conn, &vault, &c, &c).unwrap(),
            1
        );

        assert!(!vault.join("a.md").exists(), "old plaintext file removed");
        let enc = vault.join("a.md.pmenc");
        assert!(crypto::is_encrypted(&std::fs::read(&enc).unwrap()));
        let stored: String = conn
            .query_row(
                "SELECT vault_path FROM documents WHERE title='A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "a.md.pmenc", "vault_path follows the rename");
        assert_eq!(c.read(&enc).unwrap(), "# A\nplaintext body");

        // Idempotent: a second pass over the now-encrypted folder changes nothing.
        assert_eq!(
            crate::ingest::convert_markdown(&conn, &vault, &c, &c).unwrap(),
            0
        );
    }
}
