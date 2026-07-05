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
use crate::error::{Error, Result};
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

    /// Push the finished archive at `local` (whose basename is `archive_name`) to this destination.
    /// `app` is only needed by the Proton arm, which re-derives the shared `backup_cancel` flag
    /// inside its blocking task so a mid-upload Cancel is honoured (F-13); the Google arm ignores it.
    pub async fn upload(&self, app: &AppHandle, local: &Path, archive_name: &str) -> Result<()> {
        match self {
            Self::Proton { cli } => {
                let (cli, local, app) = (cli.clone(), local.to_path_buf(), app.clone());
                spawn_blocking_result(move || {
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
                spawn_blocking_result(move || super::proton::list_archives(&cli)).await
            }
            Self::GoogleDrive { token_key } => {
                let folder = super::gdrive::ensure_backup_folder(token_key).await?;
                super::gdrive::list_archives(token_key, &folder).await
            }
        }
    }

    /// Trim to keep the newest `keep_n` archives whose name carries `prefix` (this vault's).
    pub async fn apply_retention(&self, keep_n: usize, prefix: &str) -> Result<usize> {
        match self {
            Self::Proton { cli } => {
                let (cli, prefix) = (cli.clone(), prefix.to_string());
                spawn_blocking_result(move || super::proton::apply_retention(&cli, keep_n, &prefix))
                    .await
            }
            Self::GoogleDrive { token_key } => {
                let folder = super::gdrive::ensure_backup_folder(token_key).await?;
                super::gdrive::apply_retention(token_key, &folder, keep_n, prefix).await
            }
        }
    }

    /// Pull the archive named `name` into `dest_dir` (written as `dest_dir/<name>`).
    pub async fn download(&self, name: &str, dest_dir: &Path) -> Result<()> {
        match self {
            Self::Proton { cli } => {
                let (cli, name, dest) = (cli.clone(), name.to_string(), dest_dir.to_path_buf());
                // `None`: this enum arm is never exercised — live Proton restore uses the direct
                // `proton::download_archive` call in `restore_from_proton`, which threads the real
                // cancel flag. Passing `None` keeps this dead arm compiling + timeout-bounded without
                // dragging an AppHandle into an otherwise-cold path.
                spawn_blocking_result(move || {
                    super::proton::download_archive(&cli, &name, &dest, None)
                })
                .await
            }
            Self::GoogleDrive { token_key } => {
                super::gdrive::download_archive(token_key, name, dest_dir).await
            }
        }
    }
}

/// Run a blocking, fallible closure on the blocking pool and flatten the `JoinError`. Keeps the
/// Proton arms above to one line each.
async fn spawn_blocking_result<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Other(format!("backup task panicked: {e}")))?
}
