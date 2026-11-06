// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! macOS runtime download of a standalone Python interpreter.
//!
//! On Windows the app ships a bundled interpreter (`scripts/fetch-python.mjs` +
//! `tauri.windows.conf.json`), so the venv is built without any system Python. On
//! macOS we can't do that yet — bundling unsigned dylibs inside the signed `.app`
//! needs the Apple-signing pipeline (see `docs/MACOS-SIGNING.md`). So the fallback
//! here is the other half of the same idea: when [`crate::sidecar::SidecarManager::resolve_base_python`]
//! probes the machine and finds **no** interpreter ≥ 3.11, it downloads one at
//! runtime into PM's data-home (a subprocess download into a user directory, not
//! a build-time bundle into the signed app — so it isn't gated on signing, and a
//! `reqwest` download carries no `com.apple.quarantine` flag).
//!
//! This mirrors `fetch-python.mjs`'s discipline exactly: the version, asset, and
//! SHA-256 are pinned below; the download is verified against that hash
//! before it is unpacked (no trust-on-first-use); a stamp file makes a second run
//! skip the work. The pinned interpreter version is deliberately exact — it's the
//! one validated in CI against markitdown/fastembed — unlike the
//! *probe*, which accepts any ≥ 3.11 already on the machine.
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
// version; only the asset/SHA differ). To advance: pick a new
// python-build-standalone release that ships the macOS arm64 `install_only` asset
// *and* the Windows one, and update every field from its signed SHA256SUMS.
// The SHA-256 below is the real one from the release's SHA256SUMS.
//
// Apple Silicon only, like PM's Mac build (release.yml builds aarch64-apple-darwin
// alone, for macOS 14 or newer): onnxruntime, which runs the document engine's
// search and photo models, publishes no Intel Mac wheel, so an Intel interpreter
// could never get as far as a working engine. There is no Intel pin to keep current.
const PY_VERSION: &str = "3.12.13";
const PBS_TAG: &str = "20260610";

const ASSET_AARCH64: &str = "cpython-3.12.13+20260610-aarch64-apple-darwin-install_only.tar.gz";
const SHA256_AARCH64: &str = "e18ddd4c1e8f4a1d6c4590b37f423d76aec734447edc20ed08e93983d95f2132";

/// GitHub Releases base; the asset URL is `{BASE}/{PBS_TAG}/{ASSET}` (same shape
/// `fetch-python.mjs` builds).
const RELEASE_BASE: &str = "https://github.com/astral-sh/python-build-standalone/releases/download";

/// Stamp filename inside the destination dir — same name `fetch-python.mjs` uses,
/// so the two provisioning paths are recognisable as siblings.
const STAMP_FILE: &str = ".pm-pyver";

/// Pick the asset + expected hash for a given arch string: the Apple Silicon one, or
/// `None` for any other arch, which PM doesn't ship on macOS (an Intel build made from
/// source). Pure (takes the arch explicitly) so both answers are unit-testable without
/// cross-compiling. Rosetta is a non-issue: a single-arch binary reports its *own*
/// arch via `std::env::consts::ARCH`, which is what must match the interpreter we run.
fn macos_asset_for(arch: &str) -> Option<(&'static str, &'static str)> {
    (arch == "aarch64").then_some((ASSET_AARCH64, SHA256_AARCH64))
}

/// The asset + hash for the running binary's architecture, or why there is none.
fn macos_asset() -> Result<(&'static str, &'static str)> {
    macos_asset_for(std::env::consts::ARCH).ok_or_else(|| {
        Error::Other(
            "PM's document engine needs an Apple Silicon Mac: the engine that runs its search \
             and photo models isn't made for Intel Macs."
                .to_string(),
        )
    })
}

/// The interpreter path inside an unpacked `install_only` archive: it extracts to
/// a top-level `python/` dir, and on macOS/Unix the interpreter is `bin/python3`
/// (the Windows layout is `python/python.exe` — see `bundled_python`).
fn interpreter_path(dest_dir: &Path) -> PathBuf {
    dest_dir.join("python").join("bin").join("python3")
}

/// Identity written to the stamp file: version+tag+hash, so ANY pin change (new
/// version, tag, or asset) invalidates a previous download and
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
    let (asset, sha256) = macos_asset()?;
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
    fn only_apple_silicon_has_an_interpreter() {
        assert_eq!(
            macos_asset_for("aarch64"),
            Some((ASSET_AARCH64, SHA256_AARCH64))
        );
        assert!(ASSET_AARCH64.contains("aarch64-apple-darwin"));
        // Intel, or any other arch, has none: PM on Mac is Apple Silicon only.
        assert_eq!(macos_asset_for("x86_64"), None);
        assert_eq!(macos_asset_for("weird"), None);
    }

    #[test]
    fn stamp_encodes_version_tag_and_hash() {
        let s = stamp_for(SHA256_AARCH64);
        assert!(s.contains(PY_VERSION));
        assert!(s.contains(PBS_TAG));
        assert!(s.contains(SHA256_AARCH64));
        // A different pin (a new hash) produces a different stamp, so it invalidates.
        assert_ne!(stamp_for(SHA256_AARCH64), stamp_for(&"0".repeat(64)));
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
            downloaded_python_current(dest, &stamp_for(&"0".repeat(64))),
            None
        );
    }
}
