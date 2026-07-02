// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! macOS runtime download of a standalone Python interpreter.
//!
//! On Windows the app ships a bundled interpreter (`scripts/fetch-python.mjs` +
//! `tauri.windows.conf.json`), so the venv is built without any system Python. On
//! macOS we can't do that yet — `python-build-standalone` has no universal2 build
//! and bundling unsigned dylibs inside the signed `.app` needs the Apple-signing
//! pipeline (see `docs/MACOS-SIGNING.md`). So the fallback here is the other half
//! of the same idea: when [`crate::sidecar::SidecarManager::resolve_base_python`]
//! probes the machine and finds **no** interpreter ≥ 3.10, it downloads one at
//! runtime into PM's data-home (a subprocess download into a user directory, not
//! a build-time bundle into the signed app — so it isn't gated on signing, and a
//! `reqwest` download carries no `com.apple.quarantine` flag).
//!
//! This mirrors `fetch-python.mjs`'s discipline exactly: the version, per-arch
//! asset, and SHA-256 are pinned below; the download is verified against that hash
//! before it is unpacked (no trust-on-first-use); a stamp file makes a second run
//! skip the work. The pinned interpreter version is deliberately exact — it's the
//! one validated in CI against markitdown/fastembed/faster-whisper — unlike the
//! *probe*, which accepts any ≥ 3.10 already on the machine.
//!
//! macOS-only in practice: [`fetch_macos_python`] is only ever called from the
//! `cfg!(target_os = "macos")` arm of `resolve_base_python`. The pure helpers are
//! platform-independent so they stay unit-testable on the Windows/Linux CI — hence
//! the module compiles everywhere, and its items are legitimately unused on
//! non-macOS non-test builds (the `dead_code` allow below, scoped to exactly that).

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

// ---- the pin -------------------------------------------------------------
// Kept in lockstep with `scripts/fetch-python.mjs` (Windows uses the same tag +
// version; only the per-arch asset/SHA differ). To advance: pick a new
// python-build-standalone release that ships BOTH macOS `install_only` assets
// *and* the Windows one, and update every field from its signed SHA256SUMS.
// The three SHA-256 values below are the real ones from the release's SHA256SUMS
// (the Windows one matches `fetch-python.mjs`'s pin exactly, cross-checking the
// file's authenticity).
const PY_VERSION: &str = "3.12.13";
const PBS_TAG: &str = "20260610";

const ASSET_AARCH64: &str = "cpython-3.12.13+20260610-aarch64-apple-darwin-install_only.tar.gz";
const SHA256_AARCH64: &str = "e18ddd4c1e8f4a1d6c4590b37f423d76aec734447edc20ed08e93983d95f2132";

const ASSET_X86_64: &str = "cpython-3.12.13+20260610-x86_64-apple-darwin-install_only.tar.gz";
const SHA256_X86_64: &str = "ba02164e4db381af8c288c0bc1657584a835e9121a0fa2836b0f2e712ff8cdf5";

/// GitHub Releases base; the asset URL is `{BASE}/{PBS_TAG}/{ASSET}` (same shape
/// `fetch-python.mjs` builds).
const RELEASE_BASE: &str = "https://github.com/astral-sh/python-build-standalone/releases/download";

/// Stamp filename inside the destination dir — same name `fetch-python.mjs` uses,
/// so the two provisioning paths are recognisable as siblings.
const STAMP_FILE: &str = ".pm-pyver";

/// Pick the asset + expected hash for a given arch string. Pure (takes the arch
/// explicitly) so both branches are unit-testable without cross-compiling. Rosetta
/// is a non-issue: a single-arch binary reports its *own* arch via
/// `std::env::consts::ARCH`, which is what must match the interpreter we run.
fn macos_asset_for(arch: &str) -> (&'static str, &'static str) {
    match arch {
        "aarch64" => (ASSET_AARCH64, SHA256_AARCH64),
        // x86_64 (and any unexpected value, which can't happen on a macOS build
        // target PM ships) falls back to the Intel asset.
        _ => (ASSET_X86_64, SHA256_X86_64),
    }
}

/// The asset + hash for the running binary's architecture.
fn macos_asset() -> (&'static str, &'static str) {
    macos_asset_for(std::env::consts::ARCH)
}

/// The interpreter path inside an unpacked `install_only` archive: it extracts to
/// a top-level `python/` dir, and on macOS/Unix the interpreter is `bin/python3`
/// (the Windows layout is `python/python.exe` — see `bundled_python`).
fn interpreter_path(dest_dir: &Path) -> PathBuf {
    dest_dir.join("python").join("bin").join("python3")
}

/// Identity written to the stamp file: version+tag+hash, so ANY pin change (new
/// version, tag, or a different arch's asset) invalidates a previous download and
/// forces a re-fetch — exactly like `fetch-python.mjs`'s `STAMP`.
fn stamp_for(sha256: &str) -> String {
    format!("{PY_VERSION}+{PBS_TAG} {sha256}")
}

/// If a previously downloaded interpreter is present at `dest_dir` and its stamp
/// matches the current pin, return its path (skip the download). Pure except for
/// reading the stamp file and an existence check — no network, so it's hermetically
/// testable. Returns `None` when nothing is there or the stamp is stale.
fn downloaded_python_current(dest_dir: &Path, stamp: &str) -> Option<PathBuf> {
    let stamped = std::fs::read_to_string(dest_dir.join(STAMP_FILE)).ok()?;
    let interp = interpreter_path(dest_dir);
    (stamped.trim() == stamp && interp.exists()).then_some(interp)
}

/// Download, verify, and unpack the pinned macOS interpreter into `dest_dir`,
/// returning the interpreter path. Idempotent: a matching stamp short-circuits the
/// whole thing. Blocking — must be called off the async runtime (it is: the only
/// caller is `provision()`, already inside `spawn_blocking`). `on_progress` gets a
/// monotonic `0.0..=1.0` fraction from the download's byte count (skipped if the
/// server sends no `Content-Length`).
pub(crate) fn fetch_macos_python(
    dest_dir: &Path,
    mut on_progress: impl FnMut(f32),
) -> Result<PathBuf> {
    let (asset, sha256) = macos_asset();
    let stamp = stamp_for(sha256);

    if let Some(p) = downloaded_python_current(dest_dir, &stamp) {
        return Ok(p);
    }

    let url = format!("{RELEASE_BASE}/{PBS_TAG}/{asset}");

    // Download to a temp file, hashing on the fly, so a truncated/tampered archive
    // fails BEFORE we unpack it. reqwest::blocking is fine here — we're on a
    // spawn_blocking worker with no entered async runtime.
    let tmp = tempfile::Builder::new().prefix("pm-python-").tempdir()?;
    let archive = tmp.path().join(asset);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;
    let mut resp = client.get(&url).send()?.error_for_status()?;
    let total = resp.content_length();

    {
        let mut file = std::fs::File::create(&archive)?;
        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            downloaded += n as u64;
            if let Some(total) = total.filter(|t| *t > 0) {
                on_progress((downloaded as f32 / total as f32).clamp(0.0, 1.0));
            }
        }
        file.flush()?;

        let got = hex::encode(hasher.finalize());
        if got != sha256 {
            return Err(Error::Other(format!(
                "the downloaded Python interpreter failed its integrity check — the download was \
                 corrupted or blocked. Expected SHA-256 {sha256}, got {got}."
            )));
        }
    }

    // Replace any stale/partial interpreter dir wholesale, then unpack. Shell out to
    // the system tar (present on every macOS by absolute path — matches what
    // fetch-python.mjs does on Windows and adds no Rust tar/gzip dependency).
    match std::fs::remove_dir_all(dest_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    std::fs::create_dir_all(dest_dir)?;

    let status = Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .map_err(|e| {
            Error::Other(format!(
                "could not run tar to unpack the Python interpreter: {e}"
            ))
        })?;
    if !status.success() {
        return Err(Error::Other(
            "tar failed to unpack the downloaded Python interpreter.".to_string(),
        ));
    }

    let interp = interpreter_path(dest_dir);
    if !interp.exists() {
        return Err(Error::Other(format!(
            "unpacked the Python archive but {} is missing — unexpected archive layout.",
            interp.display()
        )));
    }

    // Stamp last, so a run interrupted mid-unpack re-does the work next time rather
    // than trusting a half-written dir.
    std::fs::write(dest_dir.join(STAMP_FILE), format!("{stamp}\n"))?;
    on_progress(1.0);
    Ok(interp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_selection_is_arch_specific() {
        assert_eq!(macos_asset_for("aarch64"), (ASSET_AARCH64, SHA256_AARCH64));
        assert_eq!(macos_asset_for("x86_64"), (ASSET_X86_64, SHA256_X86_64));
        // An unexpected arch defaults to the Intel asset rather than panicking.
        assert_eq!(macos_asset_for("weird"), (ASSET_X86_64, SHA256_X86_64));
    }

    #[test]
    fn stamp_encodes_version_tag_and_hash() {
        let s = stamp_for(SHA256_AARCH64);
        assert!(s.contains(PY_VERSION));
        assert!(s.contains(PBS_TAG));
        assert!(s.contains(SHA256_AARCH64));
        // A different pin (arch/hash) produces a different stamp, so it invalidates.
        assert_ne!(stamp_for(SHA256_AARCH64), stamp_for(SHA256_X86_64));
    }

    #[test]
    fn interpreter_path_is_install_only_layout() {
        let dest = PathBuf::from("/data/runtime/python-standalone");
        assert_eq!(
            interpreter_path(&dest),
            PathBuf::from("/data/runtime/python-standalone/python/bin/python3")
        );
    }

    #[test]
    fn downloaded_python_current_matches_only_on_fresh_stamp_and_interpreter() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path();
        let stamp = stamp_for(SHA256_AARCH64);

        // (a) nothing there yet → None.
        assert_eq!(downloaded_python_current(dest, &stamp), None);

        // A fake interpreter standing in for the real one (like the bundled_python tests).
        let interp = interpreter_path(dest);
        std::fs::create_dir_all(interp.parent().unwrap()).unwrap();
        std::fs::write(&interp, b"").unwrap();

        // (b) interpreter present but stamp file absent → None.
        assert_eq!(downloaded_python_current(dest, &stamp), None);

        // (c) matching stamp + interpreter → Some(path).
        std::fs::write(dest.join(STAMP_FILE), format!("{stamp}\n")).unwrap();
        assert_eq!(
            downloaded_python_current(dest, &stamp),
            Some(interp.clone())
        );

        // (d) stale stamp (a pin bump) → None, forcing a re-download.
        assert_eq!(
            downloaded_python_current(dest, &stamp_for(SHA256_X86_64)),
            None
        );
    }
}
