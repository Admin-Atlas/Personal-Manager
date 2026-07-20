// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cross-platform input staging for the confined sidecar worker (#286). Every platform's confinement
//! (Windows AppContainer / macOS sandbox-exec / Linux Landlock) freezes the worker's readable-dir
//! allow-list at launch, so an already-running worker can't be handed read on a fresh arbitrary input
//! path. Each untrusted input file is therefore COPIED into the granted staging dir under a unique
//! name and the request is pointed at the copy; the copy is deleted when the request finishes. Shared
//! here (not per-platform) so the staging behaviour — the load-bearing "the worker sees only this one
//! file" guarantee — can never drift between platforms.
//!
//! Currently only the Windows confined path calls this; the macOS/Linux arms (PR2c/PR2d) will too. So
//! the API is `allow(dead_code)` off Windows for now — the all-platform unit test keeps it honest
//! meanwhile (the acl.rs idiom).
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A staged copy of an untrusted input file inside a granted staging dir, deleted when dropped. The
/// caller keeps it alive for the whole request (including any fetch-and-retry) so the copy outlives the
/// worker's read.
pub struct StagedInput {
    path: PathBuf,
}

impl StagedInput {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Monotonic across the process — the sidecar is single-process and serialized, so a counter gives
/// collision-free staged names without pulling in a uuid dependency, and each copy is deleted right
/// after its request anyway.
static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Copy `src` into `staging_dir` under a unique `in-{n}{.ext}` name the confined worker can read,
/// preserving the extension so the worker's format sniffing (which keys off it) is unaffected. Returns
/// the guard whose `path()` the request should be rewritten to. `io::Error` on a copy failure — the
/// caller turns that into a coded fall-back (see `sbx::STAGE_COPY`).
pub fn stage_into(staging_dir: &Path, src: &Path) -> std::io::Result<StagedInput> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let n = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let dst = staging_dir.join(format!("in-{n}{ext}"));
    std::fs::copy(src, &dst)?;
    Ok(StagedInput { path: dst })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs on EVERY platform (per the acl.rs idiom): the staging contract is identical everywhere, so
    /// testing it here keeps Windows/macOS/Linux honest even though only the host arm is CI-compiled.
    #[test]
    fn stages_a_copy_preserving_ext_and_deletes_on_drop() {
        let dir = std::env::temp_dir().join(format!("pm_stage_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("orig.PDF");
        std::fs::write(&src, b"payload").unwrap();

        let staged_path = {
            let staged = stage_into(&dir, &src).expect("stage");
            let p = staged.path().to_path_buf();
            assert!(p.exists(), "staged copy should exist during the request");
            assert_eq!(p.extension().and_then(|e| e.to_str()), Some("PDF"));
            assert!(
                p.file_name().unwrap().to_str().unwrap().starts_with("in-"),
                "staged name: {p:?}"
            );
            assert_eq!(std::fs::read(&p).unwrap(), b"payload");
            p
        };
        assert!(
            !staged_path.exists(),
            "staged copy must be deleted when the guard drops"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two stages never collide (monotonic counter).
    #[test]
    fn distinct_names_across_stages() {
        let dir = std::env::temp_dir().join(format!("pm_stage_seq_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.txt");
        std::fs::write(&src, b"x").unwrap();
        let a = stage_into(&dir, &src).unwrap();
        let b = stage_into(&dir, &src).unwrap();
        assert_ne!(a.path(), b.path());
        std::fs::remove_dir_all(&dir).ok();
    }
}
