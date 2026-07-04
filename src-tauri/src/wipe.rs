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
//!   4. **Browser local storage** — handled entirely in the webview (`localStorage.clear()`), so it
//!      isn't part of this backend command.
//!
//! Backups are intentionally NOT touched here: local `.pmbackup` files and any Proton/Google Drive
//! destination are separate, and the UI directs the user to remove those at the source.
//!
//! Every step is best-effort and independent — this runs when the user is deliberately erasing PM,
//! so "remove as much as possible, report honestly" beats "abort on the first locked file".

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::error::{Error, Result};
use crate::{google, paths, secrets, vault, AppState};

/// Which classes of data to remove. Mirrors the four checkboxes; `camelCase` to match the webview.
/// `local_storage` is cleared in the frontend, so it isn't represented here.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeSelection {
    /// The regenerable `runtime/` (venv + t-SNE/OCR + Whisper + bundled interpreter).
    pub regenerable: bool,
    /// The vault, encrypted store, and vault artifacts. Irreversible.
    pub vault_and_db: bool,
    /// Every keychain secret; implies revoking Google grants + reporting Microsoft accounts.
    pub keychain: bool,
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
        let account = match (provider.as_str(), service.as_str()) {
            ("google", "drive") => Some(OauthAccount {
                token_key: format!("{}{}", secrets::GOOGLE_TOKEN_DRIVE_PREFIX, email),
                email,
                provider: Provider::Google,
            }),
            ("google", "calendar") => Some(OauthAccount {
                token_key: format!("{}{}", secrets::GOOGLE_TOKEN_CALENDAR_PREFIX, email),
                email,
                provider: Provider::Google,
            }),
            ("microsoft", "onedrive") => Some(OauthAccount {
                token_key: format!("{}{}", secrets::MICROSOFT_TOKEN_ONEDRIVE_PREFIX, email),
                email,
                provider: Provider::Microsoft,
            }),
            ("microsoft", "calendar") => Some(OauthAccount {
                token_key: format!("{}{}", secrets::MICROSOFT_TOKEN_CALENDAR_PREFIX, email),
                email,
                provider: Provider::Microsoft,
            }),
            // Apple subscriptions / local folders hold no OAuth token in the keychain.
            _ => None,
        };
        if let Some(a) = account {
            out.push(a);
        }
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

/// Execute the confirmed removal. The UI has already gated this behind the full confirmation ladder
/// (explicit checkboxes → "are you sure" itemisation → optional Windows Hello → type-to-confirm), so
/// this simply carries it out in a safe order and reports what happened.
///
/// Order matters: accounts are enumerated and Google grants revoked **while the store is still open**
/// (the account list lives in the DB); the keychain is wiped next; only then is the store closed and
/// its files deleted; the regenerable runtime goes last. See the module docs for the four classes.
#[tauri::command]
pub async fn wipe_pm_data(
    app: AppHandle,
    state: State<'_, AppState>,
    selection: WipeSelection,
) -> Result<WipeReport> {
    let mut report = WipeReport::default();

    // --- OS keychain (+ OAuth revoke). Enumerate accounts + the vault id before any teardown. ---
    if selection.keychain {
        // Collect everything we need from the open store into owned values, then drop the guard so
        // nothing is held across the `.await` revoke calls below.
        let (accounts_result, vault_ids) = {
            let conn = state.conn()?;
            let accounts = enumerate_oauth_accounts(&conn);
            // The current vault's cached-key entry (`vault_key::<id>`); best-effort — a plain device
            // vault has no cached key, and a missing meta just yields no id.
            let vault_ids = vault::resolve(&app)
                .ok()
                .and_then(|r| vault::load_meta(&r.vault_root).ok().flatten())
                .map(|m| vec![m.vault_id])
                .unwrap_or_default();
            (accounts, vault_ids)
        };
        let accounts = accounts_result.unwrap_or_default();

        // Revoke Google grants at Google's end, then record Microsoft accounts for the manual link.
        let mut google_token_keys = Vec::new();
        let mut microsoft_token_keys = Vec::new();
        let mut google_emails = Vec::new();
        for a in accounts {
            match a.provider {
                Provider::Google => {
                    if let Ok(Some(blob)) = secrets::get_google_token_for(&a.token_key) {
                        match google::revoke(blob.expose()).await {
                            Ok(()) => report.google_revoked += 1,
                            Err(_) => report.google_revoke_failures += 1,
                        }
                    }
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

        let mut token_keys = google_token_keys;
        token_keys.extend(microsoft_token_keys);
        report.keychain_deleted =
            secrets::wipe_all_secrets(&token_keys, &google_emails, &vault_ids);
        report.removed.push("Keychain secrets & saved keys".into());
        report.quit_required = true;
    }

    // --- Vault & database. Close the store first so the DB file's lock is released. ---
    if selection.vault_and_db {
        let resolved = vault::resolve(&app)?;
        let data_dir = paths::data_dir(&app)?;

        // Size the user data before removing it (DB file + Markdown tree).
        report.freed_bytes += std::fs::metadata(&resolved.db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        report.freed_bytes += dir_size(&resolved.markdown_dir);

        // Drop the live connection (its `Drop` releases SQLite's file lock), then delete the files.
        let _ = state.take_conn();

        for suffix in ["", "-wal", "-shm"] {
            let p = resolved.db_path.with_extension(format!("sqlite{suffix}"));
            let _ = std::fs::remove_file(&p);
        }
        let _ = std::fs::remove_file(resolved.vault_root.join(vault::META_FILENAME));
        let _ = std::fs::remove_file(resolved.vault_root.join(crate::entities::RULES_FILENAME));
        let _ = std::fs::remove_file(
            resolved
                .vault_root
                .join(crate::index_only::MANIFEST_FILENAME),
        );
        let _ = vault::lock::clear_baton_files(&resolved.vault_root);
        let _ = vault::migrate::clear_journal(&resolved.vault_root);
        remove_dir_all_retrying(&resolved.markdown_dir);
        // A relocated vault's external root can go wholesale; also clear the pointer so PM reverts to
        // the default location on next launch instead of chasing a folder that no longer exists.
        if resolved.vault_root != data_dir {
            remove_dir_all_retrying(&resolved.vault_root);
        }
        let _ = vault::pointer::clear(&data_dir);

        report.removed.push("Vault & encrypted database".into());
        report.quit_required = true;
    }

    // --- Regenerable components. Stop the sidecar (releases the interpreter's locks), then remove. ---
    if selection.regenerable {
        state.sidecar.prepare_for_runtime_removal();
        let runtime = paths::data_dir(&app)?.join("runtime");
        report.freed_bytes += dir_size(&runtime);
        remove_dir_all_retrying(&runtime);
        report
            .removed
            .push("Downloaded components (engine, models)".into());
    }

    if report.removed.is_empty() {
        return Err(Error::Other("Nothing was selected to remove.".into()));
    }
    Ok(report)
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
}
