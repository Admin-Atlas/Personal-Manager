// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Encrypted, portable backup archives (`.pmbackup`) — the "push-out snapshot" the
//! spec files under encrypted cloud backup (distinct from live sync, and not an
//! index-only source). A backup is:
//!
//!   inner bundle  →  zstd level-19  →  chunked XChaCha20-Poly1305 STREAM
//!
//! (compress THEN encrypt — encrypted bytes are high-entropy and won't compress).
//! The archive is self-contained and restorable on any machine: a user backup
//! passphrase is stretched with Argon2id (reusing the vault's [`crate::vault::kdf`]),
//! and the source vault's raw DB key is embedded *inside* the encrypted layer so
//! `pm.sqlite` (SQLCipher) — and everything keyed off the same master — opens after a
//! restore. Confidentiality therefore collapses to the backup passphrase; that is the
//! intended trade for portability, and strictly better than the same-machine-only
//! `export_all_data`, which can't be restored elsewhere at all.
//!
//! The header (magic, KDF salt+params, nonce prefix) is cleartext so the key can be
//! derived before any decryption, but it is bound to the payload as AAD (`blake3` of
//! the header JSON) so it can't be swapped or downgraded without failing auth.
//!
//! The compress→encrypt core produces a destination-agnostic archive on disk; where it goes
//! afterwards is a [`destination::BackupDestination`] — today Proton Drive (via its CLI) or
//! Google Drive (via the Drive v3 REST API). Naming/validation/retention selection is shared
//! (`naming`), so both destinations name and trim archives identically.

pub mod bundle;
pub mod destination;
pub mod format;
pub mod gdrive;
pub mod manifest;
pub mod naming;
pub mod pack;
pub mod proton;
pub mod restore;
pub mod schedule;

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// A `Read` adapter that meters bytes for the progress bar and honours the cancel flag.
/// Wraps each input file during pack, and the archive file during restore. Reports a
/// monotonic `0.0..=1.0` fraction of `total`, throttled to whole-percent changes so the
/// UI channel isn't flooded. A set cancel flag surfaces as an `io::Error` that unwinds
/// the whole pipeline (the command then reports it as cancelled).
pub(crate) struct ProgressReader<'a, R: Read> {
    pub(crate) inner: R,
    pub(crate) done: &'a mut u64,
    pub(crate) total: u64,
    pub(crate) last_pct: &'a mut i32,
    pub(crate) phase: BackupPhase,
    pub(crate) report: &'a mut dyn FnMut(BackupPhase, f32),
    pub(crate) cancel: &'a AtomicBool,
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        *self.done += n as u64;
        // `checked_div` folds in the `total > 0` guard (total is 0 when we can't meter).
        if let Some(pct) = self.done.saturating_mul(100).checked_div(self.total) {
            let pct = pct as i32;
            if pct != *self.last_pct {
                *self.last_pct = pct;
                let frac = (*self.done as f32 / self.total as f32).clamp(0.0, 1.0);
                (self.report)(self.phase, frac);
            }
        }
        Ok(n)
    }
}

/// Which stage a backup or restore is in — drives the single progress bar in the UI
/// (rendered in percent mode, like the Python/t-SNE downloads).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupPhase {
    /// `VACUUM INTO` a consistent, encrypted DB snapshot (held briefly under the lock).
    Snapshot,
    /// Stream the bundle through zstd + the STREAM cipher into the archive.
    Pack,
    /// Push the finished archive to the cloud (PR2).
    Upload,
    /// Pull an archive from the cloud (PR2).
    Download,
    /// Decrypt → decompress → unbundle into a validated staging area.
    Restore,
    /// Open the restored DB with the embedded key and run an integrity check.
    Validate,
}

/// Whether an operation was a backup (out) or a restore (in) — carried on the report.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    Backup,
    Restore,
}

/// What a retention pass actually managed to do: how many archives it trimmed, and how many it
/// selected but the destination refused to let it touch.
///
/// A refusal is not an error. Google Drive grants PM the narrow `drive.file` scope, whose write
/// authority is per (user, OAuth client, file) — PM may only modify files *its own client created*.
/// Revoking and re-granting access, or signing in under a different client, leaves earlier archives
/// perfectly visible and listable while refusing every write to them. Counting refusals separately
/// is what lets a pass trim what it can and explain the rest, instead of failing at the first one
/// and trimming nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RetentionOutcome {
    /// Moved to the destination's trash (recoverable, never a hard delete).
    pub trashed: usize,
    /// Selected for trimming, but the destination refused PM write access to them.
    pub skipped: usize,
}

/// The result of a finished backup/restore, kept in the shared snapshot so a user who
/// navigates away and back still sees the outcome (mirrors `CloudSyncReport`).
#[derive(Debug, Clone, Serialize)]
pub struct BackupReport {
    pub kind: BackupKind,
    /// The vault this archive belongs to (source for a backup, restored for a restore).
    pub vault_id: Option<String>,
    /// Where a restore materialized the vault (absolute path), so the UI can offer
    /// "switch to it now". `None` for a backup.
    pub target_dir: Option<String>,
    /// The archive's creation timestamp (RFC3339), surfaced on restore.
    pub created_at: Option<String>,
    /// Destinations that failed this run while at least one other succeeded (F-22), as
    /// `"<label>: <error>"` strings — the UI shows a non-blocking "backed up, but X failed" banner.
    /// Empty on a clean run and always empty for a restore.
    ///
    /// Genuine UPLOAD failures only. A keep-last-N prune that could not trim is NOT one of these —
    /// it used to be pushed into the same vec, so a destination whose archive uploaded perfectly
    /// was reported as "destination failed". Those go in `retention_notes`.
    #[serde(default)]
    pub failed_destinations: Vec<String>,
    /// Retention (keep-last-N) trouble on a destination whose upload SUCCEEDED: either older
    /// archives that could not be trimmed, or a trim that errored. Reported in its own sentence,
    /// never under the "destination failed" headline, because the backup did reach the destination.
    #[serde(default)]
    pub retention_notes: Vec<RetentionNote>,
}

/// One destination's retention trouble. Carries `BackupDestination::kind()` — the stable machine
/// key — rather than the user-facing label, so the UI can match a note against that destination's
/// live listing without string-matching a name that is allowed to be reworded.
#[derive(Debug, Clone, Serialize)]
pub struct RetentionNote {
    /// `"proton"` / `"gdrive"` — `BackupDestination::kind()`.
    pub kind: String,
    /// Ready-to-show sentence, already naming the destination.
    pub message: String,
    /// Whether this is a count fact (archives are over the limit and could not be trimmed) rather
    /// than a transport failure. Only a count fact may be auto-suppressed once a fresh listing
    /// shows the destination back under its limit — a failed trim is not healed by the count.
    pub over_limit: bool,
}

/// A progress event broadcast on the global `backup://progress` channel. Detached from
/// the view that started the op (like the Drive sync), so progress survives navigation.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackupEvent {
    /// Monotonic `0.0..=1.0` progress within `phase`.
    Phase { phase: BackupPhase, fraction: f32 },
    /// The op finished successfully.
    Finished { report: BackupReport },
    /// The op failed (or was cancelled); `message` is user-facing.
    Failed { message: String },
}
