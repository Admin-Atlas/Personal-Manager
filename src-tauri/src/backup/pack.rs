// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Build a `.pmbackup` archive: enumerate the vault's durable files, then stream them
//! through `bundle → zstd(19) → chunked STREAM cipher` into a single file. The DB
//! snapshot is produced by the caller (a `VACUUM INTO` under the lock, like the export
//! command); everything here is off the DB lock and runs inside `spawn_blocking`.
//!
//! The write is staged to a temp file in the destination directory and atomically
//! `persist`ed over the final path, so a crash never leaves a half-written archive
//! where a real backup should be.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305};

use crate::error::{Error, Result};
use crate::secret::Secret;
use crate::vault::kdf;
use crate::vault::{self, KeyMode, MarkdownEncryption, VaultMeta};

use super::{bundle, format, manifest, BackupPhase, ProgressReader};

/// zstd compression level — high, since a backup is cold storage and we optimise ratio
/// over speed (the whole run happens in the background).
const ZSTD_LEVEL: i32 = 19;
/// Argon2id calibration target for stretching the backup passphrase (~mid of the
/// 250–500 ms unlock band, matching the vault's own creation target).
const KDF_TARGET_MS: u64 = 350;

/// Everything `pack` needs, gathered by the command while it still holds the vault
/// context. `db_snapshot` is a consistent `VACUUM INTO` copy (archived as `pm.sqlite`).
pub struct PackInputs {
    pub vault_root: PathBuf,
    pub db_snapshot: PathBuf,
    pub markdown_dir: PathBuf,
    pub meta: VaultMeta,
    /// The 64-hex SQLCipher key (= hex of the master), embedded so the archive is portable.
    pub db_key_hex: Secret,
    pub app_version: String,
    pub created_at: String,
}

/// One archive member: synthesized bytes (the manifest) or a file on disk.
enum Source {
    Bytes {
        path: String,
        data: Vec<u8>,
    },
    File {
        path: String,
        disk: PathBuf,
        len: u64,
    },
}

impl Source {
    fn len(&self) -> u64 {
        match self {
            Source::Bytes { data, .. } => data.len() as u64,
            Source::File { len, .. } => *len,
        }
    }
}

/// Produce the `.pmbackup` at `dest`. `report(phase, fraction)` is called during the
/// pack; `cancel` aborts between reads. Blocking — call inside `spawn_blocking`.
pub fn pack(
    inputs: PackInputs,
    dest: &Path,
    passphrase: &str,
    mut report: impl FnMut(BackupPhase, f32),
    cancel: &AtomicBool,
) -> Result<()> {
    let sources = enumerate_sources(&inputs)?;
    let total: u64 = sources.iter().map(Source::len).sum();

    // Stretch the backup passphrase with a fresh salt; pick a fresh STREAM nonce prefix.
    let salt: [u8; kdf::SALT_LEN] = vault::random_array()?;
    let params = kdf::calibrate(KDF_TARGET_MS);
    let key = kdf::derive_master(passphrase, &salt, &params)?;
    let nonce_prefix: [u8; format::NONCE_PREFIX_LEN] = vault::random_array()?;

    let header = format::Header {
        format_version: format::FORMAT_VERSION,
        cipher: format::CIPHER_ID.to_string(),
        kdf: format::KdfBlock {
            params,
            kdf_salt_b64: B64.encode(salt),
        },
        stream_nonce_prefix_b64: B64.encode(nonce_prefix),
        chunk_size: format::DEFAULT_CHUNK_SIZE,
        compression: format::COMPRESSION_ID.to_string(),
        created_at: inputs.created_at.clone(),
    };
    let flags = format::FLAG_DB_KEY_EMBEDDED
        | md_flag(inputs.meta.markdown.encryption)
        | key_mode_flag(inputs.meta.key_mode);

    // Stage in the destination directory so the final `persist` is a same-volume rename.
    let dest_parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
    let dest_parent = dest_parent.unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dest_parent)?;
    let tmp = tempfile::Builder::new()
        .prefix(".pmbackup-")
        .tempfile_in(dest_parent)?;

    {
        let mut file = tmp.as_file().try_clone()?;
        let header_json = format::write_header(&mut file, flags, &header)?;
        let aad = format::aad(flags, &header_json);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));
        let aead_w =
            format::ChunkedAeadWriter::new(file, cipher, &nonce_prefix, aad, header.chunk_size)?;
        let mut zstd_w = zstd::stream::write::Encoder::new(aead_w, ZSTD_LEVEL)?;

        bundle::write_header(&mut zstd_w, sources.len() as u32)?;

        let mut done: u64 = 0;
        let mut last_pct: i32 = -1;
        for src in &sources {
            match src {
                Source::Bytes { path, data } => {
                    let mut pr = ProgressReader {
                        inner: &data[..],
                        done: &mut done,
                        total,
                        last_pct: &mut last_pct,
                        phase: BackupPhase::Pack,
                        report: &mut report,
                        cancel,
                    };
                    bundle::write_entry(&mut zstd_w, path, data.len() as u64, &mut pr)?;
                }
                Source::File { path, disk, len } => {
                    let f = File::open(disk)?;
                    let mut pr = ProgressReader {
                        inner: f,
                        done: &mut done,
                        total,
                        last_pct: &mut last_pct,
                        phase: BackupPhase::Pack,
                        report: &mut report,
                        cancel,
                    };
                    bundle::write_entry(&mut zstd_w, path, *len, &mut pr)?;
                }
            }
        }

        let aead_w = zstd_w
            .finish()
            .map_err(|e| Error::Other(format!("compression failed: {e}")))?;
        let file = aead_w.finish()?;
        file.sync_all()?;
    }

    tmp.persist(dest)
        .map_err(|e| Error::Other(format!("could not save the backup file: {e}")))?;
    report(BackupPhase::Pack, 1.0);
    Ok(())
}

/// The ordered allow-list of archive members. The manifest goes first so a restore can
/// read it early; only the vault's durable, portable files follow. Regenerable
/// (`runtime/`) and per-profile (`vault-pointer.json`, WAL/SHM sidecars, `.pm-*` stamps)
/// state is excluded by construction — this is an allow-list, not a copy-with-denylist.
fn enumerate_sources(inputs: &PackInputs) -> Result<Vec<Source>> {
    let mut out = Vec::new();

    let m = manifest::BackupManifest {
        schema: manifest::SCHEMA,
        source_vault_id: inputs.meta.vault_id.clone(),
        source_key_mode: key_mode_str(inputs.meta.key_mode).to_string(),
        db_key_hex: inputs.db_key_hex.expose().to_string(),
        source_markdown_encryption: md_str(inputs.meta.markdown.encryption).to_string(),
        app_version: inputs.app_version.clone(),
        created_at: inputs.created_at.clone(),
    };
    let mbytes = serde_json::to_vec(&m)
        .map_err(|e| Error::Other(format!("could not encode the backup manifest: {e}")))?;
    out.push(Source::Bytes {
        path: manifest::MANIFEST_ENTRY.to_string(),
        data: mbytes,
    });

    // vault-meta.json + the DB snapshot are required; the two always-encrypted sidecars
    // are optional (a fresh vault may not have written them yet).
    //
    // The archive ENTRY names are the layout constants, not independent wire-format strings: a
    // restore rebuilds each entry verbatim under its staging root and then renames that root into
    // place, after which `vault::resolve_layout` opens `<root>/pm.sqlite` and `load_meta` reads
    // `<root>/vault-meta.json`. The names are therefore already forced equal to the on-disk ones —
    // the two sidecars below have always spelled it this way. Consequence to keep in view: renaming
    // [`vault::DB_FILENAME`] or [`vault::META_FILENAME`] changes the archive format, so it needs a
    // `manifest::SCHEMA` decision, not just a rename.
    push_required(
        &mut out,
        &inputs.vault_root.join(vault::META_FILENAME),
        vault::META_FILENAME,
    )?;
    push_required(&mut out, &inputs.db_snapshot, vault::DB_FILENAME)?;
    push_optional(
        &mut out,
        &inputs.vault_root.join(crate::entities::RULES_FILENAME),
        crate::entities::RULES_FILENAME,
    )?;
    push_optional(
        &mut out,
        &inputs.vault_root.join(crate::index_only::MANIFEST_FILENAME),
        crate::index_only::MANIFEST_FILENAME,
    )?;

    // The same NotFound-vs-Err split as `push_optional` above, and for a much larger stake. `is_dir()`
    // is `metadata(..).map(..).unwrap_or(false)`, so a permission denial, an I/O error or a synced
    // share dropping mid-run read as "this vault has no Markdown" — and the archive then packed,
    // verified and reported success with the ENTIRE vault missing. A backup may only omit what it can
    // prove is not there.
    match std::fs::metadata(&inputs.markdown_dir) {
        Ok(m) if m.is_dir() => collect_tree(&mut out, &inputs.markdown_dir, "vault")?,
        // Provably nothing to pack: no vault folder yet, or something that is not a directory sitting
        // under the name (which `is_dir()` also skipped, and which `collect_tree` could not walk).
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::Other(format!(
                "backup could not read the vault folder: {e}"
            )))
        }
    }
    Ok(out)
}

fn push_required(out: &mut Vec<Source>, disk: &Path, name: &str) -> Result<()> {
    let len = std::fs::metadata(disk)
        .map_err(|e| Error::Other(format!("backup could not read {name}: {e}")))?
        .len();
    out.push(Source::File {
        path: name.to_string(),
        disk: disk.to_path_buf(),
        len,
    });
    Ok(())
}

fn push_optional(out: &mut Vec<Source>, disk: &Path, name: &str) -> Result<()> {
    match std::fs::metadata(disk) {
        Ok(m) => out.push(Source::File {
            path: name.to_string(),
            disk: disk.to_path_buf(),
            len: m.len(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Other(format!("backup could not read {name}: {e}"))),
    }
    Ok(())
}

/// Recursively add a directory under `prefix`, using `/`-separated relative paths. File
/// names must be valid UTF-8 (the vault names files by content hash, so they always are;
/// a stray non-UTF-8 name is refused rather than silently mangled).
fn collect_tree(out: &mut Vec<Source>, dir: &Path, prefix: &str) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            Error::Other("a vault file has a non-UTF-8 name; cannot back it up".into())
        })?;
        let rel = format!("{prefix}/{name}");
        let path = entry.path();
        if path.is_dir() {
            collect_tree(out, &path, &rel)?;
        } else {
            let len = entry.metadata()?.len();
            out.push(Source::File {
                path: rel,
                disk: path,
                len,
            });
        }
    }
    Ok(())
}

fn key_mode_str(mode: KeyMode) -> &'static str {
    match mode {
        KeyMode::Device => "device",
        KeyMode::Passphrase => "passphrase",
    }
}

fn md_str(enc: MarkdownEncryption) -> &'static str {
    match enc {
        MarkdownEncryption::None => "none",
        MarkdownEncryption::XChaCha20Poly1305 => "xchacha20poly1305",
    }
}

fn md_flag(enc: MarkdownEncryption) -> u16 {
    match enc {
        MarkdownEncryption::None => 0,
        MarkdownEncryption::XChaCha20Poly1305 => format::FLAG_SOURCE_MD_ENCRYPTED,
    }
}

fn key_mode_flag(mode: KeyMode) -> u16 {
    match mode {
        KeyMode::Device => 0,
        KeyMode::Passphrase => format::FLAG_SOURCE_KEY_MODE_PASSPHRASE,
    }
}
