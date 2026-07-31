// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Restore a `.pmbackup` — the inverse of [`super::pack`], with a hard invariant: a
//! wrong passphrase, a truncated/tampered archive, a version mismatch, or a crash
//! mid-restore must NEVER touch the live vault. Everything materializes into a temp
//! staging directory and is validated (the DB opens with the embedded key and passes an
//! integrity check; the metadata matches the manifest) before it is promoted, by an
//! atomic rename, to a fresh target directory. Switching the active profile to the
//! restored vault is a separate, explicit step the caller performs afterwards.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305};

use crate::error::{Error, Result};
use crate::secret::Secret;
use crate::vault::kdf;
use crate::{db, vault};

use super::{bundle, format, manifest, BackupPhase, ProgressReader};

/// Cap on the in-memory manifest read — a manifest is tiny JSON, so anything larger is a
/// decompression bomb or corruption. The bundle framing caps the entry at its declared
/// `len`, so rejecting an oversized `len` is a sufficient guard.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Headroom the unpack leaves free on the restore volume. On Windows the vault usually sits on the
/// system volume, where driving free space to literally zero destabilises the page file and every
/// other running app — so a restore that would take the last byte should stop and say so rather than
/// take the machine down with it. Small next to any real vault, large enough to keep the OS healthy.
const RESTORE_FREE_SPACE_RESERVE: u64 = 1024 * 1024 * 1024;

/// Whether `len` more bytes still fit inside `budget` after `unpacked`. Pure so the arithmetic — the
/// part that has to survive a hostile `len` — is testable without a disk. `None` means the free-space
/// probe had no answer, which reads as "no budget", exactly as before the budget existed.
fn fits(unpacked: u64, len: u64, budget: Option<u64>) -> bool {
    match budget {
        Some(budget) => unpacked.saturating_add(len) <= budget,
        None => true,
    }
}

/// Reject Argon2 cost parameters a hostile header could weaponise. The `argon2` crate does
/// not range-check `m_cost`/`t_cost`, so a multi-TiB `m_cost_kib` or a `u32::MAX` `t_cost`
/// would OOM or hang the app *during* derivation — which happens before the header AAD can
/// be checked, so the AAD binding can't save us. These ceilings sit comfortably above
/// `kdf::calibrate`'s strongest tier (256 MiB memory, up to ~10 passes).
fn validate_kdf_params(p: &kdf::KdfParams) -> Result<()> {
    const MAX_M_COST_KIB: u32 = 1024 * 1024; // 1 GiB
    let ok = p.algorithm == "argon2id"
        && p.version == 0x13
        && p.key_len as usize == kdf::KEY_LEN
        && (8..=MAX_M_COST_KIB).contains(&p.m_cost_kib)
        && (1..=16).contains(&p.t_cost)
        && (1..=16).contains(&p.p_cost);
    if !ok {
        return Err(Error::Other(
            "backup uses unsupported or unreasonable key-derivation parameters".into(),
        ));
    }
    Ok(())
}

/// The validated result of a restore. `db_key_hex` is kept in Rust only — the command
/// seeds it into this device's keychain (`vault_key::<id>`) so the restored vault can be
/// opened; it is never returned to the webview.
pub struct RestoreOutcome {
    pub vault_id: String,
    pub key_mode: String,
    pub markdown_encryption: String,
    pub app_version: String,
    pub created_at: String,
    pub target_dir: PathBuf,
    pub db_key_hex: Secret,
}

/// Restore `src` into a fresh `target_dir` using `passphrase`. `target_dir` must not
/// already contain a vault (an existing empty dir is fine and is reused). Blocking —
/// call inside `spawn_blocking`.
pub fn restore(
    src: &Path,
    passphrase: &str,
    target_dir: &Path,
    mut report: impl FnMut(BackupPhase, f32),
    cancel: &AtomicBool,
) -> Result<RestoreOutcome> {
    // Refuse a non-empty target up front, before doing any work.
    if target_dir.exists() {
        let non_empty = std::fs::read_dir(target_dir)?.next().is_some();
        if non_empty {
            return Err(Error::Other(
                "the restore target already contains files; choose an empty folder".into(),
            ));
        }
    }
    let target_parent = target_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::Other("invalid restore target location".into()))?;
    std::fs::create_dir_all(target_parent)?;

    // --- header (cleartext) → derive key --------------------------------------------
    let mut file =
        File::open(src).map_err(|_| Error::Other("could not open the backup file".into()))?;
    let (flags, header_json, header) = format::read_header(&mut file)?;
    if header.cipher != format::CIPHER_ID {
        return Err(Error::Other("unsupported backup cipher".into()));
    }
    if header.compression != format::COMPRESSION_ID {
        return Err(Error::Other("unsupported backup compression".into()));
    }
    let header_len = 16 + header_json.len() as u64; // magic(8)+ver(2)+flags(2)+len(4)+json

    validate_kdf_params(&header.kdf.params)?;
    let salt = B64
        .decode(&header.kdf.kdf_salt_b64)
        .map_err(|e| Error::Other(format!("corrupt backup salt: {e}")))?;
    let key = kdf::derive_master(passphrase, &salt, &header.kdf.params)?;
    let nonce_prefix = B64
        .decode(&header.stream_nonce_prefix_b64)
        .map_err(|e| Error::Other(format!("corrupt backup nonce: {e}")))?;
    let aad = format::aad(flags, &header_json);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));

    // Progress is metered on archive (ciphertext) bytes consumed, whose total we know.
    let archive_len = std::fs::metadata(src)?.len();
    let payload_len = archive_len.saturating_sub(header_len);
    let mut done: u64 = 0;
    let mut last_pct: i32 = -1;
    let metered = ProgressReader {
        inner: file,
        done: &mut done,
        total: payload_len,
        last_pct: &mut last_pct,
        phase: BackupPhase::Restore,
        report: &mut report,
        cancel,
    };

    // --- decrypt → decompress → unbundle into staging -------------------------------
    let aead_r =
        format::ChunkedAeadReader::new(metered, cipher, &nonce_prefix, aad, header.chunk_size)?;
    let zstd_r = zstd::stream::read::Decoder::new(aead_r)
        .map_err(|e| Error::Other(format!("could not start decompression: {e}")))?;

    let staging = tempfile::Builder::new()
        .prefix("pm-restore-")
        .tempdir_in(target_parent)?;
    let staging_path = staging.path().to_path_buf();

    // What the unpack is allowed to write. Nothing downstream bounds it: `bundle::read_bundle`
    // streams each entry straight to disk, so a vault larger than the drive fills it and then fails
    // with a raw OS error. Fails OPEN — a probe with no answer must not be able to block a restore,
    // so an unrecognised volume behaves exactly as it did before there was a budget.
    let budget = crate::hardware::available_bytes(target_parent)
        .map(|free| free.saturating_sub(RESTORE_FREE_SPACE_RESERVE));
    let mut unpacked: u64 = 0;

    let mut manifest_bytes: Option<Vec<u8>> = None;
    bundle::read_bundle(zstd_r, |path, len, content| {
        if path == manifest::MANIFEST_ENTRY {
            // The bundle framing caps `content` at exactly `len` bytes, so refusing an
            // oversized `len` bounds this in-memory read against a decompression bomb.
            if len > MAX_MANIFEST_BYTES {
                return Err(Error::Other("backup manifest is unreasonably large".into()));
            }
            let mut buf = Vec::new();
            content.read_to_end(&mut buf)?;
            manifest_bytes = Some(buf);
            return Ok(());
        }
        // `path` is already validated by read_bundle; rebuild it component-by-component
        // (never trusting the '/' as an OS separator) under the staging root.
        let mut dest = staging_path.clone();
        for comp in path.split('/') {
            dest.push(comp);
        }
        // Checked against the DECLARED `len` before a byte is written, so one oversized entry is
        // refused up front rather than after it has filled the drive; charged by the ACTUAL count
        // afterwards, so a short entry isn't over-billed.
        if !fits(unpacked, len, budget) {
            return Err(Error::Other(
                "there is not enough free space on this drive to restore this backup; free up \
                 space and try again"
                    .into(),
            ));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&dest)?;
        unpacked = unpacked.saturating_add(std::io::copy(content, &mut f)?);
        Ok(())
    })?;

    // --- validate before promoting --------------------------------------------------
    report(BackupPhase::Validate, 0.0);
    let manifest_bytes =
        manifest_bytes.ok_or_else(|| Error::Other("the backup is missing its manifest".into()))?;
    let man: manifest::BackupManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| Error::Other(format!("corrupt backup manifest: {e}")))?;
    if man.schema != manifest::SCHEMA {
        return Err(Error::Other(format!(
            "this backup uses an unsupported manifest version ({})",
            man.schema
        )));
    }

    // The restored DB must open with the embedded key and pass an integrity check.
    let db_path = staging_path.join("pm.sqlite");
    {
        let conn = db::open(&db_path, &man.db_key_hex)
            .map_err(|e| Error::Other(format!("the restored database could not be opened: {e}")))?;
        let status: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| Error::Other(format!("integrity check failed: {e}")))?;
        if status != "ok" {
            return Err(Error::Other(format!(
                "the restored database failed its integrity check: {status}"
            )));
        }
    }
    // The metadata must belong to the same vault the manifest claims.
    let meta = vault::load_meta(&staging_path)?
        .ok_or_else(|| Error::Other("the backup is missing its vault metadata".into()))?;
    if meta.vault_id != man.source_vault_id {
        return Err(Error::Other(
            "backup is inconsistent (metadata vault id does not match the manifest)".into(),
        ));
    }
    report(BackupPhase::Validate, 1.0);

    // --- promote staging → target (atomic; old vault untouched until now) -----------
    // Persist the staging dir (disable auto-delete) so we can rename it into place.
    let staged = staging.keep();
    // A pre-existing empty target dir would make rename fail on some platforms; drop it.
    if target_dir.exists() {
        let _ = std::fs::remove_dir(target_dir);
    }
    if let Err(rename_err) = std::fs::rename(&staged, target_dir) {
        // Cross-volume (or a racing dir): fall back to the shared verified recursive copy,
        // then clean up the persisted staging dir so a decrypted copy isn't left behind.
        // The cleanup runs on the FAILURE path too — `keep()` above turned off the TempDir's own
        // auto-delete, so returning early through `?` here used to leave a fully decrypted vault
        // sitting on disk, which is precisely what the line below exists to prevent.
        let copied = crate::vault::migrate::copy_tree_verified(&staged, target_dir);
        let _ = std::fs::remove_dir_all(&staged);
        copied.map_err(|e| {
            Error::Other(format!(
                "could not place the restored vault: {e} (rename: {rename_err})"
            ))
        })?;
    }

    Ok(RestoreOutcome {
        vault_id: man.source_vault_id,
        key_mode: man.source_key_mode,
        markdown_encryption: man.source_markdown_encryption,
        app_version: man.app_version,
        created_at: man.created_at,
        target_dir: target_dir.to_path_buf(),
        db_key_hex: Secret::from(man.db_key_hex),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::pack::{pack, PackInputs};
    use std::sync::atomic::AtomicBool;

    /// Every durable file a real vault holds, relative to the vault root, with its bytes.
    ///
    /// The fixture used to be a single flat `vault/note.md`, which meant `collect_tree`'s
    /// recursion and both `push_optional` calls in pack.rs were dead code as far as the suite was
    /// concerned — a backup that silently dropped the chats subfolder, the photo originals, or
    /// either encrypted sidecar would have passed all three round-trip tests. The bytes are
    /// deliberately non-UTF-8 for the photo, because a photo original is a byte file and a
    /// text-mode copy would corrupt it invisibly.
    fn vault_contents() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("vault/note.md", b"# hello\n\nbody" as &[u8]),
            // A nested level — the only thing that exercises collect_tree's recursion.
            (
                "vault/chats/chat-01-01-2026-a.md",
                b"# chat\n\n**You:** hi\n",
            ),
            // A photo original: raw bytes, not text, under a second subfolder.
            (
                "vault/photos/0f1e2d3c.png.pmenc",
                &[0x00, 0x01, 0xff, 0xfe, 0x80, 0x0a, 0x0d, 0x1a],
            ),
            // The two always-encrypted sidecars pack.rs pushes optionally.
            (crate::entities::RULES_FILENAME, b"\x01encrypted-rules"),
            (
                crate::index_only::MANIFEST_FILENAME,
                b"\x01encrypted-manifest",
            ),
        ]
    }

    // A device-mode vault fixture: a real (tiny) encrypted DB + a meta + the tree above,
    // packed and then restored, proving the archive is self-describing and portable.
    fn make_source_vault(dir: &Path) -> (vault::VaultMeta, String) {
        std::fs::create_dir_all(dir).unwrap();
        let meta = vault::ensure_device_meta(dir).unwrap();
        // A device vault's DB key is a random 64-hex; make one and open a DB with it.
        let key_hex = "11".repeat(32);
        let db_path = dir.join("pm.sqlite");
        {
            let conn = db::open(&db_path, &key_hex).unwrap();
            conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (42);")
                .unwrap();
        }
        for (rel, bytes) in vault_contents() {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
        }
        (meta, key_hex)
    }

    #[test]
    fn the_unpack_budget_stops_before_it_fills_the_drive() {
        // `read_bundle` streams every entry straight to disk with nothing counting the total, so a
        // vault larger than the volume filled it and then died on a raw OS error. The arithmetic is
        // the part worth pinning: it runs against a DECLARED length, which is the one number a
        // damaged or hostile archive controls, so it must not be able to overflow into a pass.
        assert!(
            fits(0, u64::MAX, None),
            "no probe answer ⇒ no budget, as before"
        );
        assert!(
            fits(900, 100, Some(1000)),
            "exactly filling the budget is allowed"
        );
        assert!(!fits(900, 101, Some(1000)), "one byte past it is not");
        assert!(
            !fits(1, u64::MAX, Some(1000)),
            "a declared length near u64::MAX saturates rather than wrapping to a pass"
        );
        assert!(!fits(0, 1, Some(0)), "a full drive admits nothing");
    }

    #[test]
    fn pack_then_restore_round_trips_and_validates() {
        let root = tempfile::tempdir().unwrap();
        let src_vault = root.path().join("src");
        let (meta, key_hex) = make_source_vault(&src_vault);

        // A separate VACUUM snapshot, exactly as the command produces.
        let snap = root.path().join("snap.sqlite");
        {
            let conn = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }

        let archive = root.path().join("backup.pmbackup");
        let inputs = PackInputs {
            vault_root: src_vault.clone(),
            db_snapshot: snap,
            markdown_dir: src_vault.join("vault"),
            meta: meta.clone(),
            db_key_hex: Secret::from(key_hex.clone()),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
        };
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "correct-horse", |_, _| {}, &cancel).unwrap();

        // Restore into a fresh dir with the right passphrase.
        let target = root.path().join("restored");
        let outcome = restore(&archive, "correct-horse", &target, |_, _| {}, &cancel).unwrap();
        assert_eq!(outcome.vault_id, meta.vault_id);
        assert_eq!(outcome.db_key_hex.expose(), key_hex);

        // The restored DB opens with the embedded key and holds our row.
        let conn = db::open(&target.join("pm.sqlite"), outcome.db_key_hex.expose()).unwrap();
        let x: i64 = conn.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 42);

        // Every durable file came across byte-identically — the nested chat, the photo original's
        // raw bytes, and both encrypted sidecars, not just the one flat markdown file.
        for (rel, expected) in vault_contents() {
            let got = std::fs::read(target.join(rel))
                .unwrap_or_else(|e| panic!("{rel} did not survive the round trip: {e}"));
            assert_eq!(got, expected, "{rel} came back with different bytes");
        }
        // …and the metadata that makes the restored vault self-describing.
        assert!(target.join(vault::META_FILENAME).exists());
    }

    #[test]
    fn wrong_passphrase_fails_and_leaves_no_target() {
        let root = tempfile::tempdir().unwrap();
        let src_vault = root.path().join("src");
        let (meta, key_hex) = make_source_vault(&src_vault);
        let snap = root.path().join("snap.sqlite");
        {
            let conn = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }
        let archive = root.path().join("backup.pmbackup");
        let inputs = PackInputs {
            vault_root: src_vault.clone(),
            db_snapshot: snap,
            markdown_dir: src_vault.join("vault"),
            meta,
            db_key_hex: Secret::from(key_hex),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
        };
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "right", |_, _| {}, &cancel).unwrap();

        let target = root.path().join("restored");
        assert!(restore(&archive, "wrong", &target, |_, _| {}, &cancel).is_err());
        // Nothing was promoted.
        assert!(!target.exists());
    }

    #[test]
    fn truncated_archive_fails() {
        let root = tempfile::tempdir().unwrap();
        let src_vault = root.path().join("src");
        let (meta, key_hex) = make_source_vault(&src_vault);
        let snap = root.path().join("snap.sqlite");
        {
            let conn = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }
        let archive = root.path().join("backup.pmbackup");
        let inputs = PackInputs {
            vault_root: src_vault.clone(),
            db_snapshot: snap,
            markdown_dir: src_vault.join("vault"),
            meta,
            db_key_hex: Secret::from(key_hex),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
        };
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();

        // Chop the archive in half.
        let bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
        let target = root.path().join("restored");
        assert!(restore(&archive, "pw", &target, |_, _| {}, &cancel).is_err());
        assert!(!target.exists());
    }
}
