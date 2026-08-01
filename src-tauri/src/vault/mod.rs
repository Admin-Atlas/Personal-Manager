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
pub mod preflight;
pub mod verifier;

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use zeroize::Zeroizing;

use crate::db;
use crate::error::{io_at, Error, Result, VaultFault, VaultFaultCode};
use crate::paths;
use crate::secret::Secret;
use crate::secrets;
use kdf::{KdfParams, KEY_LEN};
use pointer::VaultPointer;
use verifier::Verifier;

/// Filename of the per-vault, non-secret metadata, stored inside the vault folder.
pub const META_FILENAME: &str = "vault-meta.json";
/// Filename of the encrypted SQLCipher store inside the vault folder. [`resolve_layout`] is the
/// layout rule; this const exists so the backup, export, wipe, preflight and migration paths that
/// must name the file directly can agree with it instead of retyping the string.
///
/// It does NOT make a rename mechanical. The name also appears in user-facing copy
/// ([`crate::commands::join_shared_vault`]'s "no vault here" message and `src/components/VaultJoin.tsx`,
/// which cannot see this const at all), in `AGENTS.md` / `README.md` prose, and — via
/// [`crate::backup::pack`] — inside every `.pmbackup` and export zip written so far. A rename is a
/// checklist covering all of those plus a `manifest::SCHEMA` decision, not a one-line edit here.
pub const DB_FILENAME: &str = "pm.sqlite";
/// The Markdown subfolder inside the vault folder — the source of truth that sits beside
/// [`DB_FILENAME`]. Named `*_DIRNAME` deliberately: [`crate::ingest`]'s `MARKDOWN_SUBDIRS` is a
/// different concept (the allow-list of subdirectories *inside* this one), and the two have been
/// confused before.
pub const MARKDOWN_DIRNAME: &str = "vault";
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

/// A recorded, confirmed change of a shared vault's owner: who held it, who took it, and when.
///
/// Written only by an explicit takeover ([`OwnerOnRekey::Claim`]) — a plain re-key carries its owner
/// forward and leaves this field alone. It rides in `vault-meta.json` rather than a sidecar so it is
/// MAC-covered like every other field, which is the whole point: an ownership record that the taker
/// can quietly erase is worth nothing, and erasing this one trips the "altered outside PM" warning.
/// Both SIDs are optional because either can be genuinely unknown — no owner was ever recorded, or
/// the claimant's SID lookup failed at the moment of the claim (in which case the vault reads
/// `Unknown` afterwards, and the record is the only trace that a takeover happened at all).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipTransfer {
    /// The owner SID this vault carried before the takeover; `None` if none was recorded.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from_sid: Option<String>,
    /// The owner SID stamped by the takeover; `None` if the lookup failed as it was stamped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub to_sid: Option<String>,
    /// RFC3339, UTC. The UI formats it (DD-MM-YYYY).
    pub at: String,
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
    /// The OS SID of the account that CREATED this shareable vault (Windows-only; `None` for a device
    /// vault, a non-Windows vault, or a legacy shared vault created before owner identity existed).
    /// Non-secret (the discovery advert already publishes the owner's username) and, being an ordinary
    /// field, MAC-covered like every other — so it is tamper-evident. Gates connector setup to the
    /// vault owner: OAuth tokens live in the per-account keychain, so a joiner can't sync them anyway.
    /// `skip_serializing_if`/`default` keep the field ABSENT for existing vaults, so their stored MAC
    /// (computed over the serialized fields) still verifies unchanged.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub owner_sid: Option<String>,
    /// The last CONFIRMED ownership takeover, when one happened — see [`OwnershipTransfer`]. Absent on
    /// every vault that has never had one, and absence costs zero serialized bytes
    /// (`skip_serializing_if`), so every existing vault's stored MAC still verifies untouched and an
    /// older build reads the file exactly as it did before this field existed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ownership_transfer: Option<OwnershipTransfer>,
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
            owner_sid: None,          // a device vault has no sharing owner
            ownership_transfer: None, // ...so there is no ownership to have transferred either
            // Stamped lazily on the first authenticated open (the device key isn't created yet here).
            meta_mac: None,
        }
    }
}

/// Whether the current OS account owns this vault. A device vault or a legacy shared vault (no
/// `owner_sid` recorded) has no ownership restriction → `true`. A shared vault stamped with an owner
/// SID is owned only by the account whose SID matches. Ownership is Windows-only (shared vaults are
/// Windows-only); elsewhere, and on any SID-resolution failure, this returns `true` — fail open, so a
/// hiccup never locks the real owner out of their own connectors.
pub fn is_vault_owner(meta: &VaultMeta) -> bool {
    is_owner_given(&meta.owner_sid, current_user_sid_opt().as_deref())
}

/// The pure ownership decision, extracted so it is unit-testable without a live SID lookup.
fn is_owner_given(owner_sid: &Option<String>, current: Option<&str>) -> bool {
    match owner_sid {
        None => true,
        Some(owner) => current.map(|c| c == owner).unwrap_or(true),
    }
}

/// The four realities [`is_vault_owner`] folds into one `bool`.
///
/// That fold is right for its own job — gating connector setup, where failing open means a SID
/// hiccup never locks the real owner out. It is wrong for anything destructive, because "nobody
/// recorded an owner" and "this account is the owner" are the same `true` there, and a delete needs
/// to tell them apart. One rule, so the badge the user reads, the button PM offers and the erase's
/// own decision can never disagree about who owns a vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultOwnership {
    /// A device vault: its key lives in this account's keychain alone, so no other account can open
    /// it wherever it sits. Ours by construction.
    Device,
    /// A shareable vault stamped with this account's SID — we created it.
    Owned,
    /// A shareable vault stamped with a DIFFERENT account's SID — someone else created it and we
    /// joined. Theirs.
    Joined,
    /// A shareable vault with no recorded owner (created before ownership existed), or one whose
    /// SID could not be resolved — which is every shareable vault off Windows, since shared vaults
    /// and SIDs are both Windows-only. Never treated as ours.
    Unknown,
}

/// The pure ownership rule, so it unit-tests without a live SID lookup — and so macOS and Linux,
/// where `current` is always `None`, are covered by the same tests as Windows.
pub fn ownership_given(
    key_mode: KeyMode,
    owner_sid: Option<&str>,
    current: Option<&str>,
) -> VaultOwnership {
    if key_mode == KeyMode::Device {
        return VaultOwnership::Device;
    }
    match (owner_sid, current) {
        (Some(owner), Some(me)) if owner == me => VaultOwnership::Owned,
        (Some(_), Some(_)) => VaultOwnership::Joined,
        // No stamp to compare, or no SID to compare it against. Not provably ours.
        _ => VaultOwnership::Unknown,
    }
}

/// [`ownership_given`] against this vault's metadata and the current OS account.
pub fn vault_ownership(meta: &VaultMeta) -> VaultOwnership {
    ownership_given(
        meta.key_mode,
        meta.owner_sid.as_deref(),
        current_user_sid_opt().as_deref(),
    )
}

/// What a re-key does to the vault's recorded owner.
///
/// Minting new metadata used to claim ownership as a side effect: [`prepare_shareable`] builds a
/// whole new meta and, until this existed, the fresh `owner_sid` stamp from [`build_passphrase_meta`]
/// simply stood — so any account that could unlock a shared vault became its recorded owner by
/// changing the passphrase, silently. Every caller now says which it means, and the default across
/// the codebase is [`Keep`](Self::Keep).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerOnRekey {
    /// The vault keeps the owner it already had (a rotation, not a creation). On a Device source
    /// there is nothing to keep, so the creator stamp stands — that is `create_shareable_vault`.
    Keep,
    /// A confirmed takeover: stamp the re-keying account as owner and record the transfer.
    Claim,
}

/// What a re-key gate decided — see [`gate_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyGate {
    /// Don't re-key at all. The caller returns an error before any key is derived and long before
    /// anything is written, so nothing on disk changes and the old passphrase still opens the vault.
    Refuse,
    /// Go ahead, with this effect on the recorded owner.
    Allow(OwnerOnRekey),
}

/// The pure re-key gate: given who owns the vault and whether the user explicitly confirmed a
/// takeover, decide whether a passphrase change may proceed and what it does to the owner record.
///
/// Only `Joined` — a shared vault provably stamped with ANOTHER account's SID — is ever refused, and
/// it has an escape hatch, because a vault nobody can re-key is the one unrecoverable state in this
/// design. `Unknown` deliberately falls open: [`ownership_given`] returns it for every shareable
/// vault off Windows (there is no SID to compare), for every vault created before ownership was
/// recorded, and for any SID-resolution hiccup — so blocking there would deny a genuine sole owner
/// their only re-key. It falls open to `Keep`, not to a claim, which is what stops an unowned vault
/// being silently claimed by whoever rotates first.
///
/// Pure and one layer below the command on purpose: the whole rule is locked by a table test that
/// needs no vault, no keychain and no Windows account.
pub fn gate_for(ownership: VaultOwnership, confirm_transfer: bool) -> RekeyGate {
    match ownership {
        VaultOwnership::Joined if !confirm_transfer => RekeyGate::Refuse,
        VaultOwnership::Joined => RekeyGate::Allow(OwnerOnRekey::Claim),
        VaultOwnership::Device | VaultOwnership::Owned | VaultOwnership::Unknown => {
            RekeyGate::Allow(OwnerOnRekey::Keep)
        }
    }
}

/// The make-private variant of [`gate_for`]: same `Joined` refusal, and **no hatch**.
///
/// Making a joined vault private is a strictly worse takeover than re-keying it — it re-keys the
/// vault to the joiner's DEVICE key (held in their keychain alone), decrypts the Markdown, and moves
/// the whole folder into their profile dir. Unlike a passphrase change, there is no passphrase that
/// gets the real owner back in, so this is the one action in the vault surface with no recovery. It
/// therefore refuses outright, exactly as `delete_shared_vault` does, and the caller points the user
/// at leaving the vault instead — which is non-destructive and already exists.
///
/// The carried [`OwnerOnRekey::Keep`] is moot on the allowed path (`private_meta` clears the owner
/// on the way to a device vault); it is `Keep` because there is nothing to claim.
pub fn private_gate_for(ownership: VaultOwnership) -> RekeyGate {
    match ownership {
        VaultOwnership::Joined => RekeyGate::Refuse,
        VaultOwnership::Device | VaultOwnership::Owned | VaultOwnership::Unknown => {
            RekeyGate::Allow(OwnerOnRekey::Keep)
        }
    }
}

/// The current account's SID for the ownership check — `None` off-Windows or on lookup failure.
#[cfg(windows)]
fn current_user_sid_opt() -> Option<String> {
    acl::current_user_sid().ok()
}

#[cfg(not(windows))]
fn current_user_sid_opt() -> Option<String> {
    None
}

/// The owner SID to stamp into a NEW shareable vault's meta (Windows-only; `None` elsewhere).
#[cfg(windows)]
fn owner_sid_for_new_share() -> Option<String> {
    acl::current_user_sid().ok()
}

#[cfg(not(windows))]
fn owner_sid_for_new_share() -> Option<String> {
    None
}

/// Path to a vault's metadata file inside its folder.
pub fn meta_path(vault_root: &Path) -> PathBuf {
    vault_root.join(META_FILENAME)
}

/// Serial number making concurrent temp names unique within a process, so two writers aiming at the
/// same target can never adopt each other's half-written file.
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write `bytes` to `path` atomically: into a sibling temp file, flushed to disk, then renamed over
/// the target. An interrupted write then leaves either the old file or the new one, never a
/// truncated one — which matters more here than anywhere else in the app, because
/// [`crypto::decrypt`] authenticates the WHOLE container: a half-written encrypted file is not a
/// partial document, it is a permanently unreadable one.
///
/// The temp file is a sibling so the rename stays within one volume (where it is atomic, and can't
/// silently degrade to a copy), and carries a `.tmp` suffix, which the vault walks already ignore.
/// It is created with `File::create` rather than through `tempfile`, so the file keeps the process
/// umask's permissions exactly as `fs::write` gave it — a shared vault's folder may be read by
/// another account, and `tempfile`'s 0600 would lock them out.
///
/// Returns the raw [`std::io::Error`] so each caller can keep its own path-bearing classification.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{seq}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);

    let write = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        // Without this the rename can land before the data does, turning a crash into an empty
        // file that reads as authentic — the exact failure the temp file exists to prevent.
        file.sync_all()
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
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
        // Classified (Denied vs transient), path-bearing — an access-denied vault folder
        // must never surface as a bare "io error: os error 5" (the ACL-lockout incident).
        Err(e) => Err(io_at("read the vault's settings", &path)(e)),
    }
}

/// Write `vault-meta.json` atomically (see [`write_atomic`]), so a crash mid-write can never leave
/// a half-written metadata file.
pub fn store_meta(vault_root: &Path, meta: &VaultMeta) -> Result<()> {
    std::fs::create_dir_all(vault_root).map_err(io_at("write the vault's settings", vault_root))?;
    let path = meta_path(vault_root);
    let json = serde_json::to_vec_pretty(meta)
        .map_err(|e| Error::Other(format!("could not encode {META_FILENAME}: {e}")))?;
    write_atomic(&path, &json).map_err(io_at("write the vault's settings", &path))?;
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
    /// this account's access was revoked). Boot LOCKED with the carried fault instead
    /// of silently creating a fresh empty vault inside someone else's folder — the
    /// failure that made a joined-then-broken vault look like "all my data vanished".
    PointedVaultMissing(VaultFault),
}

/// Decide how boot treats the (possibly absent) vault metadata. Pure: takes whether a
/// pointer redirects this profile and the outcome of loading the pointed root's meta
/// (`Err` = the load itself failed, carried as a classified [`VaultFault`] so the
/// decision stays testable without constructing real I/O errors — and so the UI can
/// pick Repair vs rejoin by `code`). A missing/unreadable meta is only auto-created at
/// the DEFAULT location; behind a pointer it is a reportable fault. A meta-load
/// failure at the default location stays fatal (`Err`), as before.
pub fn boot_meta_decision(
    pointer_present: bool,
    meta: std::result::Result<Option<VaultMeta>, VaultFault>,
) -> std::result::Result<BootMeta, VaultFault> {
    match (pointer_present, meta) {
        (_, Ok(Some(m))) => Ok(BootMeta::UseExisting(Box::new(m))),
        (false, Ok(None)) => Ok(BootMeta::CreateDeviceDefault),
        (false, Err(e)) => Err(e),
        (true, Ok(None)) => Ok(BootMeta::PointedVaultMissing(VaultFault {
            code: VaultFaultCode::NoVault,
            op: "open the vault".into(),
            path: None,
            message: "the folder doesn't contain a PM vault any more".into(),
        })),
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
    ///
    /// A MAC mismatch is reported ahead of a downgrade repair, not behind it: on that path the
    /// repair is applied in memory only, so the older "PM has turned it back on" wording would
    /// promise a durable fix that deliberately did not happen.
    pub fn warning(&self) -> Option<String> {
        if self.mac_mismatch {
            Some(
                "This vault's settings file failed its integrity check — it was altered outside \
                 PM. PM has ignored the change and kept your notes encrypted, and will keep saying \
                 so until the file is replaced. If you didn't change it, check who can reach the \
                 vault folder."
                    .into(),
            )
        } else if self.downgrade_corrected {
            Some(
                "This vault's \"encrypt notes at rest\" setting had been switched off outside PM. \
                 PM has turned it back on, so your notes stay encrypted. If you didn't change it, \
                 check who can reach the vault folder."
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

/// Whether this vault is a passphrase (shareable) vault by EVIDENCE rather than by claim.
///
/// `key_mode` is one word in `vault-meta.json`, a file that a shared vault deliberately exposes to
/// other accounts — so it is exactly the field to flip to turn Markdown encryption off, and on its
/// own it proves nothing. Two other markers of a passphrase vault are harder to forge and costlier
/// to remove: the Argon2id block, without which the owner can no longer unlock this vault from any
/// profile at all, and a verifier only the real master can satisfy. Any one of the three is enough,
/// because the answer can only ever move protection UP: a genuine device vault carries none of them
/// and is left exactly as it was.
fn is_shareable(meta: &VaultMeta, master: &[u8; KEY_LEN]) -> bool {
    meta.key_mode == KeyMode::Passphrase
        || meta.kdf.is_some()
        // A verifier that won't parse counts as present: "can't tell" resolves towards encrypting.
        || meta
            .verifier
            .as_ref()
            .is_some_and(|v| verifier::check(v, master).unwrap_or(true))
}

/// Authenticate the non-secret `vault-meta.json` against a keyed MAC under a master subkey, and repair a
/// silently-downgraded Markdown policy (M-3). `master` MUST already be authenticated by the caller (the
/// DB opened, or the passphrase verifier passed) — this is the first point where that trust exists.
///
/// Additive + backward-compatible: a legacy vault with no stored MAC is stamped on this first
/// authenticated open, and enforced thereafter. Never hard-fails — the master is trusted, so we
/// correct-and-continue rather than lock the user out of their own data. The returned report drives
/// a non-blocking warning.
///
/// **A failed MAC is never re-stamped.** This used to write a fresh, valid MAC over whatever it
/// found, which made an edit made outside PM the new authenticated truth and cleared the warning
/// after a single launch. Verification failing is precisely when the file's contents are not ours to
/// sign, so the repair below is applied in memory (where [`from_meta`](MarkdownCipher::from_meta)
/// keeps the live cipher safe) and the file is left as found, mismatch and all, to be reported on
/// every open until it is legitimately rewritten.
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
    // Repair a copy we can persist; the live cipher is already forced safe by `from_meta`. The
    // trigger is `is_shareable`, not `key_mode` alone, so flipping that one word to `device` no
    // longer takes the whole Markdown policy with it.
    let mut fixed = meta.clone();
    if is_shareable(meta, master)
        && (fixed.key_mode != KeyMode::Passphrase
            || fixed.markdown.encryption == MarkdownEncryption::None)
    {
        fixed.key_mode = KeyMode::Passphrase;
        fixed.markdown.encryption = MarkdownEncryption::XChaCha20Poly1305;
        fixed.markdown.subkey = MARKDOWN_SUBKEY_SCHEME.to_string();
        report.downgrade_corrected = true;
    }
    // Stamp a legacy vault, or persist a downgrade repair. A clean, already-stamped vault is a pure
    // verify with no write — and so, now, is a tampered one.
    if !report.mac_mismatch && (legacy || report.downgrade_corrected) {
        fixed.meta_mac = Some(meta_mac(&fixed, master)?.to_hex().to_string());
        store_meta(vault_root, &fixed)?;
    }
    Ok(report)
}

/// Reconcile a freshly-adopted vault's metadata to THIS machine after a backup restore (or a
/// relocate-home): drop any `owner_sid` the source recorded — a Windows account SID that can't be
/// valid on the adopting machine/account (see [`is_vault_owner`]), and which off its origin account
/// would gate connector setup against a foreign owner — plus any [`OwnershipTransfer`] it carried,
/// which names two SIDs from a machine this vault has left and would otherwise ride along forever —
/// then (re-)stamp the meta MAC under `master`.
/// Clearing the MAC first makes [`authenticate_meta`] treat this as a fresh stamp, so neither the
/// cleared owner nor a keep-passphrase adopt (which preserves the source meta verbatim, cloned MAC and
/// all) reads as "altered outside PM" on the next open. `master` MUST already be authenticated (the DB
/// opened with it). Idempotent: a device vault (owner already `None`) just gets a clean re-stamp.
pub fn normalize_adopted_meta(vault_root: &Path, master: &[u8; KEY_LEN]) -> Result<()> {
    let Some(mut meta) = load_meta(vault_root)? else {
        return Ok(());
    };
    meta.owner_sid = None;
    meta.ownership_transfer = None;
    meta.meta_mac = None;
    authenticate_meta(vault_root, &meta, master)?;
    Ok(())
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
    let db_path = vault_root.join(DB_FILENAME);
    let markdown_dir = vault_root.join(MARKDOWN_DIRNAME);
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
    // Classified + path-bearing: on a pointed root whose ACLs broke, these are the first
    // fresh handles a command opens, so their failure is what the user actually sees.
    //
    // Skipped entirely once a full purge has run: PM keeps running after the erase on macOS and
    // Linux, and this is the other half of the pair (with `paths::data_dir`) that would otherwise
    // rebuild the folders the user was just told were gone.
    if !paths::data_dir_is_purged() {
        std::fs::create_dir_all(&resolved.vault_root)
            .map_err(io_at("prepare the vault folder", &resolved.vault_root))?;
        std::fs::create_dir_all(&resolved.markdown_dir)
            .map_err(io_at("prepare the vault folder", &resolved.markdown_dir))?;
    }
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
        //
        // The claim to distrust is `key_mode` as much as `encryption`: reading the mode straight off
        // the file left the enforcement resting on the one field an edit would target first, so this
        // asks [`is_shareable`] instead, which wants evidence the master can corroborate.
        let encryption = if is_shareable(meta, master) {
            MarkdownEncryption::XChaCha20Poly1305
        } else {
            meta.markdown.encryption
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

    /// Decode on-disk bytes to their plaintext, by magic (the byte analogue of
    /// [`decode`](Self::decode), the counterpart to [`encode_bytes_for`](Self::encode_bytes_for)): an
    /// encrypted container is decrypted; anything else is handed back untouched. Like `decode`, this
    /// is what tolerates a folder mid-migration (some plaintext, some ciphertext).
    ///
    /// Takes the bytes **by value** — unlike `decode`, which borrows a string — so re-encoding a
    /// multi-megabyte photo original doesn't copy it just to hand it straight back.
    pub fn decode_bytes(&self, bytes: Vec<u8>, path: &Path) -> Result<Vec<u8>> {
        if crypto::is_encrypted(&bytes) {
            let key = self.subkey.as_ref().ok_or_else(|| {
                Error::Other("this vault file is encrypted but no Markdown key is loaded".into())
            })?;
            crypto::decrypt(&bytes, key, &self.vault_id, &Self::aad_stem(path))
        } else {
            Ok(bytes)
        }
    }

    /// Read + decrypt a file's bytes from disk (the byte analogue of [`read`](Self::read), the byte
    /// counterpart to [`write_bytes_to`](Self::write_bytes_to)). Used to serve an opt-in saved photo
    /// original back to the reader regardless of the vault's cipher policy.
    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        let bytes = std::fs::read(path)?;
        self.decode_bytes(bytes, path)
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

    /// Encode + write a Markdown file to `path` per policy, atomically (see [`write_atomic`]) —
    /// this and [`write_bytes_to`](Self::write_bytes_to) are the only writers of vault content, so
    /// they are where a crash would otherwise cost the user a whole note or transcript.
    pub fn write_to(&self, path: &Path, content: &str) -> Result<()> {
        let bytes = self.encode_for(path, content)?;
        write_atomic(path, &bytes)?;
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

    /// Encode + write arbitrary bytes to `path` per policy (see
    /// [`encode_bytes_for`](Self::encode_bytes_for)), atomically (see [`write_atomic`]).
    pub fn write_bytes_to(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let out = self.encode_bytes_for(path, bytes)?;
        write_atomic(path, &out)?;
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
        // Record who created the shareable vault, so connector setup can be gated to the owner (a
        // joiner can't sync a connector anyway — the tokens live in the owner's per-account keychain).
        owner_sid: owner_sid_for_new_share(),
        // A NEW vault has no history to record. A re-key that means to change hands sets this in
        // `prepare_shareable`, which is the only place a takeover is ever written.
        ownership_transfer: None,
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
///
/// **Ownership carries forward unless the caller says otherwise** ([`OwnerOnRekey`]), and this is
/// the load-bearing half of the owner gate rather than the command-level check above it. The meta
/// this returns is minted fresh by [`build_passphrase_meta`], which stamps the CALLING account as
/// owner — right for a creation, and a silent takeover for a rotation. Passing `Keep` on a
/// Passphrase source restores the old owner over that stamp, so the worst a bypassed or fallen-open
/// gate can do is "re-keyed, ownership unchanged". The `Keep` arm is guarded on the SOURCE mode: a
/// Device source has no owner to keep and the creator stamp stands, which is exactly
/// `create_shareable_vault` and is why it does not regress.
pub fn prepare_shareable(
    old_meta: &VaultMeta,
    passphrase: &str,
    owner: OwnerOnRekey,
) -> Result<(VaultMeta, Secret)> {
    prepare_shareable_with(
        old_meta,
        passphrase,
        owner,
        kdf::calibrate(CALIBRATE_TARGET_MS),
    )
}

/// [`prepare_shareable`] with the Argon2id cost params already chosen — split out for exactly the
/// reason [`build_passphrase_meta`] is: the ownership rules below are what need locking down, and a
/// real `kdf::calibrate` costs seconds per case (it derives a dozen keys to hit its 350 ms target).
fn prepare_shareable_with(
    old_meta: &VaultMeta,
    passphrase: &str,
    owner: OwnerOnRekey,
    params: KdfParams,
) -> Result<(VaultMeta, Secret)> {
    let (mut meta, master) = build_passphrase_meta(passphrase, params)?;
    meta.vault_id = old_meta.vault_id.clone();
    meta.created_at = old_meta.created_at.clone();
    match owner {
        OwnerOnRekey::Keep if old_meta.key_mode == KeyMode::Passphrase => {
            meta.owner_sid = old_meta.owner_sid.clone();
            // Any earlier takeover is part of the vault's history, not of this re-key — carry it
            // forward too, or a rotation would quietly erase the record of the last one.
            meta.ownership_transfer = old_meta.ownership_transfer.clone();
        }
        // Device -> Passphrase: nothing to keep, so the creator stamp stands (see above).
        OwnerOnRekey::Keep => {}
        OwnerOnRekey::Claim => {
            meta.ownership_transfer = Some(OwnershipTransfer {
                from_sid: old_meta.owner_sid.clone(),
                to_sid: meta.owner_sid.clone(),
                at: chrono::Utc::now().to_rfc3339(),
            });
        }
    }
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
        return Err(Error::Vault(VaultFault {
            code: VaultFaultCode::WrongPassphrase,
            op: "unlock the vault".into(),
            path: None,
            message: "That passphrase doesn't match this vault.".into(),
        }));
    }
    // The passphrase is now proven, so the master is trusted — the first point at which we can
    // authenticate + repair the non-secret meta (M-3). The report drives a non-blocking UI warning.
    let report = authenticate_meta(&resolved.vault_root, meta, &master)?;
    let key = db_key_hex(&master);
    // The verifier PASSED, so a wrong-key-shaped open failure here means the store itself is
    // damaged (e.g. a truncated copy) — tell that apart from "wrong passphrase" so recovery
    // guidance doesn't send the user back to retype a passphrase that is already right. Other
    // open failures (transient AV/indexer locks, disk I/O) pass through with their retryable
    // messages untouched.
    let conn = db::open(&resolved.db_path, key.expose()).map_err(|e| {
        if e.to_string().contains(db::WRONG_KEY_OR_CORRUPT_MSG) {
            Error::Vault(VaultFault {
                code: VaultFaultCode::Corrupt,
                op: "open the vault's database".into(),
                path: Some(resolved.db_path.display().to_string()),
                message: "The passphrase is right, but the vault's database won't open — the \
                          file may be damaged."
                    .into(),
            })
        } else {
            e
        }
    })?;
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

/// What a successful open yields: the store, the resolved 32-byte master the session runtime is
/// built from, and what authenticating the metadata found.
pub type BootOpen = (Connection, Zeroizing<[u8; KEY_LEN]>, MetaAuthReport);

/// Decide how to open a vault at boot from its metadata, via [`current_db_key`]. On
/// success also returns the resolved 32-byte master, from which the caller builds the
/// session runtime (the Markdown cipher *and* the always-on rules cipher are both
/// subkeys of it), and the meta authentication report.
///
/// The report is returned rather than only logged because boot is the one open path with no UI in
/// front of it: `unlock_vault` and `adopt_shared_vault` hand theirs to the user, and a vault that
/// comes up on a cached key used to have nowhere to say the same thing but stderr.
pub fn open_at_boot(resolved: &ResolvedVault, meta: &VaultMeta) -> Result<Option<BootOpen>> {
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
                    let report = log_meta_auth(&resolved.vault_root, meta, &master);
                    return Ok(Some((conn, master, report)));
                }
            }
            // No recovery applied, or the device key didn't open it either — the original
            // error is the honest one to report (the cache wasn't the problem).
            return Err(e);
        }
    };
    let master = master_from_db_key_hex(key.expose())?;
    let report = log_meta_auth(&resolved.vault_root, meta, &master);
    Ok(Some((conn, master, report)))
}

/// Authenticate + repair the meta on the boot/reopen path, logging a warning and handing the report
/// back so the caller can put it somewhere the user will actually look.
/// Best-effort: an authentication or persist failure must not abort opening a vault the master already
/// unlocked, so it is logged rather than propagated (the live cipher is safe regardless, via `from_meta`).
/// Such a failure reports a clean slate — it means we could not establish anything, not that anything
/// is wrong.
fn log_meta_auth(vault_root: &Path, meta: &VaultMeta, master: &[u8; KEY_LEN]) -> MetaAuthReport {
    match authenticate_meta(vault_root, meta, master) {
        Ok(report) => {
            if let Some(w) = report.warning() {
                eprintln!("vault: {w}");
            }
            report
        }
        Err(e) => {
            eprintln!("vault: meta authentication could not complete: {e}");
            MetaAuthReport::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_falls_open_without_a_recorded_owner_and_matches_by_sid() {
        // Device / legacy vault: no owner recorded → always the owner (no restriction).
        assert!(is_owner_given(&None, Some("S-1-5-21-anyone")));
        assert!(is_owner_given(&None, None));
        // A stamped owner: only the matching SID owns it.
        let owner = Some("S-1-5-21-owner".to_string());
        assert!(is_owner_given(&owner, Some("S-1-5-21-owner")));
        assert!(!is_owner_given(&owner, Some("S-1-5-21-joiner")));
        // SID unresolved (or off-Windows) → fail open, so a hiccup never locks the real owner out.
        assert!(is_owner_given(&owner, None));
    }

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
        // A pointer to a vault-less folder is a NoVault fault (PR3's tombstone check
        // hangs off this code), not a generic message.
        assert!(matches!(
            boot_meta_decision(true, Ok(None)),
            Ok(BootMeta::PointedVaultMissing(VaultFault {
                code: VaultFaultCode::NoVault,
                ..
            }))
        ));
        // The classification travels through untouched: a Denied load fault comes out as
        // a Denied PointedVaultMissing, so the boot screen can offer Repair access.
        let denied = VaultFault {
            code: VaultFaultCode::Denied,
            op: "read the vault's settings".into(),
            path: Some("C:/shared".into()),
            message: "access is denied".into(),
        };
        assert_eq!(
            boot_meta_decision(true, Err(denied.clone())),
            Ok(BootMeta::PointedVaultMissing(denied.clone()))
        );
        // At the default location a meta-load failure stays fatal, as before.
        assert_eq!(boot_meta_decision(false, Err(denied.clone())), Err(denied));
    }

    // The two layout pins below KEEP their `"pm.sqlite"` / `"vault"` literals on purpose: a test that
    // asserts `resolve_layout` against the very constants `resolve_layout` is built from proves
    // nothing. They are the only place the names are stated independently, so a sweep that "finishes
    // the job" by substituting `DB_FILENAME` / `MARKDOWN_DIRNAME` here hollows out the contract.
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
        enc_cipher_keyed([3u8; 32])
    }

    /// The same, with a caller-chosen subkey — so a test can model a passphrase CHANGE, which keeps
    /// the vault id and the encryption policy but moves the Markdown subkey.
    fn enc_cipher_keyed(subkey: [u8; 32]) -> MarkdownCipher {
        MarkdownCipher {
            vault_id: "vault-1".to_string(),
            encryption: MarkdownEncryption::XChaCha20Poly1305,
            subkey: Some(Zeroizing::new(subkey)),
        }
    }

    #[test]
    fn a_passphrase_change_re_encodes_saved_photo_originals() {
        // The bug this exists to prevent: `convert_markdown` walks the vault non-recursively, so a
        // passphrase change re-encoded the `.md` files and silently left `vault/photos/` under the
        // OLD subkey — permanently unreadable, and the feature's whole pitch is that the user can
        // delete the original once PM has a copy. Model the real transition: same vault id, same
        // policy, different subkey.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        let photos = vault.join("photos");
        std::fs::create_dir_all(&photos).unwrap();

        let old = enc_cipher_keyed([3u8; 32]);
        let new = enc_cipher_keyed([9u8; 32]);
        let original = b"\x89PNG\r\n\x1a\nnot really a png, but bytes are bytes".to_vec();
        let name = "deadbeef.png.pmenc";
        old.write_bytes_to(&photos.join(name), &original).unwrap();

        // Precondition: the new key genuinely cannot read what the old key wrote. Without this the
        // test could pass while proving nothing.
        assert!(
            new.read_bytes(&photos.join(name)).is_err(),
            "the new subkey must not already open the old ciphertext"
        );

        assert_eq!(
            crate::ingest::convert_photo_originals(&vault, &old, &new).unwrap(),
            1
        );
        assert_eq!(
            new.read_bytes(&photos.join(name)).unwrap(),
            original,
            "the saved original must survive the re-key byte-for-byte"
        );
        assert!(
            crypto::is_encrypted(&std::fs::read(photos.join(name)).unwrap()),
            "still encrypted at rest, just under the new key"
        );

        // Idempotent: re-running after an interruption changes nothing.
        assert_eq!(
            crate::ingest::convert_photo_originals(&vault, &new, &new).unwrap(),
            0
        );
    }

    #[test]
    fn making_a_vault_private_decrypts_saved_photo_originals() {
        // The other direction (passphrase → device): the originals must come back to plaintext, or
        // "make private" leaves the user's own photos locked to a key the vault no longer has.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        let photos = vault.join("photos");
        std::fs::create_dir_all(&photos).unwrap();

        let enc = enc_cipher();
        let plain = MarkdownCipher::plaintext("vault-1");
        let original = b"raw jpeg bytes".to_vec();
        // Named as it was saved, under encryption — the name it keeps (we re-encode in place).
        let name = "cafe1234.jpg.pmenc";
        enc.write_bytes_to(&photos.join(name), &original).unwrap();

        assert_eq!(
            crate::ingest::convert_photo_originals(&vault, &enc, &plain).unwrap(),
            1
        );
        let on_disk = std::fs::read(photos.join(name)).unwrap();
        assert_eq!(on_disk, original, "plaintext on disk after make-private");
        assert!(!crypto::is_encrypted(&on_disk));
        assert_eq!(
            crate::ingest::convert_photo_originals(&vault, &plain, &plain).unwrap(),
            0,
            "idempotent once already plaintext"
        );
    }

    #[test]
    fn converting_a_vault_moves_the_documents_and_the_photos_together() {
        // The actual regression. `convert_photo_originals` working is not the property that broke —
        // the property that broke is that a key migration converts BOTH halves, and a unit test of
        // either half alone cannot see the other being forgotten (the same blind spot that let the
        // #298 passphrase fix ship green: its test guarded the layer BELOW the missed call site).
        // So test the seam the migration actually calls.
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), key).unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("photos")).unwrap();

        let old = enc_cipher_keyed([3u8; 32]);
        let new = enc_cipher_keyed([9u8; 32]);
        old.write_to(&vault.join("a.md.pmenc"), "# A\nalpha")
            .unwrap();
        let img = b"photo bytes".to_vec();
        old.write_bytes_to(&vault.join("photos").join("h.png.pmenc"), &img)
            .unwrap();

        assert_eq!(
            crate::ingest::convert_vault_files(&conn, &vault, &old, &new).unwrap(),
            2,
            "the document AND the saved original both re-key"
        );
        assert_eq!(new.read(&vault.join("a.md.pmenc")).unwrap(), "# A\nalpha");
        assert_eq!(
            new.read_bytes(&vault.join("photos").join("h.png.pmenc"))
                .unwrap(),
            img,
            "the photo half is not optional — this is the assert that fails if it is dropped"
        );
    }

    #[test]
    fn a_vault_with_no_saved_photos_converts_cleanly() {
        // The overwhelmingly common shape: copy-to-vault is opt-in and off by default, so most
        // vaults have no photos/ dir at all. A missing folder is not an error.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let c = enc_cipher();
        assert_eq!(
            crate::ingest::convert_photo_originals(&vault, &c, &c).unwrap(),
            0
        );
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
        store_meta(dir.path(), &downgraded).unwrap();
        let r3 = authenticate_meta(dir.path(), &downgraded, &master).unwrap();
        assert!(r3.downgrade_corrected, "the plaintext downgrade is caught");
        assert!(
            r3.mac_mismatch,
            "the stale MAC no longer matches the tampered fields"
        );

        // ...and the file is left exactly as it was found. This USED to re-stamp the tampered meta
        // with a fresh, valid MAC, which made the edit authentic and silenced the warning after one
        // launch. Verification failing is the one moment these bytes are not ours to sign.
        let after = load_meta(dir.path()).unwrap().unwrap();
        assert_eq!(
            after.markdown.encryption,
            MarkdownEncryption::None,
            "a failed MAC is never re-signed, so the tampered file stays as found"
        );
        assert_eq!(after.meta_mac, stamped.meta_mac, "the stale MAC is kept");
        let r4 = authenticate_meta(dir.path(), &after, &master).unwrap();
        assert!(
            r4.mac_mismatch && r4.needs_warning(),
            "and it keeps reporting on every open, not just the first"
        );
    }

    #[test]
    fn flipping_key_mode_to_device_cannot_turn_markdown_encryption_off() {
        // The reported shape: an account that can write the vault FOLDER but has no passphrase edits
        // two words of `vault-meta.json` — `key_mode: device` and `encryption: none`. The owner's
        // next launch opens from its cached key, and every note written after that would land in
        // cleartext in a folder that account can read.
        let dir = tempfile::tempdir().unwrap();
        let (meta, master) = build_passphrase_meta("pw", cheap_params()).unwrap();
        store_meta(dir.path(), &meta).unwrap();
        authenticate_meta(dir.path(), &meta, &master).unwrap();
        let stamped = load_meta(dir.path()).unwrap().unwrap();

        let mut tampered = stamped.clone();
        tampered.key_mode = KeyMode::Device;
        tampered.markdown.encryption = MarkdownEncryption::None;

        // The live cipher is the thing that matters: it decides what the next write looks like.
        let cipher = MarkdownCipher::from_meta(&tampered, &master);
        assert!(
            cipher.encryption_on() && cipher.subkey.is_some(),
            "a vault that still carries its passphrase artefacts keeps encrypting, whatever the \
             file claims about its mode"
        );

        let report = authenticate_meta(dir.path(), &tampered, &master).unwrap();
        assert!(report.mac_mismatch, "and the edit is reported");
        assert!(
            report.downgrade_corrected,
            "the repair now covers key_mode too, not only the encryption field"
        );

        // Stripping the MAC line is not a way around it either: that path skips verification
        // entirely and used to bless whatever it found, silently.
        let mut stripped = tampered.clone();
        stripped.meta_mac = None;
        assert!(MarkdownCipher::from_meta(&stripped, &master).encryption_on());
        let report = authenticate_meta(dir.path(), &stripped, &master).unwrap();
        assert!(
            report.downgrade_corrected,
            "an unstamped downgrade is still caught and still warned about"
        );
    }

    #[test]
    fn a_genuine_device_vault_is_left_plaintext() {
        // The other half of the invariant: `is_shareable` must not sweep up an ordinary device
        // vault, whose Markdown is plaintext by design. It carries no kdf and no verifier.
        let meta = VaultMeta::new_device();
        let master = [3u8; KEY_LEN];
        assert!(!is_shareable(&meta, &master));
        assert!(!MarkdownCipher::from_meta(&meta, &master).encryption_on());

        let dir = tempfile::tempdir().unwrap();
        store_meta(dir.path(), &meta).unwrap();
        let report = authenticate_meta(dir.path(), &meta, &master).unwrap();
        assert!(
            !report.needs_warning(),
            "a device vault's first stamp is silent, exactly as before"
        );
        assert_eq!(
            load_meta(dir.path()).unwrap().unwrap().markdown.encryption,
            MarkdownEncryption::None,
            "and nothing forced encryption on it"
        );
    }

    #[test]
    fn normalize_adopted_meta_clears_owner_sid_and_restamps_clean() {
        // A cross-machine restore adopts a vault whose meta carries a foreign account's `owner_sid`
        // and a MAC computed over it. Normalizing must drop the SID and re-stamp so the very next
        // authenticated open sees NO tampering (a stale MAC would surface a false "altered outside PM").
        let dir = tempfile::tempdir().unwrap();
        let master = [7u8; KEY_LEN];
        let mut meta = VaultMeta::new_device();
        meta.owner_sid = Some("S-1-5-21-1111111111-2222222222-3333333333-1001".into());
        // ...and a takeover recorded on that machine, which names two SIDs of a sharing arrangement
        // this vault has left behind. It must not ride along either.
        meta.ownership_transfer = Some(OwnershipTransfer {
            from_sid: Some("S-1-5-21-1111111111-2222222222-3333333333-1002".into()),
            to_sid: meta.owner_sid.clone(),
            at: "2026-01-02T03:04:05+00:00".into(),
        });
        // Stamp a MAC over the owner-bearing meta, as an adopted vault would carry on disk.
        meta.meta_mac = Some(meta_mac(&meta, &master).unwrap().to_hex().to_string());
        store_meta(dir.path(), &meta).unwrap();

        normalize_adopted_meta(dir.path(), &master).unwrap();

        let after = load_meta(dir.path()).unwrap().unwrap();
        assert_eq!(after.owner_sid, None, "the foreign owner SID is cleared");
        assert_eq!(
            after.ownership_transfer, None,
            "and so is the foreign machine's takeover record"
        );
        assert!(after.meta_mac.is_some(), "a fresh MAC is stamped");
        let report = authenticate_meta(dir.path(), &after, &master).unwrap();
        assert!(
            !report.needs_warning(),
            "the re-stamped meta verifies cleanly (no false tamper warning)"
        );
    }

    #[test]
    fn ownership_keeps_unknown_apart_from_ours() {
        // `is_vault_owner` answers `true` for both "we own it" and "nobody recorded an owner",
        // which is right for gating connectors and wrong for anything that deletes. These are the
        // four realities it folds together.
        let me = Some("S-1-5-21-me");
        let them = Some("S-1-5-21-them");

        // A device vault is ours by construction: its key is in one account's keychain, so the
        // owner field is irrelevant and so is the platform.
        assert_eq!(
            ownership_given(KeyMode::Device, None, None),
            VaultOwnership::Device
        );
        assert_eq!(
            ownership_given(KeyMode::Device, them, me),
            VaultOwnership::Device
        );

        assert_eq!(
            ownership_given(KeyMode::Passphrase, me, me),
            VaultOwnership::Owned
        );
        assert_eq!(
            ownership_given(KeyMode::Passphrase, them, me),
            VaultOwnership::Joined
        );
        // No stamp, or nobody to compare it against. NOT ours -- where the old bool said `true`.
        assert_eq!(
            ownership_given(KeyMode::Passphrase, None, me),
            VaultOwnership::Unknown
        );
        assert_eq!(
            ownership_given(KeyMode::Passphrase, me, None),
            VaultOwnership::Unknown,
            "off Windows there is no SID to compare, so every shared vault is Unknown -- the \
             device case above is what still works there"
        );
        assert!(is_owner_given(&None, me), "the old rule still fails open");
    }

    #[test]
    fn the_rekey_gate_refuses_only_an_unconfirmed_joiner() {
        // All eight ownership x confirm combinations, one layer below `change_vault_passphrase`, so
        // the two properties that make this design recoverable are locked where no vault, keychain or
        // Windows account is needed to check them:
        //
        //   * exactly ONE branch refuses, and the same caller can always re-issue with `true` — so no
        //     reachable state has nobody able to re-key;
        //   * `Unknown` FALLS OPEN, and falls open to `Keep`. `ownership_given` returns Unknown for
        //     every shareable vault off Windows, every vault created before ownership was recorded,
        //     and any SID hiccup. Refusing there is the only way to reach "nobody can re-key this
        //     vault"; claiming there is how an unowned vault gets silently taken by whoever rotates
        //     first. Neither happens.
        use OwnerOnRekey::{Claim, Keep};
        use RekeyGate::{Allow, Refuse};
        use VaultOwnership::{Device, Joined, Owned, Unknown};

        assert_eq!(gate_for(Joined, false), Refuse, "the one blocking branch");
        assert_eq!(gate_for(Joined, true), Allow(Claim), "and its escape hatch");

        for ownership in [Device, Owned, Unknown] {
            for confirm in [false, true] {
                assert_eq!(
                    gate_for(ownership, confirm),
                    Allow(Keep),
                    "{ownership:?} rotates as before and never claims, confirmed or not"
                );
            }
        }

        // Make-private is the same refusal with NO hatch, because it is the one action here with no
        // way back: it re-keys to this account's device key, decrypts, and moves the folder into this
        // profile. `delete_shared_vault` already refuses `Joined` outright for the same reason.
        assert_eq!(private_gate_for(Joined), Refuse);
        for ownership in [Device, Owned, Unknown] {
            assert_eq!(
                private_gate_for(ownership),
                Allow(Keep),
                "{ownership:?} may still be made private"
            );
        }
    }

    #[test]
    fn re_keying_a_shared_vault_carries_its_owner_forward() {
        // The regression test for the silent takeover, and the load-bearing half of the fix. A
        // change-passphrase runs `prepare_shareable`, which MINTS a whole new meta — and
        // `build_passphrase_meta` stamps the calling account as owner. Right for a creation; a silent
        // transfer for a rotation. `Keep` must put the old owner back over that stamp, so even a
        // bypassed or fallen-open gate can only ever produce "re-keyed, ownership unchanged".
        let them = "S-1-5-21-1111111111-2222222222-3333333333-1001";
        let mut old = VaultMeta::new_device();
        old.key_mode = KeyMode::Passphrase;
        old.owner_sid = Some(them.into());

        let (meta, _key) = prepare_shareable_with(
            &old,
            "correct horse battery staple",
            OwnerOnRekey::Keep,
            cheap_params(),
        )
        .unwrap();
        assert_eq!(
            meta.owner_sid.as_deref(),
            Some(them),
            "a rotation keeps the vault's owner — it does not stamp the account doing the rotating"
        );
        assert_eq!(meta.vault_id, old.vault_id, "and the vault's identity");
        assert_eq!(
            meta.ownership_transfer, None,
            "a plain rotation records no transfer"
        );

        // The other half, and the reason `Unknown` can safely fall open: an UNOWNED shared vault
        // stays unowned. Before this, whoever rotated first quietly became its owner.
        let mut unowned = old.clone();
        unowned.owner_sid = None;
        let (meta, _key) = prepare_shareable_with(
            &unowned,
            "correct horse battery staple",
            OwnerOnRekey::Keep,
            cheap_params(),
        )
        .unwrap();
        assert_eq!(
            meta.owner_sid, None,
            "no owner recorded stays no owner recorded, on every platform"
        );

        // An earlier takeover is the vault's history, not this re-key's — a rotation must not erase it.
        let mut transferred = old.clone();
        transferred.ownership_transfer = Some(OwnershipTransfer {
            from_sid: Some("S-1-5-21-1111111111-2222222222-3333333333-1002".into()),
            to_sid: Some(them.into()),
            at: "2026-01-02T03:04:05+00:00".into(),
        });
        let (meta, _key) = prepare_shareable_with(
            &transferred,
            "correct horse battery staple",
            OwnerOnRekey::Keep,
            cheap_params(),
        )
        .unwrap();
        assert_eq!(
            meta.ownership_transfer, transferred.ownership_transfer,
            "the record of the last takeover survives a later rotation"
        );
    }

    #[test]
    fn a_device_source_still_stamps_the_creator_and_a_claim_records_the_transfer() {
        // `create_shareable_vault` must not regress: the `Keep` arm is guarded on the SOURCE mode, so
        // a Device vault (which has no owner to keep) still records the account that shared it. The
        // stale SID below is what a formerly-shareable, made-private vault could carry; it must NOT
        // be resurrected as the new owner.
        let stale = "S-1-5-21-1111111111-2222222222-3333333333-1001";
        let mut device = VaultMeta::new_device();
        device.owner_sid = Some(stale.into());

        let (meta, _key) = prepare_shareable_with(
            &device,
            "correct horse battery staple",
            OwnerOnRekey::Keep,
            cheap_params(),
        )
        .unwrap();
        assert_eq!(
            meta.owner_sid,
            owner_sid_for_new_share(),
            "sharing a device vault stamps the account doing the sharing, exactly as before"
        );
        assert_ne!(
            meta.owner_sid.as_deref(),
            Some(stale),
            "and never inherits a stale SID from the device meta"
        );
        assert_eq!(
            meta.ownership_transfer, None,
            "a creation is not a takeover"
        );

        // A CONFIRMED takeover: the new owner is stamped and the change is written down, under the
        // meta MAC, so it is tamper-evident rather than something the taker can quietly erase.
        let mut shared = VaultMeta::new_device();
        shared.key_mode = KeyMode::Passphrase;
        shared.owner_sid = Some(stale.into());
        let (meta, _key) = prepare_shareable_with(
            &shared,
            "correct horse battery staple",
            OwnerOnRekey::Claim,
            cheap_params(),
        )
        .unwrap();
        assert_eq!(meta.owner_sid, owner_sid_for_new_share());
        let transfer = meta
            .ownership_transfer
            .as_ref()
            .expect("a claim records the transfer");
        assert_eq!(transfer.from_sid.as_deref(), Some(stale));
        assert_eq!(transfer.to_sid, owner_sid_for_new_share());
        assert!(
            chrono::DateTime::parse_from_rfc3339(&transfer.at).is_ok(),
            "the timestamp is RFC3339 so the UI can format it: {}",
            transfer.at
        );
    }

    /// Exactly what a build from BEFORE `ownership_transfer` existed wrote for one device vault, in
    /// serde's declaration order (which is the order the MAC covers). A hand-written literal on
    /// purpose: it is the one reference in this suite that does not come from the current struct, so
    /// it is the only thing that can catch the current struct drifting away from it.
    const PRE_FIELD_META_JSON: &str = concat!(
        r#"{"schema":1,"vault_id":"11111111-2222-3333-4444-555555555555","#,
        r#""created_at":"2026-01-02T03:04:05+00:00","app":"org.itsatlas.pm","key_mode":"device","#,
        r#""db_cipher":{"cipher_page_size":4096,"kdf_iter":256000,"hmac_algorithm":"HMAC_SHA512","#,
        r#""kdf_algorithm":"PBKDF2_HMAC_SHA512"},"#,
        r#""markdown":{"encryption":"none","subkey":"blake3-derive-key"},"#,
        r#""owner_sid":"S-1-5-21-1111111111-2222222222-3333333333-1001"}"#
    );

    #[test]
    fn an_absent_ownership_transfer_costs_zero_bytes_so_an_existing_mac_still_verifies() {
        // The compatibility pin. `meta_mac` covers the SERIALIZED meta, so a new field that always
        // emitted would invalidate every stored MAC on disk and greet every user with "this vault's
        // settings file was altered outside PM" on first launch after the update.
        // `skip_serializing_if` is what stops that, and this proves it against a byte string that
        // predates the field rather than against the struct that added it.
        let meta: VaultMeta = serde_json::from_str(PRE_FIELD_META_JSON).unwrap();
        assert_eq!(
            meta.ownership_transfer, None,
            "an absent field parses as None (the `default`), so old files still load"
        );
        assert_eq!(
            serde_json::to_string(&meta).unwrap(),
            PRE_FIELD_META_JSON,
            "and re-serializing reproduces the pre-field bytes exactly — no new key, no reordering"
        );

        // End to end: the tag an OLD build stored, computed here straight from those pre-field bytes
        // without going through the current struct at all, must still verify under THIS build.
        let dir = tempfile::tempdir().unwrap();
        let master = [9u8; KEY_LEN];
        let subkey = blake3::derive_key(META_MAC_CONTEXT, &master);
        let stored = blake3::keyed_hash(&subkey, PRE_FIELD_META_JSON.as_bytes());
        let mut on_disk = meta.clone();
        on_disk.meta_mac = Some(stored.to_hex().to_string());
        store_meta(dir.path(), &on_disk).unwrap();

        let report = authenticate_meta(dir.path(), &on_disk, &master).unwrap();
        assert!(
            !report.mac_mismatch,
            "a vault that never had a takeover must not read as tampered"
        );
        assert!(!report.needs_warning());

        // And the field really is MAC-covered once it IS set — the record cannot be erased silently.
        let mut transferred = on_disk.clone();
        transferred.ownership_transfer = Some(OwnershipTransfer {
            from_sid: Some("S-1-5-21-1111111111-2222222222-3333333333-1002".into()),
            to_sid: Some("S-1-5-21-1111111111-2222222222-3333333333-1001".into()),
            at: "2026-01-02T03:04:05+00:00".into(),
        });
        assert_ne!(
            meta_mac(&transferred, &master).unwrap(),
            meta_mac(&on_disk, &master).unwrap(),
            "a transfer record changes the MAC, so removing one is detectable"
        );
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
    fn a_vault_write_leaves_no_staging_file_and_still_authenticates() {
        // The staging file must not be observable after the write, and — the subtler half — the
        // ciphertext must still be encoded for the FINAL name. `aad_stem` binds the file name, so
        // encoding for the temp path would produce a file that fails authentication the moment it
        // is renamed, with nothing to show for it at write time.
        let dir = tempfile::tempdir().unwrap();
        let c = enc_cipher();
        let path = dir.path().join(c.on_disk_name("note.md"));

        c.write_to(&path, "# v1").unwrap();
        assert_eq!(c.read(&path).unwrap(), "# v1");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no staging file survives a successful write"
        );

        // A rewrite is old-or-new, never a splice of the two.
        c.write_to(&path, "# v2 much longer body text").unwrap();
        assert_eq!(c.read(&path).unwrap(), "# v2 much longer body text");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        // Bytes take the same route, and are what a saved photo original travels on.
        let photo = dir.path().join(c.on_disk_name("shot.jpg"));
        c.write_bytes_to(&photo, &[0xffu8; 64]).unwrap();
        assert_eq!(c.read_bytes(&photo).unwrap(), vec![0xffu8; 64]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_failed_vault_write_leaves_nothing_behind() {
        // The cleanup the count-sensitive photo/export walks depend on: those loops have no
        // extension filter, so a leaked staging file would be re-encoded as if it were a photo.
        let dir = tempfile::tempdir().unwrap();
        let c = enc_cipher();
        let path = dir.path().join("no-such-dir").join(c.on_disk_name("a.md"));
        assert!(c.write_to(&path, "# A").is_err());
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "a write that could not be placed leaves no residue"
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
    fn export_plaintext_frees_the_saved_photo_originals_too() {
        // "Never locked in" has to include the images: the originals are encrypted with the same
        // subkey as the Markdown, so an export that skipped them would hand the user a folder of
        // documents referencing photos only PM could ever open.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("photos")).unwrap();
        let c = enc_cipher();
        c.write_to(&vault.join(c.on_disk_name("a.md")), "# A\nalpha")
            .unwrap();
        let img = b"\x89PNG\r\n\x1a\nphoto bytes".to_vec();
        c.write_bytes_to(&vault.join("photos").join("abc123.png.pmenc"), &img)
            .unwrap();

        let dest = dir.path().join("export");
        let n = crate::ingest::export_plaintext(&vault, &c, &dest).unwrap();
        assert_eq!(n, 2, "the document and the photo both count as written");
        assert_eq!(
            std::fs::read(dest.join("photos").join("abc123.png")).unwrap(),
            img,
            "decrypted, and under a name an image viewer will actually open"
        );
        assert!(
            !dest.join("photos").join("abc123.png.pmenc").exists(),
            "no ciphertext suffix survives the export"
        );
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

    #[test]
    fn convert_markdown_rekeys_the_chats_folder_and_repoints_both_tables() {
        // The v3.19.2 photo bug, re-run for chats — and the reason `convert_vault_files` exists at all.
        // A key migration that walked only the vault root would leave every chat encrypted under the
        // PREVIOUS key: unreadable by the very app that wrote them, with no error anywhere.
        //
        // The second half is a defect this test was written to pin after finding it in the #281 audit.
        // `chat::record_turn_pair` appends the next turn to whatever `chat_sessions.vault_path` holds.
        // The rename used to update `documents` alone, so the next message created a SECOND file under
        // the pre-rename name and split the transcript in two — both stamped with the same
        // `chat_conversation_id`, which a later Rebuild then fights over on `documents.vault_path`'s
        // UNIQUE. Both tables have to follow the file.
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), key).unwrap();
        conn.execute("INSERT INTO conversations(title) VALUES ('A chat')", [])
            .unwrap();
        let conv = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type) \
             VALUES ('chats/chat-01-01-2026-a.md', 'A chat', 'h', 'chat')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope, vault_path) \
             VALUES (?1, 'general', 'chats/chat-01-01-2026-a.md')",
            rusqlite::params![conv],
        )
        .unwrap();

        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("chats")).unwrap();
        std::fs::write(
            vault.join("chats").join("chat-01-01-2026-a.md"),
            "---\nsource_type: chat\n---\n\nhello",
        )
        .unwrap();

        let c = enc_cipher();
        assert_eq!(
            crate::ingest::convert_markdown(&conn, &vault, &c, &c).unwrap(),
            1,
            "the chat is reached at all"
        );

        // It stayed in `chats/` — a rekey that hoisted it back to the root would collide with the
        // relocation pass, which refuses to touch a name that exists in both places.
        let enc = vault.join("chats").join("chat-01-01-2026-a.md.pmenc");
        assert!(crypto::is_encrypted(&std::fs::read(&enc).unwrap()));
        assert_eq!(
            c.read(&enc).unwrap(),
            "---\nsource_type: chat\n---\n\nhello"
        );
        assert!(!vault.join("chats").join("chat-01-01-2026-a.md").exists());

        let doc_path: String = conn
            .query_row("SELECT vault_path FROM documents", [], |r| r.get(0))
            .unwrap();
        let session_path: String = conn
            .query_row("SELECT vault_path FROM chat_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(doc_path, "chats/chat-01-01-2026-a.md.pmenc");
        assert_eq!(
            session_path, "chats/chat-01-01-2026-a.md.pmenc",
            "the session row follows too, or the next turn splits the transcript"
        );
    }

    #[test]
    fn export_plaintext_frees_the_chats_too_and_keeps_them_foldered() {
        // "Never locked in" has to include the conversations. A root-only walk would hand the user an
        // export that LOOKS complete — documents present, no error — with every chat silently missing.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("chats")).unwrap();
        let c = enc_cipher();
        c.write_to(&vault.join(c.on_disk_name("a.md")), "# A\nalpha")
            .unwrap();
        c.write_to(
            &vault
                .join("chats")
                .join(c.on_disk_name("chat-01-01-2026-a.md")),
            "# Chat\nhello",
        )
        .unwrap();

        let dest = dir.path().join("export");
        let n = crate::ingest::export_plaintext(&vault, &c, &dest).unwrap();
        assert_eq!(n, 2, "the document AND the chat");
        assert_eq!(
            std::fs::read_to_string(dest.join("chats").join("chat-01-01-2026-a.md")).unwrap(),
            "# Chat\nhello",
            "decrypted, still foldered, no .pmenc suffix"
        );
        assert!(!dest
            .join("chats")
            .join("chat-01-01-2026-a.md.pmenc")
            .exists());
    }
}
