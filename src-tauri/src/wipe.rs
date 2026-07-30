// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Remove PM data" — the in-app teardown behind Settings → Data & Security (the counterpart to the
//! Windows uninstaller, which only removes the app + the regenerable `runtime/`). It erases, à la
//! carte, the four classes of data PM leaves on a machine:
//!
//!   1. **Regenerable components** — the whole `runtime/` (Python venv incl. t-SNE/OCR, the Whisper
//!      model, the bundled interpreter). Exactly what Settings → Storage frees; re-downloaded on
//!      next use. The shared embedder model cache lives OUTSIDE the data dir and is deliberately
//!      left (matching the Storage tab, which marks it read-only).
//!   2. **Vault & database** — the real user data: the encrypted `pm.sqlite`, the `vault-meta.json`,
//!      the Markdown vault, and the ancillary vault artifacts (entity rules, index-only manifest,
//!      lock batons, the relocation pointer). Irreversible.
//!   3. **OS keychain** — every secret under the `org.itsatlas.pm` service (DB key, API keys, backup
//!      passphrase, all OAuth client creds + per-account tokens, ICS feeds, cached vault keys). When
//!      selected, PM first **revokes** each Google grant at Google's end; Microsoft has no equivalent
//!      revoke endpoint for a public desktop client, so its accounts are returned for a "finish at
//!      account.microsoft.com" link and only the local tokens are deleted.
//!   4. **Interface preferences & webview data** — the webview clears its own `localStorage`, then
//!      this command removes the OS-level store behind it ([`paths::os_app_leftovers`]). On macOS
//!      that store is a set of real directories the app never owned a handle to
//!      (`~/Library/WebKit/<id>`, plus caches, cookies, saved window state and the `NSUserDefaults`
//!      plist), so `localStorage.clear()` alone left a reinstall remembering dev mode and the
//!      last-seen version. On Linux it is the single directory `~/.local/share/<id>`, which
//!      WebKitGTK uses for all of the above — and which nothing else on that platform would ever
//!      remove, since Linux ships no uninstaller hook. Windows is the only no-op: there the folder
//!      is held open while PM runs, so the NSIS uninstaller purges it from outside instead.
//!
//! **Removing PM itself is not the same act on every platform**, and this module no longer pretends
//! it is: Windows launches the NSIS uninstaller, macOS has none (the user moves the `.app` to the
//! Trash — PM only offers to reveal it), and Linux leaves the binary to the package manager. See
//! [`FinishStep`].
//!
//! Backups are intentionally NOT touched here: local `.pmbackup` files and any Proton/Google Drive
//! destination are separate, and the UI directs the user to remove those at the source.
//!
//! Steps are best-effort — this runs when the user is deliberately erasing PM, so "remove as much as
//! possible, report honestly" beats "abort on the first locked file". The one deliberate exception is
//! the encrypted store file: because its only key lives in the keychain, the wipe deletes that key
//! **only after** the file is confirmed gone, and ABORTS (leaving everything intact and openable) if
//! the file can't be removed — never leaving a keyless, unreadable store behind (the boot "wrong key
//! or corrupt file" brick). See [`wipe_pm_data`] for the ordered sequence.

use std::path::Path;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::{google, paths, secrets, vault, AppState};

/// The data-dir-relative folder every restore extracts into (`restored-vaults/restore-<ts>/`). Three
/// concerns must agree on this name or a restored, decryptable vault copy leaks: the restore writers
/// (`commands.rs`), the wipe's vault branch, and the boot-time [`sweep_restore_staging`] GC. It lives
/// here — the module that owns the removal semantics — so those consumers can't drift from it.
pub const RESTORE_STAGING_DIR: &str = "restored-vaults";

/// Which classes of data to remove. Mirrors the four checkboxes; `camelCase` to match the webview.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeSelection {
    /// The regenerable `runtime/` (venv + t-SNE/OCR + Whisper + bundled interpreter).
    pub regenerable: bool,
    /// The vault, encrypted store, and vault artifacts. Irreversible.
    pub vault_and_db: bool,
    /// Every keychain secret; implies revoking Google grants + reporting Microsoft accounts.
    pub keychain: bool,
    /// Interface preferences. The webview clears its own `localStorage` before calling in; this
    /// flag is what lets the backend also remove the OS-level store BEHIND it — a real directory
    /// `localStorage.clear()` cannot reach, at `~/Library/WebKit/<id>` on macOS and
    /// `~/.local/share/<id>` on Linux. A no-op only on Windows, where the folder is held open while
    /// PM runs and the NSIS uninstaller purges it from outside instead. Linux used to be grouped
    /// with Windows here, which was wrong twice: the folder IS removable there, and Linux has no
    /// uninstaller to fall back on — so nothing removed it, ever.
    #[serde(default)]
    pub local_storage: bool,
}

/// What the user must still do to remove PM *itself* once its data is gone. The platforms diverge
/// completely here, and the UI used to assume the Windows answer — so a Mac, where
/// `launch_uninstaller` always fails, was told to "finish uninstalling PM from Windows Settings".
#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FinishStep {
    /// Not a full purge — PM stays installed and there is nothing further to do.
    #[default]
    None,
    /// Windows: PM launches the NSIS uninstaller, which removes the program files and the leftover
    /// data + webview folders via the marker armed during the wipe.
    WindowsUninstaller,
    /// macOS: there is no uninstaller. Everything PM wrote is gone, but the `.app` bundle is the
    /// user's to move to the Trash; PM offers to reveal it in Finder.
    MacosDragToTrash,
    /// Linux: the package manager (deb/rpm) or the AppImage file owns the binary.
    ManualRemoval,
}

/// The outcome, surfaced in the "done" summary. All counts are best-effort.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeReport {
    /// Human-readable labels of what was removed, for the summary list.
    pub removed: Vec<String>,
    /// Approx bytes freed on disk (regenerable + vault/db), summed before deletion.
    pub freed_bytes: u64,
    /// Google grants successfully revoked at Google's end.
    pub google_revoked: usize,
    /// Google tokens that couldn't be revoked (offline / already-invalid); their local copy is gone
    /// regardless, so nothing sensitive remains on the device.
    pub google_revoke_failures: usize,
    /// Connected Microsoft account emails — there's no programmatic revoke for PM's public client, so
    /// the UI links the user to account.microsoft.com to finish removing the grant.
    pub microsoft_accounts: Vec<String>,
    /// Keychain entries actually deleted.
    pub keychain_deleted: usize,
    /// True when the store or keychain was touched, so the running app can no longer function and
    /// must close (the frontend shows a final "PM has been reset" screen and exits).
    pub quit_required: bool,
    /// True when EVERY class was removed — a "remove PM completely" wipe. The UI then launches the
    /// NSIS uninstaller (which purges the leftover data + webview folders via the marker armed here)
    /// so nothing of PM remains on the machine.
    pub full_purge: bool,
    /// How the user finishes removing PM itself — see [`FinishStep`]. Set only on a full purge.
    pub finish_step: FinishStep,
    /// OS-written leftovers removed from outside the data dir (macOS only). Reported so the summary
    /// can be specific instead of implying a clean sweep it may not have made.
    pub os_leftovers_removed: usize,
    /// Everything PM knows about and did NOT remove, each with the path and a plain reason.
    ///
    /// The erase is best-effort by design, and until now that meant a locked folder or a vault
    /// belonging to another account simply went unmentioned while the summary still said everything
    /// was gone. The promise is not "PM deleted everything" — it is "nothing of yours is left
    /// somewhere you don't know about", and this is the half that makes the second one true.
    pub could_not_remove: Vec<Leftover>,
}

/// One thing the erase left behind, and why — so a user can finish by hand.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    /// The full path, exactly as it should be pasted into a file manager.
    pub path: String,
    /// Why it is still there, as a sentence fragment following the path.
    pub reason: String,
}

impl WipeReport {
    /// Record something the erase deliberately or unavoidably left. De-duplicated by path, since
    /// several steps can reach the same folder and one leftover should be one line to the user.
    fn leave_behind(&mut self, path: impl std::fmt::Display, reason: impl Into<String>) {
        let path = path.to_string();
        if self.could_not_remove.iter().any(|l| l.path == path) {
            return;
        }
        self.could_not_remove.push(Leftover {
            path,
            reason: reason.into(),
        });
    }
}

/// One connected OAuth account, reduced to what the wipe needs: its keychain token key, whether it's
/// Google (revocable) and the account email.
struct OauthAccount {
    /// The fully-formed keychain key the token blob lives under.
    token_key: String,
    /// The account email (for the per-account Google client keys / the Microsoft manual list).
    email: String,
    /// `google` grants are revoked; `microsoft` grants are only deleted locally + reported.
    provider: Provider,
}

#[derive(PartialEq)]
enum Provider {
    Google,
    Microsoft,
}

/// Everything the keychain teardown needs, gathered from the open store up front — but nothing is
/// revoked or deleted yet. The actual revoke + secret deletion happens LAST (after the DB file is
/// confirmed gone), so an aborted/locked file delete can never strand the store without its key, and
/// a wipe that stops before that point has not already revoked the user's Google grants.
struct KeychainWipePlan {
    /// Every per-account OAuth token key to delete (Google + Microsoft).
    token_keys: Vec<String>,
    /// The Google token keys specifically — their blobs are revoked at Google before deletion.
    google_token_keys: Vec<String>,
    /// Google account emails (drive the per-account client id/secret keys for Advanced-Protection).
    google_emails: Vec<String>,
    /// Ids of vaults whose derived key this profile has cached.
    vault_ids: Vec<String>,
}

/// Read every connected OAuth account out of `connector_sources` and build its keychain token key
/// from the provider/service (the crate can't enumerate the keychain, so we reconstruct the keys
/// from the DB). Runs while the store is still open, before any teardown.
fn enumerate_oauth_accounts(conn: &rusqlite::Connection) -> Result<Vec<OauthAccount>> {
    let mut stmt = conn.prepare(
        "SELECT provider, service, account_email FROM connector_sources \
         WHERE account_email IS NOT NULL AND account_email <> ''",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (provider, service, email) = row?;
        // Apple subscriptions / local folders hold no OAuth token in the keychain.
        let Some(token_key) = secrets::token_key_for(&provider, &service, &email) else {
            continue;
        };
        out.push(OauthAccount {
            token_key,
            email,
            provider: if provider == "google" {
                Provider::Google
            } else {
                Provider::Microsoft
            },
        });
    }
    Ok(out)
}

/// Recursively sum regular-file sizes under `path` (best-effort; an unreadable entry counts 0).
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            total += dir_size(&entry.path());
        } else if ft.is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

/// Remove a directory tree, tolerating the brief Windows lag between killing a child that held a
/// file open and the OS releasing the handle. Best-effort: a still-locked file just stays.
fn remove_dir_all_retrying(path: &Path) {
    if !path.exists() {
        return;
    }
    for attempt in 0..3 {
        if std::fs::remove_dir_all(path).is_ok() || !path.exists() {
            return;
        }
        // A short back-off lets a just-killed interpreter's handles close. Not on the last attempt.
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    let _ = std::fs::remove_dir_all(path); // final try; ignore a residual locked file
}

/// Remove a directory **only if it is empty**, tolerating the same brief Windows handle-release lag
/// as [`remove_dir_all_retrying`]. `remove_dir` is non-recursive, so this can only ever delete a
/// folder PM had to itself: a relocated vault whose user-chosen root still holds the user's
/// unrelated files can never be removed here (the call just fails, harmlessly). Retrying is
/// therefore always safe — it only helps the transient "empty, but a just-deleted child's handle
/// hasn't closed yet" case; a genuinely non-empty folder never succeeds and is left intact.
fn remove_empty_dir_retrying(path: &Path) {
    if !path.exists() {
        return;
    }
    for attempt in 0..3 {
        if std::fs::remove_dir(path).is_ok() || !path.exists() {
            return;
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    let _ = std::fs::remove_dir(path); // final try; a root that still holds unrelated files just stays
}

/// Delete the encrypted store's files — `pm.sqlite` and its `-wal`/`-shm` sidecars — retrying the
/// main file through the brief Windows lag while a just-dropped connection's handle closes (or an
/// antivirus / Search scan releases it). Returns whether `pm.sqlite` is actually gone. This is the
/// safety gate of the whole wipe: the store's ONLY key lives in the keychain, so the caller must
/// never delete that key while this returns `false`, or the store is left unreadable (the reported
/// "wrong key or corrupt file" brick). The sidecars are harmless leftovers, so they're best-effort.
fn remove_db_files_retrying(db_path: &Path) -> bool {
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(db_path.with_extension(format!("sqlite{suffix}")));
    }
    for attempt in 0..3 {
        if std::fs::remove_file(db_path).is_ok() || !db_path.exists() {
            return true;
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    let _ = std::fs::remove_file(db_path); // final try; report the truth either way
    !db_path.exists()
}

/// Delete the non-DB vault artifacts: metadata, the encrypted entity-rules file, the index-only
/// manifest, lock batons, the migration journal + its pre-migration backup (kept at the DATA DIR,
/// which differs from the vault root for a relocated vault — clearing at the root stranded a full
/// decryptable backup copy, B1-4), the Markdown tree, abandoned restore staging, and the relocation
/// pointer. A relocated vault's user-chosen root is only removed if PM had it to itself (never
/// wholesale — that would take the user's unrelated files, F-03). Returns the approximate bytes
/// freed by the tree removals it sizes. Kept separate from [`remove_db_files_retrying`] so the DB
/// file is confirmed gone first; shared by the wipe and the boot-error "start fresh" recovery.
fn remove_vault_artifacts(resolved: &vault::ResolvedVault, data_dir: &Path) -> u64 {
    let mut freed = 0u64;
    // One enumeration, shared with the migration's copy + delete. Naming the files here by hand had
    // already drifted: `vault-access.json` — the list of OS accounts linked to a shared vault, i.e.
    // account names — was in `vault_sidecar_files` and not in this list, so it survived "Remove PM
    // data". A file the erase forgets is also a file that keeps `remove_empty_dir_retrying` below
    // from ever succeeding, which is how the same class of bug bit the shared-vault delete.
    for file in vault::migrate::vault_sidecar_files(&resolved.vault_root) {
        let _ = std::fs::remove_file(file);
    }
    let _ = vault::lock::clear_baton_files(&resolved.vault_root);
    let _ = vault::migrate::clear_journal(data_dir);
    let migration_backup = vault::migrate::backup_dir(data_dir);
    freed += dir_size(&migration_backup);
    remove_dir_all_retrying(&migration_backup);
    remove_dir_all_retrying(&resolved.markdown_dir);
    // A relocated vault lives in a folder the *user* chose, which may hold unrelated files — its PM
    // artifacts were removed individually above, so take the root itself only if it's now empty.
    if resolved.vault_root != data_dir {
        remove_empty_dir_retrying(&resolved.vault_root);
    }
    let _ = vault::pointer::clear(data_dir);
    // The rejoin breadcrumb goes too — "remove the vault" means every trace of it.
    let _ = vault::pointer::clear_retired(data_dir);
    // Restore staging: full, DECRYPTABLE vault copies left by every inspected restore. PM owns this
    // whole tree (each `restore-*` is a PM-chosen path under the data dir), so it's safe to remove
    // wholesale — leaving decryptable copies behind after "remove the vault" would be a footgun.
    let restore_staging = data_dir.join(RESTORE_STAGING_DIR);
    freed += dir_size(&restore_staging);
    remove_dir_all_retrying(&restore_staging);
    freed
}

/// Union the vault ids a wipe must NAME, from its three best-effort sources. Pure, because this
/// is the whole correctness surface — the keychain can't be enumerated, so an id missing here is
/// a secret that survives "remove everything", silently and forever. Deduplicated, since deleting
/// the same key twice would double-count the "entries removed" the report shows the user.
fn cached_vault_ids_to_wipe(
    registry: Vec<String>,
    resolved: Option<String>,
    retired: Option<String>,
) -> Vec<String> {
    let mut out = registry;
    for id in [resolved, retired].into_iter().flatten() {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Whether the erase may delete the vault a pointer names, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointedVaultVerdict {
    /// Provably this account's own vault: remove PM's files from that folder.
    Delete,
    /// Not ours to remove. The reason becomes the sentence the user reads next to the path.
    Leave(LeaveReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaveReason {
    /// A shared vault another account created. Theirs to delete, from their own PM.
    OwnedByAnotherAccount,
    /// A shareable vault with no owner recorded, or an OS that cannot tell us who we are.
    OwnerUnknown,
    /// The folder holds no readable vault metadata, so we cannot establish anything about it.
    MetaUnreadable,
    /// The metadata failed its integrity check, so the ownership stamp in it is not evidence.
    MetaTampered,
}

impl LeaveReason {
    /// The user-facing half, appended after the folder's path.
    fn why(self) -> &'static str {
        match self {
            Self::OwnedByAnotherAccount => {
                "it belongs to another account on this machine, so it is theirs to delete"
            }
            Self::OwnerUnknown => {
                "PM can't confirm this account created it, and it won't delete a shared folder on a \
                 guess"
            }
            Self::MetaUnreadable => "PM can no longer read what is in it",
            Self::MetaTampered => {
                "its settings file failed its integrity check, so PM won't act on what it claims"
            }
        }
    }
}

/// Whether "Remove PM data" may delete the vault at the far end of this profile's pointer.
///
/// Pure, because this is the entire correctness surface of deleting outside PM's own folder: it
/// decides whether files the user chose the location of get removed. The polarity is deliberately
/// the OPPOSITE of [`vault::is_vault_owner`] — that one fails OPEN so a SID hiccup never locks a
/// real owner out of their connectors; this one fails CLOSED so a SID hiccup can never destroy
/// somebody's data. Every branch that isn't a proof leaves the folder alone and reports where it is.
///
/// `Device` deletes on every platform: a device vault's key lives in one account's keychain, so no
/// other account can open it wherever it sits. `Owned` needs a SID match, which only Windows can
/// give — so on macOS and Linux every *shareable* vault lands in `Unknown` and is reported rather
/// than removed. That is the honest outcome of shared vaults being a Windows feature, not an
/// oversight, and it leaves the ordinary "I moved my vault to another drive" case working everywhere.
fn pointed_vault_verdict(
    ownership: Option<vault::VaultOwnership>,
    meta_tampered: bool,
) -> PointedVaultVerdict {
    // Checked first: a tampered meta means the ownership stamp inside it is exactly the field an
    // attacker would edit, so it cannot be the thing that authorises a delete.
    if meta_tampered {
        return PointedVaultVerdict::Leave(LeaveReason::MetaTampered);
    }
    match ownership {
        None => PointedVaultVerdict::Leave(LeaveReason::MetaUnreadable),
        Some(vault::VaultOwnership::Device) => PointedVaultVerdict::Delete,
        Some(vault::VaultOwnership::Owned) => PointedVaultVerdict::Delete,
        Some(vault::VaultOwnership::Joined) => {
            PointedVaultVerdict::Leave(LeaveReason::OwnedByAnotherAccount)
        }
        Some(vault::VaultOwnership::Unknown) => {
            PointedVaultVerdict::Leave(LeaveReason::OwnerUnknown)
        }
    }
}

/// Gather (but do not yet act on) everything the keychain teardown needs, while the store is still
/// open. Best-effort: a locked / forgotten-passphrase vault has no connection, and that user must
/// still be able to erase their secrets, so no store just means no per-account tokens to enumerate —
/// the fixed keys (DB key, API keys, backup passphrase) + the cached vault key are wiped regardless
/// (F-24). Records the connected Microsoft accounts into `report` for the manual-revoke link.
fn plan_keychain_wipe(
    app: &AppHandle,
    state: &AppState,
    report: &mut WipeReport,
) -> KeychainWipePlan {
    let accounts = match state.conn() {
        Ok(conn) => enumerate_oauth_accounts(&conn).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    // Every vault whose key this profile has cached (`vault_key::<id>`) — not just the current
    // one. The keychain can't be enumerated, so a key survives unless the wipe can NAME it, and
    // naming only the resolved vault left a live SQLCipher master key behind for any vault the
    // profile had moved on from: `detach_from_shared_vault` KEEPS its cached key on purpose (for a
    // silent rejoin), so a detached shared vault's key outlived "remove everything" while the
    // folder it opens may still be sitting on the machine. Three sources, unioned:
    //
    //  1. the registry of ids this profile ever cached (secrets::known_cached_vault_ids),
    //  2. the currently resolved vault's meta — the old behaviour, kept so a lost or
    //     never-written registry can only ever degrade to it, never below it,
    //  3. the retired pointer's root, which covers a profile that detached BEFORE the registry
    //     shipped and whose folder still answers.
    //
    // All best-effort: an unreachable root just contributes nothing. Read here, in step 1, while
    // the retired pointer still exists — step 2 deletes it.
    let vault_ids = cached_vault_ids_to_wipe(
        secrets::known_cached_vault_ids(),
        vault::resolve(app)
            .ok()
            .and_then(|r| vault::load_meta(&r.vault_root).ok().flatten())
            .map(|m| m.vault_id),
        paths::data_dir(app)
            .ok()
            .and_then(|d| vault::pointer::load_retired(&d).ok().flatten())
            .and_then(|p| vault::load_meta(&p.vault_root).ok().flatten())
            .map(|m| m.vault_id),
    );

    let mut google_token_keys = Vec::new();
    let mut microsoft_token_keys = Vec::new();
    let mut google_emails = Vec::new();
    for a in accounts {
        match a.provider {
            Provider::Google => {
                google_emails.push(a.email);
                google_token_keys.push(a.token_key);
            }
            Provider::Microsoft => {
                if !report.microsoft_accounts.contains(&a.email) {
                    report.microsoft_accounts.push(a.email);
                }
                microsoft_token_keys.push(a.token_key);
            }
        }
    }
    google_emails.sort();
    google_emails.dedup();

    let mut token_keys = google_token_keys.clone();
    token_keys.extend(microsoft_token_keys);

    KeychainWipePlan {
        token_keys,
        google_token_keys,
        google_emails,
        vault_ids,
    }
}

/// Revoke each connected Google grant at Google's end, reading the token blobs from the keychain (a
/// wipe deletes them right after). Runs the revokes concurrently with a short per-call bound so an
/// offline or slow endpoint can't stall the wipe for the HTTP client's full 30s each — the local
/// token is deleted regardless, so a missed revoke only leaves a grant to tidy at
/// myaccount.google.com. Tallies successes/failures into `report`.
async fn revoke_google_grants(plan: &KeychainWipePlan, report: &mut WipeReport) {
    let blobs: Vec<String> = plan
        .google_token_keys
        .iter()
        .filter_map(|k| {
            secrets::get_google_token_for(k)
                .ok()
                .flatten()
                .map(|s| s.expose().to_string())
        })
        .collect();
    let results = join_all(blobs.iter().map(|b| async move {
        // The local copy is removed regardless, so a timeout / offline / already-invalid token is a
        // non-fatal "couldn't reach Google" — never a reason to hold up the erase.
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(8), google::revoke(b)).await,
            Ok(Ok(()))
        )
    }))
    .await;
    for ok in results {
        if ok {
            report.google_revoked += 1;
        } else {
            report.google_revoke_failures += 1;
        }
    }
}

/// A full wipe (every data class selected) means "remove PM completely." The app can delete its own
/// `Personal Manager` data dir, but not the in-use WebView2 folder (`%LOCALAPPDATA%\org.itsatlas.pm`)
/// or the installed program — those need the process gone. So drop a marker the NSIS uninstaller's
/// hook reads (it purges both leftover folders once PM has exited; a normal uninstall, with no
/// marker, still keeps user data for a reinstall), remove the data dir now, and flag the report so
/// the UI launches the uninstaller. Best-effort throughout — the uninstaller hook is the backstop.
fn arm_full_uninstall(app: &AppHandle, report: &mut WipeReport) {
    // The marker exists for exactly one reader: the NSIS uninstaller's post-uninstall hook. That
    // hook is Windows-only, as is `launch_uninstaller`, so writing the marker anywhere else created
    // a file nothing would ever read — on macOS it even re-created
    // `~/Library/Application Support/org.itsatlas.pm/` moments after the wipe had removed it, then
    // reported a full purge. Gated, so the leftover sweep below is the last word off Windows.
    #[cfg(target_os = "windows")]
    {
        // The marker lives in the webview folder (which survives the app deleting its own data dir).
        if let Ok(marker) = paths::uninstall_purge_marker(app) {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, b"PM full-uninstall purge marker\n");
        }
    }
    // Remove the `Personal Manager` data dir itself — it's entirely PM-owned (a relocated vault
    // lives elsewhere and was handled by the vault branch), so unlike a user-chosen relocation root
    // this is safe to take wholesale. `paths::data_dir` recreates it empty; delete that. If a stray
    // handle blocks it, the marker-driven uninstaller hook is the backstop.
    if let Ok(data_dir) = paths::data_dir(app) {
        remove_dir_all_retrying(&data_dir);
        // Truthful, not assumed. This used to be set unconditionally, so off Windows — where no
        // uninstaller follows to sweep up — the summary said "everything PM stored is gone" over a
        // folder that might still be sitting there.
        if data_dir.exists() {
            report.leave_behind(
                data_dir.display(),
                "PM could not remove its own folder — most likely a file still open in another \
                 program. Close everything and delete it by hand",
            );
        }
    }
    // The Windows AppContainer PM creates to confine its document worker. Nothing has ever removed
    // it, so a profile folder and a registry mapping outlive "remove PM completely". Must run after
    // the worker is stopped — the API refuses while a process is still using the container.
    #[cfg(windows)]
    crate::sidecar_sandbox::remove_container_profile();
    report.full_purge = true;
}

/// Remove the OS-written leftovers that live outside the data dir (see [`paths::os_app_leftovers`]).
/// Returns how many existed and were removed, so the summary can say something true rather than
/// claiming a clean sweep it did not make.
///
/// Best-effort per entry, like the rest of the wipe: one stubborn path must not abort the others.
/// Name — never delete — the stores PM CAUSED to exist but does not own.
///
/// PM's pull button downloaded the Ollama models and PM signed the user into Proton Drive, so
/// leaving these unmentioned is the "found something afterwards" failure even though removing them
/// would be wrong: they are another program's data, shared with whatever else uses it. Reported only
/// when the path actually exists, so nobody is sent hunting for a folder they never had.
fn report_third_party_stores(app: &AppHandle, report: &mut WipeReport) {
    let Ok(home) = app.path().home_dir() else {
        return;
    };
    for root in crate::local_disk::ollama_roots(&home, std::env::var("OLLAMA_MODELS").ok()) {
        if root.exists() {
            report.leave_behind(
                root.display(),
                "models you downloaded through PM's Local AI tab, stored by Ollama. They can be \
                 many gigabytes, and they belong to Ollama rather than to PM — remove them there \
                 if you no longer want them",
            );
        }
    }
    // macOS keeps the microphone grant in a system database no application may edit.
    #[cfg(target_os = "macos")]
    report.leave_behind(
        "Microphone permission (System Settings > Privacy & Security)",
        "macOS remembers that you allowed PM to use the microphone, and no app can revoke that for \
         itself. Remove PM from the Microphone list there, or run: tccutil reset Microphone \
         org.itsatlas.pm",
    );
}

fn remove_os_leftovers(app: &AppHandle, report: &mut WipeReport) -> usize {
    let mut removed = 0usize;
    for path in paths::os_app_leftovers(app) {
        if !path.exists() {
            continue;
        }
        report.freed_bytes += if path.is_dir() {
            dir_size(&path)
        } else {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        if path.is_dir() {
            remove_dir_all_retrying(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
        if !path.exists() {
            removed += 1;
        }
    }
    removed
}

/// Boot-time GC of abandoned restore staging. Each `restored-vaults/restore-*` folder holds a full,
/// DECRYPTABLE copy of a vault that a restore extracted for the user to inspect before committing. If
/// they committed (`switch_to_vault`), its key was promoted into the keychain and that folder IS the
/// live vault — keep it. Every other copy's key lived only in `pending_restore_keys` (in memory) and
/// died with the process that restored it, so it can never be opened again: it's pure plaintext-leak
/// residue and is removed. Best-effort; a still-locked entry just waits for the next boot. (F-25)
///
/// Deletion is fail-safe: we canonicalise the active root once and skip the whole sweep if it won't
/// resolve, then only ever remove a candidate whose *canonical* path resolves and differs from it — so
/// a path-shape mismatch (trailing slash, case, `\\?\`) can never delete the vault in use.
pub fn sweep_restore_staging(data_dir: &Path, active_vault_root: &Path) {
    let staging = data_dir.join(RESTORE_STAGING_DIR);
    // If the active root can't be resolved, do nothing rather than risk mistaking it for a stale copy.
    let Ok(active) = active_vault_root.canonicalize() else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(&staging) else {
        return; // no staging dir yet — nothing to sweep
    };
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        // Only remove a folder we can positively resolve AND prove is not the live vault. A candidate
        // that won't canonicalise is left alone (a lingering copy is harmless; a wrong delete is not).
        match path.canonicalize() {
            Ok(canon) if canon != active => remove_dir_all_retrying(&path),
            _ => {}
        }
    }
}

/// Remove PM's own files from a vault folder the user chose the location of, once
/// [`pointed_vault_verdict`] has proved it is this account's to remove.
///
/// Deliberately the `delete_shared_vault` recipe (commands.rs), in its order and for its reasons —
/// release OUR writer lock first, because `vault.lock` sits in the folder we are about to empty and
/// `delete_vault_artifacts` spares it on purpose (it cannot tell our lock from another instance's);
/// then drop the ACL lockdown PM applied so the artifacts are deletable; then the artifacts; then
/// the folder ONLY if PM had it to itself. Never `remove_dir_all` on a user-chosen folder: it may
/// hold their own files, and it may be a synced folder where a recursive delete propagates to every
/// other device on the account.
fn remove_pointed_vault(
    app: &AppHandle,
    state: &AppState,
    root: &Path,
    meta: Option<&vault::VaultMeta>,
    report: &mut WipeReport,
) {
    report.freed_bytes += dir_size(&root.join("vault"));
    report.freed_bytes += std::fs::metadata(root.join("pm.sqlite"))
        .map(|m| m.len())
        .unwrap_or(0);

    crate::lock_session::disengage(app);
    let _ = vault::lock::release(root, &state.instance_id);
    let _ = vault::acl::reset_inheritance(root);
    vault::migrate::delete_vault_artifacts(root);
    // `delete_vault_artifacts` spares vault.lock for its boot-recovery callers; this caller is
    // erasing its own vault, so the lock is ours or nobody's — and left in place it would guarantee
    // the empty check below never passes.
    let _ = std::fs::remove_file(root.join("vault.lock"));

    let emptied = std::fs::read_dir(root)
        .map(|mut d| d.next().is_none())
        .unwrap_or(false);
    if emptied {
        let _ = std::fs::remove_dir(root);
    } else if root.exists() {
        report.leave_behind(
            root.display(),
            "PM removed its own files from this folder but left the folder itself, because it \
             still holds things PM did not put there",
        );
    }

    // The machine-wide advertisement names this folder and the account that owned it, and lives
    // where every account can read it. With the vault gone it is stale either way; a vault that
    // lived outside PM's own folder gets a tombstone instead of a plain retraction, so a joiner
    // learns it was deleted rather than seeing it as merely unreachable.
    if let (Some(meta), Some(ads)) = (meta, vault::advert::ads_dir()) {
        if meta.key_mode == vault::KeyMode::Passphrase {
            let tombstone = vault::advert::SharedVaultAd::tombstone(&meta.vault_id, root);
            if vault::advert::publish(&ads, &tombstone).is_ok() {
                report.leave_behind(
                    vault::advert::ads_dir()
                        .map(|d| {
                            d.join(format!("{}.json", meta.vault_id))
                                .display()
                                .to_string()
                        })
                        .unwrap_or_default(),
                    "a small marker telling other accounts on this machine that the shared vault \
                     was deleted rather than merely unreachable — safe to delete once they have \
                     each opened PM once",
                );
            }
        } else {
            let _ = vault::advert::retract(&ads, &meta.vault_id);
        }
    }
}

/// Every filename prefix PM mints inside the OS temp directory. Each is followed by a random
/// suffix from `tempfile`, so nothing a user put there can match one — which is what makes sweeping
/// by prefix safe. Kept as one list because a new staging site that forgets to join it becomes a
/// leftover nothing will ever find.
///
/// `pm-drive-`/`pm-onedrive-` are the ones that matter most: they hold a **plaintext** copy of a
/// document downloaded from a connector, staged for the converter. That staging deliberately
/// disarms its own auto-delete (`cloud_sync::stage_temp` calls `.keep()`, so the converter can open
/// it on Windows) and is removed only on the normal return path — so a crash, a kill, or a
/// force-quit mid-sync strands the user's actual file, in the clear, forever.
const TEMP_STAGING_PREFIXES: &[&str] = &[
    "pm-drive-",
    "pm-onedrive-",
    "pm-export-",
    "pm-backup-",
    "pm-backup-snap-",
    "pm-restore-",
    "pm-restore-gdrive-",
    "pm-restore-proton-",
    "pm-python-",
];

/// The infix in the updater's own staging directory name, which `tauri-plugin-updater` builds as
/// `<app name>-<version>-updater-<random>`. It downloads the next installer there and then calls
/// `process::exit`, so the temp guard's destructor never runs and every installer PM has ever
/// applied stays on disk — around 100 MB each, forever.
///
/// Matched by the infix rather than by an `app_name` prefix on purpose: the product name is "PM",
/// and sweeping everything in the temp directory starting `PM-` would be reckless. Requiring
/// `-updater-` as well makes a false positive essentially impossible.
const UPDATER_STAGING_INFIX: &str = "-updater-";

/// Whether a temp entry is one of PM's own staging files. Pure, so the matching rule — the part
/// that must never widen to something a user owns — is testable without touching a disk.
///
/// `app_name` is passed in rather than hardcoded so the rule stays keyed to the configured product
/// name and this stays testable from anywhere.
fn is_pm_temp_staging(name: &str, app_name: &str) -> bool {
    if TEMP_STAGING_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    // Both halves required, and the prefix must be the WHOLE app name followed by a separator —
    // "PMS-1-updater-x" from some other program must not match.
    name.starts_with(&format!("{app_name}-")) && name.contains(UPDATER_STAGING_INFIX)
}

/// Remove PM's abandoned staging files from the OS temp directory, returning how many went.
///
/// Runs at boot (no sync, backup or restore can be in flight yet, so nothing live is at risk) and
/// again during the wipe. Scoped to `std::env::temp_dir()` and to [`is_pm_temp_staging`]; anything
/// that is not a positive prefix match is not touched.
pub fn sweep_temp_staging(app_name: &str) -> usize {
    let tmp = std::env::temp_dir();
    let Ok(rd) = std::fs::read_dir(&tmp) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_pm_temp_staging(&name, app_name) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            remove_dir_all_retrying(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
        if !path.exists() {
            removed += 1;
        }
    }
    removed
}

/// Execute the confirmed removal. The UI has already gated this behind the full confirmation ladder
/// (explicit checkboxes → "are you sure" itemisation → optional Windows Hello → type-to-confirm), so
/// this simply carries it out in a safe order and reports what happened.
///
/// **The order is a data-safety invariant, not a convenience.** The encrypted store's only key lives
/// in the keychain, so if the key is deleted while `pm.sqlite` survives — because the file delete was
/// interrupted (a force-quit during a slow revoke) or lost a race with an antivirus / Windows-Search
/// lock — the next boot regenerates a fresh key, can't decrypt the old store, and dead-ends on the
/// "could not open database" screen with no way back to setup. So:
///   1. gather the keychain plan from the open store — but revoke/delete nothing yet;
///   2. close + delete the DB file **reliably**, and if it can't be removed, ABORT with the store
///      fully intact (its key still opens it) rather than pressing on to brick it;
///   3. only now revoke Google grants and delete the keychain secrets (the key outlives the file);
///   4. remove the regenerable runtime;
///   5. if every class was selected, arm the uninstaller so PM leaves nothing behind.
/// Every interruption point in this sequence leaves a recoverable state.
#[tauri::command]
pub async fn wipe_pm_data(
    app: AppHandle,
    state: State<'_, AppState>,
    selection: WipeSelection,
) -> Result<WipeReport> {
    let mut report = WipeReport::default();

    // --- 1. Plan the keychain teardown from the open store (revoke + delete come later, in step 3). ---
    let keychain_plan = if selection.keychain {
        Some(plan_keychain_wipe(&app, &state, &mut report))
    } else {
        None
    };

    // --- 2. Vault & database. Wiping the keychain removes the DB's only key, so the store must go
    //        with it — enforced here (`vault_and_db || keychain`), not only in the UI, so the pair
    //        can never orphan an unreadable store. Delete the DB FILE first and reliably; abort if
    //        it can't be removed, before any irreversible key deletion. ---
    if selection.vault_and_db || selection.keychain {
        let data_dir = paths::data_dir(&app)?;
        // A profile can POINT at a vault outside its own folder, and the pointer is written by two
        // very different doors: joining someone else's shared vault, and moving your own. This used
        // to refuse both, which was right for the first and wrong for the second — a vault YOU moved
        // is live and fully accessible, so the erase deleted its key while leaving the folder, and a
        // moved device vault keeps PLAINTEXT notes. Readable notes with a destroyed key is the worst
        // of both. So the two cases are now told apart by evidence, and the local state at the
        // DEFAULT location is wiped either way.
        let pointed = vault::pointer::load(&data_dir).ok().flatten();
        let pointed_root = pointed.as_ref().map(|p| p.vault_root.clone());
        let pointed_verdict = pointed_root.as_ref().map(|root| {
            let meta = vault::load_meta(root).ok().flatten();
            let verdict = pointed_vault_verdict(
                meta.as_ref().map(vault::vault_ownership),
                state.meta_warning().is_some(),
            );
            (verdict, meta)
        });
        let resolved = if pointed_root.is_some() {
            let _ = state.take_conn();
            let _ = state.clear_vault_runtime();
            vault::resolve_layout(&data_dir, None)
        } else {
            vault::resolve(&app)?
        };

        // Size the user data before removing it (DB file + Markdown tree).
        report.freed_bytes += std::fs::metadata(&resolved.db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        report.freed_bytes += dir_size(&resolved.markdown_dir);

        // Drop the live connection (its `Drop` releases SQLite's file lock), then delete the store.
        let _ = state.take_conn();

        if !remove_db_files_retrying(&resolved.db_path) {
            // The encrypted store is still on disk — a file lock we couldn't outwait (antivirus /
            // Windows Search). Nothing has been revoked or key-deleted yet, so the store still opens
            // with its existing key: this is a clean "try again", never a brick. Stop before any
            // irreversible step and say so honestly.
            return Err(Error::Other(
                "Couldn't remove the database file — it may be held open by antivirus or Windows \
                 Search. Close other apps and run Remove again. Nothing was deleted."
                    .into(),
            ));
        }

        // The DB file is confirmed gone; now the rest of the vault artifacts.
        report.freed_bytes += remove_vault_artifacts(&resolved, &data_dir);
        // `delete_vault_artifacts` spares `vault.lock` for its boot-recovery callers, and the wipe
        // inherited that without inheriting the reason: here the lock is this profile's own, and
        // left behind it names the OS profile that held it.
        crate::lock_session::disengage(&app);
        let _ = vault::lock::release(&resolved.vault_root, &state.instance_id);
        let _ = std::fs::remove_file(resolved.vault_root.join("vault.lock"));
        // PM's staging files in the OS temp dir. A connector download is staged there in the CLEAR
        // and its auto-delete is disarmed so the converter can read it, so an interrupted sync
        // leaves the user's document behind — outside every folder this erase would otherwise visit.
        let swept = sweep_temp_staging(&app.package_info().name);
        if swept > 0 {
            report.removed.push(format!(
                "{swept} leftover staging file(s) from interrupted syncs"
            ));
        }

        // Now the pointed vault, if this profile has one and it is provably ours.
        match (&pointed_root, pointed_verdict) {
            (Some(root), Some((PointedVaultVerdict::Delete, meta))) => {
                remove_pointed_vault(&app, &state, root, meta.as_ref(), &mut report);
                report.removed.push(format!(
                    "Vault & encrypted database, including {}",
                    root.display()
                ));
            }
            (Some(root), Some((PointedVaultVerdict::Leave(why), _))) => {
                // Named, never guessed at. The user is about to lose the key that opens whatever is
                // in there, so being told exactly where it is is the least PM owes them.
                report.leave_behind(
                    root.display(),
                    format!("PM did not delete this vault: {}", why.why()),
                );
                report
                    .removed
                    .push("This account's PM data (see what was left below)".into());
            }
            _ => report.removed.push("Vault & encrypted database".into()),
        }
        report.quit_required = true;
    }

    // --- 3. Keychain secrets, LAST — only now that the store file is gone. Revoke Google grants
    //        (reading the tokens still in the keychain), then delete every secret. ---
    if let Some(plan) = keychain_plan {
        revoke_google_grants(&plan, &mut report).await;
        report.keychain_deleted =
            secrets::wipe_all_secrets(&plan.token_keys, &plan.google_emails, &plan.vault_ids);
        report.removed.push("Keychain secrets & saved keys".into());
        report.quit_required = true;
    }

    // --- 4. Regenerable components. Stop the sidecar (releases the interpreter's locks), then remove. ---
    if selection.regenerable {
        state.sidecar.prepare_for_runtime_removal();
        let runtime = paths::data_dir(&app)?.join("runtime");
        report.freed_bytes += dir_size(&runtime);
        remove_dir_all_retrying(&runtime);
        report
            .removed
            .push("Downloaded components (engine, models)".into());
    }

    // --- 5. A full wipe (every class) removes PM itself: arm the uninstaller to purge the leftover
    //        data + webview folders once PM exits, and clear the data dir now. ---
    let full = selection.regenerable && selection.vault_and_db && selection.keychain;
    if full {
        arm_full_uninstall(&app, &mut report);
    }

    // --- 6. The OS-written leftovers outside the data dir (macOS). Driven by the preferences
    //        checkbox — they ARE this app's preferences and webview store — and unconditionally on a
    //        full purge, so "remove PM completely" means it. LAST, because `arm_full_uninstall`
    //        recreates one of these paths on Windows and we must not race it, and because the
    //        webview is still live: the frontend clears `localStorage` BEFORE calling in, so there
    //        is nothing left for WKWebView to flush back over the top of this. ---
    if selection.local_storage || full {
        report.os_leftovers_removed = remove_os_leftovers(&app, &mut report);
        if report.os_leftovers_removed > 0 {
            report.removed.push("App preferences & webview data".into());
            report.quit_required = true;
        }
    }

    // Last: name what PM caused but must not remove. Only on a full purge — on a partial wipe the
    // user is keeping PM, so pointing them at their own Ollama models would be noise.
    if full {
        report_third_party_stores(&app, &mut report);
    }

    // How the user finishes removing PM itself. Resolved here, in the one place that knows the wipe
    // was total, rather than left to the UI to infer from the platform.
    if full {
        report.finish_step = if cfg!(target_os = "windows") {
            FinishStep::WindowsUninstaller
        } else if cfg!(target_os = "macos") {
            FinishStep::MacosDragToTrash
        } else {
            FinishStep::ManualRemoval
        };
    }

    if report.removed.is_empty() {
        return Err(Error::Other("Nothing was selected to remove.".into()));
    }
    Ok(report)
}

/// Reveal PM's own `.app` bundle in Finder, selected, so the user can drag it to the Trash — the
/// macOS end of a full wipe, where there is no uninstaller to launch.
///
/// PM deliberately does NOT delete itself. A self-delete is unrecoverable outside the Trash and
/// fails in ways the user cannot see (app translocation, a bundle in a read-only location, or
/// `/Applications` wanting admin), so the last step stays a deliberate act by the person doing it.
#[tauri::command]
pub fn reveal_app_in_finder() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // …/PM.app/Contents/MacOS/PM → …/PM.app
        let exe = std::env::current_exe()
            .map_err(|e| Error::Other(format!("couldn't locate the app: {e}")))?;
        let bundle = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| {
                Error::Other(
                    "PM isn't running from an app bundle, so there's nothing to reveal — your data \
                     is already removed."
                        .into(),
                )
            })?;
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(bundle)
            .spawn()
            .map_err(|e| Error::Other(format!("couldn't open Finder: {e}")))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::Other(
            "Revealing the app in Finder is a macOS step.".into(),
        ))
    }
}

/// Whether a carried boot open-error message denotes a genuine, non-transient brick — a wrong key or
/// a corrupt file, the deterministic failure whose data can't be recovered. Transient lock / disk-I/O
/// messages (which `db::open` tags distinctly) return `false`, so the destructive "start fresh" reset
/// can never delete a healthy vault that was only momentarily unavailable. Pure, so the safety gate
/// is unit-tested without a live store.
fn is_genuine_brick(boot_error: &str) -> bool {
    boot_error.contains(crate::db::WRONG_KEY_OR_CORRUPT_MSG)
}

/// Why a "start fresh" reset must be refused for this fault, or `None` when the reset may
/// proceed. Pure, so both safety gates are unit-tested together: an access-DENIED vault is
/// NEVER a brick — the data is intact behind a permissions problem, whatever the message
/// text says — and a transient lock/disk blip may clear on its own. Only the deterministic
/// wrong-key / corrupt-file failure passes.
fn reset_refusal(fault: &crate::error::VaultFault) -> Option<&'static str> {
    if fault.code == crate::error::VaultFaultCode::Denied {
        return Some(
            "Windows is refusing access to the vault — the vault itself isn't broken, and \
             nothing needs deleting. Use Repair access (Settings → Vault) instead.",
        );
    }
    if !is_genuine_brick(&fault.message) {
        return Some(
            "The vault file looks momentarily unavailable — often antivirus or Windows Search \
             holding it — rather than broken. Close this and choose \"Try again\"; it should \
             open once the file is free.",
        );
    }
    None
}

/// Recover from a boot-time vault-open failure by deleting the unreadable store and starting fresh.
/// This is the escape hatch behind the VaultOpenError screen's "Start fresh": a Device vault whose
/// key was lost (e.g. an interrupted "Remove PM data", or the historical delete-key-before-file bug)
/// regenerates a fresh key at boot that can't decrypt the old `pm.sqlite`, so the open-error screen
/// loops forever. Deleting the orphaned store + metadata lets the next boot create a clean, empty
/// vault (onboarding). The caller relaunches the app afterwards.
///
/// **Permitted ONLY for a genuine wrong-key / corrupt-file failure** — a deterministic brick whose
/// data is unrecoverable regardless. A boot open-error is *also* raised for a transient AV /
/// Windows-Search lock or a disk-I/O blip on a perfectly healthy vault (the B1-6 Retry path); that
/// lock can clear on its own, so deleting then would destroy a recoverable vault. `db::open` already
/// tags the genuine brick with a distinct message ([`crate::db::WRONG_KEY_OR_CORRUPT_MSG`]); we key
/// on exactly that and refuse otherwise, sending the user back to "Try again". Keychain secrets are
/// left untouched — the regenerated DB key simply becomes the key for the new empty store.
#[tauri::command]
pub fn reset_after_open_error(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    let Some(fault) = state.vault_fault() else {
        return Err(Error::Other(
            "The vault opened normally — there's nothing to reset.".into(),
        ));
    };
    // The pure gate: denied is never a brick, transient hiccups get "Try again" — only the
    // deterministic wrong-key / corrupt-file failure may delete anything.
    if let Some(refusal) = reset_refusal(&fault) {
        return Err(Error::Other(refusal.into()));
    }

    let data_dir = paths::data_dir(&app)?;
    // Never delete a vault this profile only POINTS at (a joined/shared vault): the folder isn't
    // ours to destroy, and the owner still opens it fine. A pointed vault that stops answering
    // (owner made it private, revoked access, changed the passphrase) is recovered by stepping
    // back to a local vault — `detach_from_shared_vault` — not by wiping the shared folder.
    if vault::pointer::load(&data_dir)?.is_some() {
        return Err(Error::Other(
            "This vault lives in a shared folder that PM is only pointed at, so it can't be reset \
             from here — it may belong to another account. Use \"Use a vault on this account \
             instead\" to step back to your own vault; the shared folder is left untouched."
                .into(),
        ));
    }

    let resolved = vault::resolve(&app)?;
    // The store never opened, so there's no live connection — but be safe.
    let _ = state.take_conn();
    if !remove_db_files_retrying(&resolved.db_path) {
        return Err(Error::Other(
            "Couldn't remove the vault file — it may be held open by antivirus or Windows Search. \
             Close other apps and try again."
                .into(),
        ));
    }
    remove_vault_artifacts(&resolved, &data_dir);
    state.set_vault_fault(None);
    Ok(())
}

/// Launch the Windows uninstaller (`uninstall.exe`, alongside the running executable) and return so
/// the caller can exit. Used only after a full "remove PM completely" wipe: the app has cleared its
/// data + armed the purge marker, and the uninstaller — running once PM exits — removes the program
/// files and the leftover data/webview folders, so nothing remains. On non-Windows, or a dev build
/// with no installed uninstaller, this returns an error the UI turns into a "remove it from your OS"
/// hint; the user's data is already gone regardless.
#[tauri::command]
pub fn launch_uninstaller() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe()
            .map_err(|e| Error::Other(format!("couldn't locate the app executable: {e}")))?;
        let uninstaller = exe
            .parent()
            .map(|d| d.join("uninstall.exe"))
            .filter(|p| p.exists())
            .ok_or_else(|| {
                Error::Other(
                    "Couldn't find the uninstaller next to the app. Your data is already removed — \
                     finish by uninstalling PM from Windows Settings → Apps."
                        .into(),
                )
            })?;
        std::process::Command::new(uninstaller)
            .spawn()
            .map_err(|e| Error::Other(format!("couldn't start the uninstaller: {e}")))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(Error::Other(
            "Automatic uninstall is only wired up on Windows — remove PM through your OS to finish."
                .into(),
        ))
    }
}

/// Run an OS user-presence check (Windows Hello / Touch ID) as the penultimate gate of the wipe
/// ladder, reusing the app-lock verifier. Returns `true` only on a successful verification, `false`
/// when the user cancels/fails; `Err` when the verifier can't run at all (the UI treats that like a
/// cancel so no one is trapped). Mirrors `commands::unlock_app`'s HWND handling but has no session
/// side effect — it never flips the app-unlocked flag.
#[tauri::command]
pub async fn confirm_wipe_identity(window: tauri::WebviewWindow) -> Result<bool> {
    let raw_handle = {
        #[cfg(target_os = "windows")]
        {
            window
                .hwnd()
                .map_err(|e| Error::Other(format!("no window handle for verification: {e}")))?
                .0 as isize
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = &window;
            0isize
        }
    };
    tauri::async_runtime::spawn_blocking(move || {
        crate::applock::verify(raw_handle, "Confirm you want to remove PM data")
    })
    .await
    .map_err(|e| Error::Other(format!("verification task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn macos_leftovers_name_every_place_the_os_writes_for_us() {
        // These paths are the whole fix: each one is a place a wipe used to miss. Asserted by exact
        // shape because nothing on Windows CI can observe them, and a typo here is silent — the
        // wipe would simply skip a path that does not exist and report success.
        let home = Path::new("/Users/someone");
        let got = paths::macos_leftovers_in(home, "org.itsatlas.pm");
        let want = [
            "/Users/someone/Library/Application Support/org.itsatlas.pm",
            "/Users/someone/Library/WebKit/org.itsatlas.pm",
            "/Users/someone/Library/Caches/org.itsatlas.pm",
            "/Users/someone/Library/HTTPStorages/org.itsatlas.pm",
            "/Users/someone/Library/Preferences/org.itsatlas.pm.plist",
            "/Users/someone/Library/Saved Application State/org.itsatlas.pm.savedState",
            "/Users/someone/Library/HTTPStorages/org.itsatlas.pm.binarycookies",
        ];
        let got: Vec<String> = got
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(got, want, "the macOS leftover set must not drift");

        // The three Bobby hit by hand, named individually so a regression says which one went.
        for must in [
            "Library/WebKit/org.itsatlas.pm",
            "Library/Caches/org.itsatlas.pm",
            "Library/Preferences/org.itsatlas.pm.plist",
        ] {
            assert!(
                got.iter().any(|p| p.ends_with(must)),
                "{must} must be removed by Remove PM data"
            );
        }
    }

    #[test]
    fn linux_leftovers_name_the_webkitgtk_store_the_erase_used_to_skip() {
        // Tauri forces a webview data directory at `<local data>/<identifier>` on Linux as well as
        // Windows, and wry hands that exact path to WebKitGTK as BOTH its data and its cache base —
        // so this one directory is the app's localStorage, IndexedDB, service workers and cookie
        // jar. The wipe skipped it because the leftover sweep was macOS-only, and Linux has no
        // uninstaller behind it, so "Remove PM data" left it there permanently.
        let got =
            paths::linux_leftovers_in(Path::new("/home/someone/.local/share"), "org.itsatlas.pm");
        let got: Vec<String> = got
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(got, ["/home/someone/.local/share/org.itsatlas.pm"]);

        // Same one-way-door rule as macOS: read the identifier, never repeat it.
        let other = paths::linux_leftovers_in(Path::new("/x"), "com.example.other");
        assert!(other
            .iter()
            .all(|p| p.to_string_lossy().contains("com.example.other")));
    }

    #[test]
    fn leftovers_are_keyed_to_the_bundle_identifier_not_a_hardcoded_name() {
        // The identifier is a one-way door (renaming it orphans the keychain), so the wipe reads it
        // rather than repeating it — otherwise the two could drift and the sweep would miss silently.
        let got = paths::macos_leftovers_in(Path::new("/h"), "com.example.other");
        assert!(got
            .iter()
            .all(|p| p.to_string_lossy().contains("com.example.other")));
        assert!(!got.iter().any(|p| p.to_string_lossy().contains("itsatlas")));
    }

    #[test]
    fn the_finish_step_is_only_set_by_a_total_wipe() {
        // Default must be None: a partial wipe leaves PM installed, and offering "drag me to the
        // Trash" after clearing only the downloaded components would be wrong on every platform.
        assert_eq!(WipeReport::default().finish_step, FinishStep::None);
    }

    #[test]
    fn a_detached_shared_vaults_cached_key_is_named_by_the_wipe() {
        // The bug: this collected ONLY the resolved vault's id. `detach_from_shared_vault` keeps
        // its cached key on purpose (for a silent rejoin), so after a detach the profile resolves
        // its own device vault and the shared vault's raw SQLCipher master key survived "Remove PM
        // data" — for a vault that may still be sitting in a folder on the same machine.
        let ids = cached_vault_ids_to_wipe(
            vec!["shared-vault".into()],
            Some("device-vault".into()),
            None,
        );
        assert!(
            ids.contains(&"shared-vault".to_string()),
            "a key cached for a vault we no longer point at must still be wiped"
        );
        assert!(ids.contains(&"device-vault".to_string()));
    }

    #[test]
    fn a_lost_registry_degrades_to_the_old_behaviour_never_below_it() {
        // The registry is best-effort (a keychain write can fail, and installs predating it have
        // none). Losing it must cost the extra ids, never the current vault's — otherwise this
        // "fix" would wipe LESS than the code it replaced.
        let ids = cached_vault_ids_to_wipe(vec![], Some("device-vault".into()), None);
        assert_eq!(ids, vec!["device-vault".to_string()]);
    }

    #[test]
    fn the_retired_pointer_covers_a_detach_that_predates_the_registry() {
        // Third source: a profile that detached before the registry shipped has no registry entry,
        // but its retired pointer still names the folder — and that folder's meta still names the
        // id whose key we cached.
        let ids = cached_vault_ids_to_wipe(vec![], None, Some("old-shared".into()));
        assert_eq!(ids, vec!["old-shared".to_string()]);
    }

    #[test]
    fn wipe_ids_never_repeat() {
        // All three sources routinely name the SAME vault (registry + resolved is the common
        // case). A duplicate would delete the same key twice and inflate the entries-removed count.
        let ids = cached_vault_ids_to_wipe(
            vec!["v1".into(), "v2".into()],
            Some("v1".into()),
            Some("v2".into()),
        );
        assert_eq!(ids, vec!["v1".to_string(), "v2".to_string()]);
    }

    /// A minimal in-memory `connector_sources` with just the columns the enumeration reads, so the
    /// pure provider/service → token-key mapping can be tested without the full migration stack.
    fn conn_with_accounts(rows: &[(&str, &str, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE connector_sources (provider TEXT, service TEXT, account_email TEXT);",
        )
        .unwrap();
        for (provider, service, email) in rows {
            conn.execute(
                "INSERT INTO connector_sources(provider, service, account_email) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![provider, service, email],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn enumerates_oauth_token_keys_by_provider_and_service() {
        let conn = conn_with_accounts(&[
            ("google", "drive", "a@gmail.com"),
            ("google", "calendar", "b@gmail.com"),
            ("microsoft", "onedrive", "c@outlook.com"),
            ("microsoft", "calendar", "d@outlook.com"),
            ("apple", "calendar", "e@icloud.com"), // subscription — no keychain token
            ("local", "folder", "some-folder"),    // local folder — no keychain token
        ]);
        let accounts = enumerate_oauth_accounts(&conn).unwrap();

        // Only the four OAuth connectors produce a token key; apple/local carry none.
        assert_eq!(accounts.len(), 4);

        let key_for = |email: &str| {
            accounts
                .iter()
                .find(|a| a.email == email)
                .map(|a| a.token_key.clone())
                .unwrap_or_default()
        };
        // Each account's key is reconstructed from the right per-service prefix.
        assert_eq!(
            key_for("a@gmail.com"),
            format!("{}a@gmail.com", secrets::GOOGLE_TOKEN_DRIVE_PREFIX)
        );
        assert_eq!(
            key_for("b@gmail.com"),
            format!("{}b@gmail.com", secrets::GOOGLE_TOKEN_CALENDAR_PREFIX)
        );
        assert_eq!(
            key_for("c@outlook.com"),
            format!("{}c@outlook.com", secrets::MICROSOFT_TOKEN_ONEDRIVE_PREFIX)
        );
        assert_eq!(
            key_for("d@outlook.com"),
            format!("{}d@outlook.com", secrets::MICROSOFT_TOKEN_CALENDAR_PREFIX)
        );

        // Providers are classified so only Google is revoked at the provider's end (Microsoft has no
        // programmatic revoke), and the two Microsoft accounts fall to the manual-link path.
        let google = accounts
            .iter()
            .filter(|a| a.provider == Provider::Google)
            .count();
        let microsoft = accounts
            .iter()
            .filter(|a| a.provider == Provider::Microsoft)
            .count();
        assert_eq!((google, microsoft), (2, 2));
    }

    #[test]
    fn skips_rows_with_no_email() {
        // An empty account_email never yields a dangling `prefix::` key (filtered in SQL).
        let conn = conn_with_accounts(&[("google", "drive", "")]);
        assert!(enumerate_oauth_accounts(&conn).unwrap().is_empty());
    }

    // --- "Start fresh" may delete ONLY a genuine brick, never a transiently-locked healthy vault ---

    fn fault_with(code: crate::error::VaultFaultCode, message: &str) -> crate::error::VaultFault {
        crate::error::VaultFault {
            code,
            op: "open the vault".into(),
            path: None,
            message: message.into(),
        }
    }

    #[test]
    fn a_denied_vault_never_arms_the_reset() {
        // The ACL-lockout pin: an access-DENIED vault is intact data behind a permissions
        // problem — "Start fresh" must refuse EVEN IF the message happens to contain the
        // genuine-brick literal (belt-and-braces against message drift).
        let denied = fault_with(
            crate::error::VaultFaultCode::Denied,
            crate::db::WRONG_KEY_OR_CORRUPT_MSG,
        );
        assert!(reset_refusal(&denied).unwrap().contains("Repair access"));
        // A transient hiccup keeps its "Try again" refusal…
        let transient = fault_with(
            crate::error::VaultFaultCode::Other,
            "the database is in use by another program",
        );
        assert!(reset_refusal(&transient).unwrap().contains("Try again"));
        // …and only the deterministic brick passes.
        let brick = fault_with(
            crate::error::VaultFaultCode::Other,
            crate::db::WRONG_KEY_OR_CORRUPT_MSG,
        );
        assert_eq!(reset_refusal(&brick), None);
    }

    #[test]
    fn only_a_genuine_brick_arms_the_reset() {
        // The deterministic wrong-key / corrupt-file failure is the one (and only) case reset deletes.
        assert!(is_genuine_brick(crate::db::WRONG_KEY_OR_CORRUPT_MSG));
        // The transient messages db::open tags distinctly must NOT arm deletion — the vault may still
        // open once the lock clears, so destroying it here would lose a healthy vault.
        assert!(!is_genuine_brick(
            "the database is in use by another program; close other copies of PM (or your \
             antivirus's lock) and try again"
        ));
        assert!(!is_genuine_brick(
            "could not read the database file (disk I/O error): some os error"
        ));
        assert!(!is_genuine_brick(""));
    }

    #[test]
    fn a_pointed_vault_is_only_deleted_when_it_is_provably_ours() {
        use vault::VaultOwnership::*;
        // The whole safety argument for deleting outside PM's own folder. Every branch that is not
        // a proof must leave the folder alone — the opposite polarity to `is_vault_owner`, which
        // fails OPEN so a SID hiccup never locks an owner out of their connectors. Here a SID
        // hiccup must never destroy data instead.
        assert_eq!(
            pointed_vault_verdict(Some(Device), false),
            PointedVaultVerdict::Delete,
            "a device vault's key lives in this account's keychain alone, wherever the folder sits"
        );
        assert_eq!(
            pointed_vault_verdict(Some(Owned), false),
            PointedVaultVerdict::Delete,
            "a shared vault stamped with our own SID is ours"
        );
        assert_eq!(
            pointed_vault_verdict(Some(Joined), false),
            PointedVaultVerdict::Leave(LeaveReason::OwnedByAnotherAccount)
        );
        assert_eq!(
            pointed_vault_verdict(Some(Unknown), false),
            PointedVaultVerdict::Leave(LeaveReason::OwnerUnknown),
            "no recorded owner is not the same as ours -- and off Windows this is EVERY shared vault"
        );
        assert_eq!(
            pointed_vault_verdict(None, false),
            PointedVaultVerdict::Leave(LeaveReason::MetaUnreadable)
        );

        // A tampered meta outranks everything: the ownership stamp lives inside the file whose
        // integrity just failed, so it cannot be the thing that authorises a delete.
        for ownership in [Device, Owned, Joined, Unknown] {
            assert_eq!(
                pointed_vault_verdict(Some(ownership), true),
                PointedVaultVerdict::Leave(LeaveReason::MetaTampered),
                "{ownership:?} must not be trusted from a meta that failed its integrity check"
            );
        }
    }

    #[test]
    fn the_temp_sweep_matches_pms_own_staging_and_nothing_else() {
        // This sweep deletes files in a directory shared with every other program on the machine,
        // so the matching rule is the thing that must never widen. Each prefix is PM-minted and
        // followed by a random suffix.
        let m = |n: &str| is_pm_temp_staging(n, "PM");
        assert!(m("pm-drive-a1B2c3.pdf"));
        assert!(m("pm-onedrive-XyZ.docx"));
        assert!(m("pm-restore-gdrive-99"));
        assert!(m("pm-backup-snap-7"));
        // The updater's own staging. It exits the process rather than dropping its temp guard, so
        // every installer it has ever applied is still sitting there, around 100 MB each.
        assert!(m("PM-3.111.0-alpha-updater-Ab12"));

        for other in [
            "pm.sqlite",                  // never in temp, but the closest-looking real filename
            "pm-check.log",               // a developer's own scratch file, seen in the wild
            "personal-manager-notes.txt", // a user's file that merely starts similarly
            "drive-export.pdf",
            "PM-notes.txt",        // our app name, but not the updater's shape
            "PMS-1-updater-x",     // another program whose name merely starts with ours
            "something-updater-x", // the infix alone is not enough
            "",
        ] {
            assert!(!m(other), "{other} is not PM staging");
        }
    }

    #[test]
    fn sweeping_temp_removes_a_stranded_download_and_leaves_the_rest() {
        // End to end against the real temp dir, using names no other process would mint.
        let tmp = std::env::temp_dir();
        let staged = tmp.join("pm-drive-wipetest-90210.pdf");
        let innocent = tmp.join("pm-notes-wipetest-90210.txt");
        std::fs::write(&staged, b"a user's actual document").unwrap();
        std::fs::write(&innocent, b"not PM's").unwrap();

        sweep_temp_staging("PM");

        assert!(
            !staged.exists(),
            "an abandoned staged download is collected"
        );
        assert!(
            innocent.exists(),
            "anything outside the prefix list is untouched"
        );
        let _ = std::fs::remove_file(&innocent);
    }

    #[test]
    fn the_erase_removes_every_file_the_vault_list_names() {
        // The wipe's artifact list used to be typed out by hand beside `vault_sidecar_files`, and had
        // already drifted: `vault-access.json` — the OS accounts linked to a shared vault — was in
        // that list and not in this one, so "Remove PM data" left it behind. Driving both from one
        // enumeration is the fix; this asserts the outcome rather than the mechanism, so a file added
        // to the list later is covered without anyone remembering to come back here.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("vault")).unwrap();
        for file in vault::migrate::vault_sidecar_files(&root) {
            std::fs::write(&file, b"x").unwrap();
        }
        std::fs::write(root.join("vault").join("note.md"), b"# note").unwrap();
        let unrelated = root.join("taxes-2025.xlsx");
        std::fs::write(&unrelated, b"not PM's to delete").unwrap();

        let resolved = vault::resolve_layout(&root, None);
        remove_vault_artifacts(&resolved, &root);

        for file in vault::migrate::vault_sidecar_files(&root) {
            assert!(!file.exists(), "{} survived the erase", file.display());
        }
        assert!(!root.join("vault").exists(), "the Markdown tree goes too");
        assert!(unrelated.exists(), "and nothing that isn't PM's is touched");
    }

    // --- F-03: a relocated vault's user-chosen root must never be deleted wholesale ---

    #[test]
    fn removing_the_vault_root_deletes_it_only_when_empty() {
        let tmp = tempfile::tempdir().unwrap();

        // The happy case: PM had the relocated folder to itself, so after its artifacts are gone the
        // root is empty and gets tidied away.
        let pm_only = tmp.path().join("pm-only-root");
        std::fs::create_dir(&pm_only).unwrap();
        remove_empty_dir_retrying(&pm_only);
        assert!(
            !pm_only.exists(),
            "an empty relocated root should be removed"
        );

        // The dangerous case: the user pointed the vault at a folder that already held their own
        // files (e.g. `D:\Documents`). PM's artifacts were removed individually upstream; the leftover
        // unrelated files must survive, and the folder itself must be left standing.
        let shared = tmp.path().join("Documents");
        std::fs::create_dir(&shared).unwrap();
        let their_file = shared.join("taxes-2025.xlsx");
        std::fs::write(&their_file, b"not PM's to delete").unwrap();
        let their_subdir = shared.join("photos");
        std::fs::create_dir(&their_subdir).unwrap();

        remove_empty_dir_retrying(&shared);

        assert!(
            shared.exists(),
            "a non-empty relocated root must be left alone"
        );
        assert!(their_file.exists(), "the user's file must survive the wipe");
        assert!(
            their_subdir.exists(),
            "the user's subfolder must survive the wipe"
        );
        assert_eq!(std::fs::read(&their_file).unwrap(), b"not PM's to delete");
    }

    #[test]
    fn removing_a_missing_root_is_a_no_op() {
        // The default-location case (vault_root == data_dir) never calls this, but a caller that
        // passes an already-gone path must not error.
        let tmp = tempfile::tempdir().unwrap();
        remove_empty_dir_retrying(&tmp.path().join("never-existed"));
    }

    // --- F-25: boot-time GC of abandoned, decryptable restore staging ---

    #[test]
    fn sweep_removes_stale_copies_but_keeps_the_active_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let staging = data_dir.join(RESTORE_STAGING_DIR);
        std::fs::create_dir_all(&staging).unwrap();

        // Two staged restores: one is the vault the user switched to (still the LIVE vault, living
        // under `restored-vaults/`); the other is an inspected-then-abandoned decryptable copy.
        let active = staging.join("restore-active");
        let stale = staging.join("restore-stale");
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("pm.sqlite"), b"live").unwrap();
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("pm.sqlite"), b"abandoned decryptable copy").unwrap();

        sweep_restore_staging(data_dir, &active);

        assert!(
            active.exists(),
            "the switched-to (live) restore must survive"
        );
        assert!(active.join("pm.sqlite").exists());
        assert!(!stale.exists(), "an abandoned restore copy must be swept");
    }

    #[test]
    fn sweep_removes_everything_when_running_from_the_default_location() {
        // The common case: the profile runs from its default data dir, not a switched-to restore, so
        // every staged copy is abandoned residue.
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let staging = data_dir.join(RESTORE_STAGING_DIR);
        std::fs::create_dir_all(&staging).unwrap();
        let a = staging.join("restore-1");
        let b = staging.join("restore-2");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        sweep_restore_staging(data_dir, data_dir);

        assert!(
            !a.exists() && !b.exists(),
            "all staged copies must be swept"
        );
    }

    #[test]
    fn sweep_is_a_no_op_without_a_staging_dir() {
        // No `restored-vaults/` at all — must not error.
        let tmp = tempfile::tempdir().unwrap();
        sweep_restore_staging(tmp.path(), tmp.path());
    }

    #[test]
    fn sweep_skips_entirely_when_the_active_root_is_unresolvable() {
        // Fail-safe: if the active root can't be canonicalised (transiently absent), do nothing rather
        // than risk deleting the live vault by mismatching an unresolved path.
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let staging = data_dir.join(RESTORE_STAGING_DIR);
        std::fs::create_dir_all(&staging).unwrap();
        let copy = staging.join("restore-1");
        std::fs::create_dir(&copy).unwrap();

        sweep_restore_staging(data_dir, &data_dir.join("does-not-exist"));

        assert!(
            copy.exists(),
            "with an unresolvable active root the sweep must delete nothing"
        );
    }
}
