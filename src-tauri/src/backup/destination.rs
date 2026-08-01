// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a finished `.pmbackup` archive goes after the (destination-agnostic) compress→encrypt
//! core has produced it. A [`BackupDestination`] is one target — today Proton Drive (via its CLI)
//! or Google Drive (via the Drive v3 REST API) — with a uniform async surface: push a blob, list
//! what's there, trim to keep-last-N, pull one back.
//!
//! This is an **enum**, not a trait: there are exactly two implementors, and they have opposite
//! execution models (Proton is a blocking CLI shell-out; Google is native async reqwest). An enum
//! with async inherent methods expresses that with a plain `match` and needs no `async_trait`
//! dependency or `Box<dyn>` erasure. The Proton arm wraps the existing sync `proton::*` functions
//! in `spawn_blocking`; the Google arm calls the async `gdrive::*` functions (ensuring the backup
//! folder first). Adding a third destination is a new variant + arm.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use super::naming::BackupEntry;
use super::RetentionOutcome;
use crate::blocking::spawn_blocking_result;
use crate::error::Result;
use crate::AppState;

/// A single place PM pushes encrypted archives to.
pub enum BackupDestination {
    /// Proton Drive via the official `proton-drive` CLI (path to the located binary).
    Proton { cli: PathBuf },
    /// Google Drive via the Drive v3 REST API, authorized by the account's keychain token key
    /// (`google_oauth_token_drive::<email>`, carrying the `drive.file` grant).
    GoogleDrive { token_key: String },
}

impl BackupDestination {
    /// A short, user-facing name for progress/error copy.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Proton { .. } => "Proton Drive",
            Self::GoogleDrive { .. } => "Google Drive",
        }
    }

    /// A STABLE machine key for this destination (never localised / renamed), used for the
    /// per-destination `last_backup_at:<kind>` stamp (F-22) so a persistently-failing target's
    /// freshness is tracked independently of a sibling that keeps succeeding. Matches the
    /// `backup_proton_*` / `backup_gdrive_*` schedule-setting vocabulary.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Proton { .. } => "proton",
            Self::GoogleDrive { .. } => "gdrive",
        }
    }

    /// Push the finished archive at `local` (whose basename is `archive_name`) to this destination.
    /// `app` is only needed by the Proton arm, which re-derives the shared `backup_cancel` flag
    /// inside its blocking task so a mid-upload Cancel is honoured (F-13); the Google arm ignores it.
    pub async fn upload(&self, app: &AppHandle, local: &Path, archive_name: &str) -> Result<()> {
        match self {
            Self::Proton { cli } => {
                let (cli, local, app) = (cli.clone(), local.to_path_buf(), app.clone());
                spawn_blocking_result("backup", move || {
                    let st = app.state::<AppState>();
                    super::proton::upload_archive(&cli, &local, Some(&st.backup_cancel))
                })
                .await
            }
            Self::GoogleDrive { token_key } => {
                let folder = super::gdrive::ensure_backup_folder(token_key).await?;
                super::gdrive::upload_archive(token_key, local, archive_name, &folder).await
            }
        }
    }

    /// The archives already at this destination (newest first).
    pub async fn list(&self) -> Result<Vec<BackupEntry>> {
        match self {
            Self::Proton { cli } => {
                let cli = cli.clone();
                spawn_blocking_result("backup", move || super::proton::list_archives(&cli)).await
            }
            Self::GoogleDrive { token_key } => {
                let folder = super::gdrive::ensure_backup_folder(token_key).await?;
                super::gdrive::list_archives(token_key, &folder).await
            }
        }
    }

    /// Trim to keep the newest `keep_n` archives whose name carries `prefix` (this vault's). The
    /// outcome separates what was trimmed from what the destination refused to let PM touch — see
    /// [`RetentionOutcome`]; only Google Drive can report a refusal.
    pub async fn apply_retention(&self, keep_n: usize, prefix: &str) -> Result<RetentionOutcome> {
        match self {
            Self::Proton { cli } => {
                let (cli, prefix) = (cli.clone(), prefix.to_string());
                spawn_blocking_result("backup", move || {
                    super::proton::apply_retention(&cli, keep_n, &prefix)
                })
                .await
            }
            Self::GoogleDrive { token_key } => {
                let folder = super::gdrive::ensure_backup_folder(token_key).await?;
                super::gdrive::apply_retention(token_key, &folder, keep_n, prefix).await
            }
        }
    }

    /// Pull the archive named `name` into `dest_dir` (written as `dest_dir/<name>`). `app` is
    /// threaded for the same reason [`Self::upload`] takes it: the Proton arm re-derives the shared
    /// `backup_cancel` flag inside its blocking task so a mid-download Cancel kills and reaps the
    /// CLI child (F-13) instead of waiting out the whole transfer. The Google arm ignores it — the
    /// Drive download takes no cancel flag today (see `gdrive::download_archive`), which is why
    /// Cancel is inert for the length of a Drive restore's Download phase.
    ///
    /// This arm used to hard-code `cancel: None` and carry a comment saying it was never exercised,
    /// because `restore_from_proton` called `proton::download_archive` directly. That made the enum
    /// the weak path: routing the live restore through it as it stood would have silently deleted
    /// PM's only mid-download cancellation. The flag is threaded here so the two are the same path.
    pub async fn download(&self, app: &AppHandle, name: &str, dest_dir: &Path) -> Result<()> {
        match self {
            Self::Proton { cli } => {
                let (cli, name, dest, app) = (
                    cli.clone(),
                    name.to_string(),
                    dest_dir.to_path_buf(),
                    app.clone(),
                );
                spawn_blocking_result("backup", move || {
                    let st = app.state::<AppState>();
                    super::proton::download_archive(&cli, &name, &dest, Some(&st.backup_cancel))
                })
                .await
            }
            Self::GoogleDrive { token_key } => {
                super::gdrive::download_archive(token_key, name, dest_dir).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_is_a_stable_machine_key_per_destination() {
        // F-22: `kind` keys the per-destination `last_backup_at:<kind>` stamp, so it must be stable and
        // never localised (unlike `label`) and must match the schedule's proton/gdrive vocabulary.
        assert_eq!(
            BackupDestination::Proton {
                cli: PathBuf::from("proton-drive")
            }
            .kind(),
            "proton"
        );
        assert_eq!(
            BackupDestination::GoogleDrive {
                token_key: "google_oauth_token_drive::me@x.com".into()
            }
            .kind(),
            "gdrive"
        );
    }

    #[test]
    fn kind_spells_the_restore_scratch_prefixes_the_two_commands_used_to_hardcode() {
        // `commands::restore_scratch_dir` mints `pm-restore-<kind>-`. Both restore commands used to
        // retype that prefix; the literals below are what they said, so this is the pin that makes
        // the substitution a rename-free refactor rather than an eyeballed one.
        assert_eq!(
            format!(
                "pm-restore-{}-",
                BackupDestination::Proton {
                    cli: PathBuf::from("proton-drive")
                }
                .kind()
            ),
            "pm-restore-proton-"
        );
        assert_eq!(
            format!(
                "pm-restore-{}-",
                BackupDestination::GoogleDrive {
                    token_key: "google_oauth_token_drive::me@x.com".into()
                }
                .kind()
            ),
            "pm-restore-gdrive-"
        );
    }
}
