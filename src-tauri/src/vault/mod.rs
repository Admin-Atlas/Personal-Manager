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

pub mod access;
pub mod acl;
pub mod advert;
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
/// BLAKE3 derive_key context for the vault-meta authentication subkey (M-3) — distinct from the DB
/// key, the Markdown subkey, and the verifier, so the MAC key is independent of every other use.
const META_MAC_CONTEXT: &str = "org.itsatlas.pm vault-meta auth subkey v1";
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
    /// Keyed BLAKE3 MAC (hex) over the authenticated meta fields, under a master subkey (M-3).
    /// Absent on a legacy vault written before meta authentication; stamped on the first authenticated
    /// open and enforced thereafter. `skip_serializing_if`/`default` keep older files loading and keep
    /// the MAC out of its own input.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub meta_mac: Option<String>,
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
            // Stamped lazily on the first authenticated open (the device key isn't created yet here).
            meta_mac: None,
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

/// What boot should do about vault metadata, decided by [`boot_meta_decision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootMeta {
    /// Metadata found — open this vault (pointered or default alike). Boxed so the loaded-meta
    /// variant (much larger than the two string/unit variants) doesn't bloat the whole enum.
    UseExisting(Box<VaultMeta>),
    /// Default location, no metadata: a genuinely fresh profile — create the
    /// zero-friction device vault, exactly as before.
    CreateDeviceDefault,
    /// A pointer names a folder that has no readable vault (deleted, made private, or
    /// this account's access was revoked). Boot LOCKED with the carried detail instead
    /// of silently creating a fresh empty vault inside someone else's folder — the
    /// failure that made a joined-then-broken vault look like "all my data vanished".
    PointedVaultMissing(String),
}

/// Decide how boot treats the (possibly absent) vault metadata. Pure: takes whether a
/// pointer redirects this profile and the outcome of loading the pointed root's meta
/// (`Err` = the load itself failed, e.g. access denied — stringly so the decision
/// stays testable without constructing real I/O errors). A missing/unreadable meta is
/// only auto-created at the DEFAULT location; behind a pointer it is a reportable
/// fault. A meta-load failure at the default location stays fatal (`Err`), as before.
pub fn boot_meta_decision(
    pointer_present: bool,
    meta: std::result::Result<Option<VaultMeta>, String>,
) -> std::result::Result<BootMeta, String> {
    match (pointer_present, meta) {
        (_, Ok(Some(m))) => Ok(BootMeta::UseExisting(Box::new(m))),
        (false, Ok(None)) => Ok(BootMeta::CreateDeviceDefault),
        (false, Err(e)) => Err(e),
        (true, Ok(None)) => Ok(BootMeta::PointedVaultMissing(
            "the folder doesn't contain a PM vault any more".into(),
        )),
        (true, Err(e)) => Ok(BootMeta::PointedVaultMissing(e)),
    }
}

/// What [`authenticate_meta`] did on open — surfaced so a caller can show a non-blocking warning (M-3).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MetaAuthReport {
    /// A passphrase vault whose Markdown policy had been downgraded to plaintext was forced back to
    /// encrypted (and the on-disk meta repaired).
    pub downgrade_corrected: bool,
    /// The stored meta MAC did not match — the metadata was altered outside PM.
    pub mac_mismatch: bool,
}

impl MetaAuthReport {
    /// Whether anything happened that the user should be told about (a legacy stamp is silent).
    pub fn needs_warning(&self) -> bool {
        self.downgrade_corrected || self.mac_mismatch
    }

    /// A short, user-facing warning line, or `None` when there is nothing to report.
    pub fn warning(&self) -> Option<String> {
        if self.downgrade_corrected {
            Some(
                "This vault's \"encrypt notes at rest\" setting had been switched off outside PM. \
                 PM has turned it back on, so your notes stay encrypted. If you didn't change it, \
                 check who can reach the vault folder."
                    .into(),
            )
        } else if self.mac_mismatch {
            Some(
                "This vault's metadata failed its integrity check — it was altered outside PM. \
                 PM has re-secured it. If you didn't change it, check who can reach the vault folder."
                    .into(),
            )
        } else {
            None
        }
    }
}

/// The keyed BLAKE3 MAC over the authenticated meta fields (everything EXCEPT the MAC itself), under a
/// dedicated subkey of the master. Deterministic: serde serializes struct fields in declaration order,
/// and the MAC field is cleared before serialization so it never covers itself.
fn meta_mac(meta: &VaultMeta, master: &[u8; KEY_LEN]) -> Result<blake3::Hash> {
    let mut bare = meta.clone();
    bare.meta_mac = None;
    let bytes = serde_json::to_vec(&bare).map_err(|e| {
        Error::Other(format!(
            "could not encode vault meta for authentication: {e}"
        ))
    })?;
    let subkey = blake3::derive_key(META_MAC_CONTEXT, master);
    Ok(blake3::keyed_hash(&subkey, &bytes))
}

/// Authenticate the non-secret `vault-meta.json` against a keyed MAC under a master subkey, and repair a
/// silently-downgraded Markdown policy (M-3). `master` MUST already be authenticated by the caller (the
/// DB opened, or the passphrase verifier passed) — this is the first point where that trust exists.
///
/// Additive + backward-compatible: a legacy vault with no stored MAC is stamped on this first
/// authenticated open, and enforced thereafter. On any repair the corrected meta is persisted so the
/// on-disk file is durably safe. Never hard-fails — the master is trusted, so we correct-and-continue
/// rather than lock the user out of their own data. The returned report drives a non-blocking warning.
pub fn authenticate_meta(
    vault_root: &Path,
    meta: &VaultMeta,
    master: &[u8; KEY_LEN],
) -> Result<MetaAuthReport> {
    let mut report = MetaAuthReport::default();
    let legacy = meta.meta_mac.is_none();
    if let Some(stored_hex) = &meta.meta_mac {
        let stored = blake3::Hash::from_hex(stored_hex.as_str())
            .map_err(|e| Error::Other(format!("corrupt vault-meta authentication tag: {e}")))?;
        // blake3::Hash equality is constant-time.
        if meta_mac(meta, master)? != stored {
            report.mac_mismatch = true;
        }
    }
    // Repair a copy we can persist; the live cipher is already forced safe by `from_meta`.
    let mut fixed = meta.clone();
    if fixed.key_mode == KeyMode::Passphrase
        && fixed.markdown.encryption == MarkdownEncryption::None
    {
        fixed.markdown.encryption = MarkdownEncryption::XChaCha20Poly1305;
        fixed.markdown.subkey = MARKDOWN_SUBKEY_SCHEME.to_string();
        report.downgrade_corrected = true;
    }
    // Stamp a legacy vault, or re-secure after a downgrade / tamper. A clean, already-stamped vault is
    // a pure verify with no write.
    if legacy || report.downgrade_corrected || report.mac_mismatch {
        fixed.meta_mac = Some(meta_mac(&fixed, master)?.to_hex().to_string());
        store_meta(vault_root, &fixed)?;
    }
    Ok(report)
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

    /// An encrypting cipher with a fixed subkey (no KDF) — for file-IO tests in sibling modules that
    /// need to prove their writes round-trip under encryption (the crypto itself is covered here).
    #[cfg(test)]
    pub(crate) fn for_test_encrypted(vault_id: &str) -> Self {
        Self {
            vault_id: vault_id.to_string(),
            encryption: MarkdownEncryption::XChaCha20Poly1305,
            subkey: Some(Zeroizing::new([7u8; 32])),
        }
    }

    /// Build the cipher for a vault from its metadata + the resolved master key. The
    /// Markdown subkey is derived only when the policy calls for encryption, so a
    /// device vault never holds a Markdown key it won't use.
    pub fn from_meta(meta: &VaultMeta, master: &[u8; KEY_LEN]) -> Self {
        // M-3: a passphrase (shareable) vault MUST encrypt Markdown at rest — its folder is reachable
        // by other accounts, so plaintext there is a confidentiality downgrade. Enforce the invariant at
        // the point of use, regardless of what `vault-meta.json` claims, so a tampered `encryption:none`
        // can never turn new notes into cleartext — even on the same open that repairs the meta.
        let encryption = match meta.key_mode {
            KeyMode::Passphrase => MarkdownEncryption::XChaCha20Poly1305,
            KeyMode::Device => meta.markdown.encryption,
        };
        let subkey = match encryption {
            MarkdownEncryption::None => None,
            MarkdownEncryption::XChaCha20Poly1305 => Some(markdown_subkey(master)),
        };
        Self {
            vault_id: meta.vault_id.clone(),
            encryption,
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

    /// Read + decrypt a file's bytes from disk (the byte analogue of [`read`](Self::read), the byte
    /// counterpart to [`write_bytes_to`](Self::write_bytes_to)): an encrypted container is decrypted to
    /// its plaintext bytes; anything else is returned as-is. Used to serve an opt-in saved photo original
    /// back to the reader regardless of the vault's cipher policy.
    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        let bytes = std::fs::read(path)?;
        if crypto::is_encrypted(&bytes) {
            let key = self.subkey.as_ref().ok_or_else(|| {
                Error::Other("this vault file is encrypted but no Markdown key is loaded".into())
            })?;
            crypto::decrypt(&bytes, key, &self.vault_id, &Self::aad_stem(path))
        } else {
            Ok(bytes)
        }
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

    /// Encode arbitrary bytes for writing to `path` by the SAME policy as Markdown — encrypted into a
    /// container (AAD-bound to the path's logical name) when encryption is on, else the bytes as-is.
    /// Lets an opt-in saved photo original follow the vault's cipher instead of leaking plaintext at
    /// rest in an otherwise-encrypted vault.
    pub fn encode_bytes_for(&self, path: &Path, bytes: &[u8]) -> Result<Vec<u8>> {
        if self.encryption_on() {
            let key = self.subkey.as_ref().ok_or_else(|| {
                Error::Other("Markdown encryption is on but no key is loaded".into())
            })?;
            crypto::encrypt(bytes, key, &self.vault_id, &Self::aad_stem(path))
        } else {
            Ok(bytes.to_vec())
        }
    }

    /// Encode + write arbitrary bytes to `path` per policy (see [`encode_bytes_for`](Self::encode_bytes_for)).
    pub fn write_bytes_to(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let out = self.encode_bytes_for(path, bytes)?;
        std::fs::write(path, out)?;
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
        // Stamped on the first authenticated open (uniform with the device path).
        meta_mac: None,
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
) -> Result<(Connection, Secret, MetaAuthReport)> {
    let master = derive_master_from_passphrase(meta, passphrase)?;
    let verifier = meta
        .verifier
        .as_ref()
        .ok_or_else(|| Error::Other("vault metadata is missing its verifier".into()))?;
    if !verifier::check(verifier, &master)? {
        return Err(Error::Other("wrong passphrase".into()));
    }
    // The passphrase is now proven, so the master is trusted — the first point at which we can
    // authenticate + repair the non-secret meta (M-3). The report drives a non-blocking UI warning.
    let report = authenticate_meta(&resolved.vault_root, meta, &master)?;
    let key = db_key_hex(&master);
    let conn = db::open(&resolved.db_path, key.expose())?;
    Ok((conn, key, report))
}

/// The 64-hex DB key this profile should open a vault with, or `None` if the vault is a
/// passphrase vault this profile hasn't unlocked yet (so it stays locked, pending a
/// passphrase prompt). The per-vault keychain cache (`vault_key::<vault_id>`) is
/// consulted FIRST for both key modes: a **restored or relocated** vault seeds its own
/// (source) key there, and its master must be reused exactly — the DB key *is* the
/// master, and the Markdown / rules / manifest ciphers are all subkeys of it, so a
/// device vault opened here must never fall back to this machine's global device key.
/// A normal local device vault has no such cache entry and takes the global key, exactly
/// as before (backward compatible).
pub fn current_db_key(meta: &VaultMeta) -> Result<Option<Secret>> {
    Ok(
        match (
            meta.key_mode,
            secrets::get_cached_vault_key(&meta.vault_id)?,
        ) {
            (_, Some(cached)) => Some(cached),
            (KeyMode::Device, None) => Some(secrets::get_or_create_db_key()?),
            (KeyMode::Passphrase, None) => None,
        },
    )
}

/// Decide how to open a vault at boot from its metadata, via [`current_db_key`]. On
/// success also returns the resolved 32-byte master, from which the caller builds the
/// session runtime (the Markdown cipher *and* the always-on rules cipher are both
/// subkeys of it).
pub fn open_at_boot(
    resolved: &ResolvedVault,
    meta: &VaultMeta,
) -> Result<Option<(Connection, Zeroizing<[u8; KEY_LEN]>)>> {
    let key = match current_db_key(meta)? {
        Some(key) => key,
        None => return Ok(None),
    };
    let conn = match db::open(&resolved.db_path, key.expose()) {
        Ok(conn) => conn,
        Err(e) => {
            // A Passphrase vault whose CACHED key fails to open the store has two very different
            // causes, told apart by the meta verifier (which authenticates the master — and the
            // cached key IS the master hex): if the verifier REJECTS the cached master, the key is
            // stale — typically the owner changed the passphrase from another profile — so drop the
            // cache and report "locked" (Ok(None)); the UI then prompts for the new passphrase
            // instead of the open-error surface (whose "start fresh" would wrongly offer to delete a
            // vault that just needs re-typing). If the verifier ACCEPTS the cached master, the key is
            // correct and the store is genuinely corrupt — fall through so the honest error reaches
            // the recovery surface (Retry / Start fresh for a local vault, Detach for a shared one).
            if meta.key_mode == KeyMode::Passphrase
                && e.to_string().contains(db::WRONG_KEY_OR_CORRUPT_MSG)
                && secrets::get_cached_vault_key(&meta.vault_id)?.is_some()
            {
                let cached_master = master_from_db_key_hex(key.expose())?;
                let key_is_stale = match meta.verifier.as_ref() {
                    Some(v) => !verifier::check(v, &cached_master)?,
                    None => true, // no verifier to trust ⇒ treat as stale (prompt) rather than brick
                };
                if key_is_stale {
                    secrets::clear_cached_vault_key(&meta.vault_id)?;
                    return Ok(None);
                }
            }
            // A Device-mode vault whose *cached* key fails to open may be a half-finished
            // make-private (B1-3): the store was re-keyed to the random device key and the meta
            // flipped to Device, but a crash before `update_keychain` left the old
            // passphrase-derived key in the per-vault cache — which `current_db_key` prefers.
            // Retry once with the freshly derived device key; if that opens, the cache was
            // stale, so clear it and continue on the clean device path. Any other failure (a
            // genuine wrong key / corruption / transient file lock) propagates unchanged.
            if meta.key_mode == KeyMode::Device
                && secrets::get_cached_vault_key(&meta.vault_id)?.is_some()
            {
                let device_key = secrets::get_or_create_db_key()?;
                if let Ok(conn) = db::open(&resolved.db_path, device_key.expose()) {
                    secrets::clear_cached_vault_key(&meta.vault_id)?;
                    let master = master_from_db_key_hex(device_key.expose())?;
                    log_meta_auth(&resolved.vault_root, meta, &master);
                    return Ok(Some((conn, master)));
                }
            }
            // No recovery applied, or the device key didn't open it either — the original
            // error is the honest one to report (the cache wasn't the problem).
            return Err(e);
        }
    };
    let master = master_from_db_key_hex(key.expose())?;
    log_meta_auth(&resolved.vault_root, meta, &master);
    Ok(Some((conn, master)))
}

/// Authenticate + repair the meta on the boot/reopen path, logging a warning (there is no UI at boot).
/// Best-effort: an authentication or persist failure must not abort opening a vault the master already
/// unlocked, so it is logged rather than propagated (the live cipher is safe regardless, via `from_meta`).
fn log_meta_auth(vault_root: &Path, meta: &VaultMeta, master: &[u8; KEY_LEN]) {
    match authenticate_meta(vault_root, meta, master) {
        Ok(report) => {
            if let Some(w) = report.warning() {
                eprintln!("vault: {w}");
            }
        }
        Err(e) => eprintln!("vault: meta authentication could not complete: {e}"),
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
    fn boot_meta_decision_never_creates_a_vault_behind_a_pointer() {
        // The regression pin for the joiner-boot fix: metadata present opens it, a fresh
        // default profile creates the device vault, but a POINTERED root with missing or
        // unreadable metadata must boot locked-with-detail — never mint a fresh vault
        // inside the pointed (shared) folder.
        let meta = VaultMeta::new_device();
        assert_eq!(
            boot_meta_decision(false, Ok(Some(meta.clone()))),
            Ok(BootMeta::UseExisting(Box::new(meta.clone())))
        );
        assert_eq!(
            boot_meta_decision(true, Ok(Some(meta.clone()))),
            Ok(BootMeta::UseExisting(Box::new(meta)))
        );
        assert_eq!(
            boot_meta_decision(false, Ok(None)),
            Ok(BootMeta::CreateDeviceDefault)
        );
        assert!(matches!(
            boot_meta_decision(true, Ok(None)),
            Ok(BootMeta::PointedVaultMissing(_))
        ));
        assert_eq!(
            boot_meta_decision(true, Err("access is denied".into())),
            Ok(BootMeta::PointedVaultMissing("access is denied".into()))
        );
        // At the default location a meta-load failure stays fatal, as before.
        assert_eq!(
            boot_meta_decision(false, Err("disk error".into())),
            Err("disk error".into())
        );
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
        let (conn, key2, _report) = open_with_passphrase(&resolved, &meta, "open-sesame").unwrap();
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
        // Use a DEVICE meta for the off case: a passphrase vault is now always forced on (M-3), so
        // only a device vault honours an `encryption: none` policy.
        let master = [5u8; KEY_LEN];
        let mut meta = VaultMeta::new_device();
        meta.markdown.encryption = MarkdownEncryption::None;
        let off = MarkdownCipher::from_meta(&meta, &master);
        assert!(!off.encryption_on() && off.subkey.is_none());
        meta.markdown.encryption = MarkdownEncryption::XChaCha20Poly1305;
        let on = MarkdownCipher::from_meta(&meta, &master);
        assert!(on.encryption_on() && on.subkey.is_some());
    }

    #[test]
    fn from_meta_forces_encryption_for_a_passphrase_vault_even_if_meta_says_none() {
        // M-3: a downgraded/tampered passphrase meta claiming plaintext must still encrypt at rest.
        let (mut meta, master) = build_passphrase_meta("pw", cheap_params()).unwrap();
        meta.markdown.encryption = MarkdownEncryption::None;
        let cipher = MarkdownCipher::from_meta(&meta, &master);
        assert!(cipher.encryption_on());
        assert!(cipher.subkey.is_some());
    }

    #[test]
    fn authenticate_meta_stamps_legacy_then_enforces_and_repairs() {
        let dir = tempfile::tempdir().unwrap();
        let (meta, master) = build_passphrase_meta("pw", cheap_params()).unwrap();
        store_meta(dir.path(), &meta).unwrap();
        assert!(meta.meta_mac.is_none(), "a fresh vault carries no MAC yet");

        // First authenticated open: a legacy (unstamped) vault gets its MAC written, silently.
        let r1 = authenticate_meta(dir.path(), &meta, &master).unwrap();
        assert!(!r1.needs_warning());
        let stamped = load_meta(dir.path()).unwrap().unwrap();
        assert!(
            stamped.meta_mac.is_some(),
            "legacy vault stamped on first open"
        );

        // A clean, already-stamped vault verifies with no repair and no warning.
        let r2 = authenticate_meta(dir.path(), &stamped, &master).unwrap();
        assert!(!r2.needs_warning());

        // Tamper the on-disk meta: flip Markdown encryption off, leaving the now-stale MAC in place.
        let mut downgraded = stamped.clone();
        downgraded.markdown.encryption = MarkdownEncryption::None;
        let r3 = authenticate_meta(dir.path(), &downgraded, &master).unwrap();
        assert!(r3.downgrade_corrected, "the plaintext downgrade is caught");
        assert!(
            r3.mac_mismatch,
            "the stale MAC no longer matches the tampered fields"
        );

        // The persisted meta is re-secured: encryption forced back on and a fresh, valid MAC.
        let repaired = load_meta(dir.path()).unwrap().unwrap();
        assert_eq!(
            repaired.markdown.encryption,
            MarkdownEncryption::XChaCha20Poly1305
        );
        let r4 = authenticate_meta(dir.path(), &repaired, &master).unwrap();
        assert!(!r4.needs_warning(), "the re-secured meta verifies cleanly");
    }

    #[test]
    fn meta_mac_rejects_a_wrong_master() {
        let (meta, master) = build_passphrase_meta("pw", cheap_params()).unwrap();
        let other = [0xabu8; KEY_LEN];
        assert_ne!(
            meta_mac(&meta, &master).unwrap(),
            meta_mac(&meta, &other).unwrap()
        );
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
