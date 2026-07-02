// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! All secrets live in the OS keychain — never on disk, never in the repo
//! (spec §6, §8.7). Entries are namespaced under a reverse-DNS service id that
//! matches the app's bundle identifier, so they can't collide with other apps'
//! keychain entries.

use keyring::Entry;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::secret::Secret;

// Reverse-DNS to match the Tauri bundle identifier (`tauri.conf.json`). Keep the
// two in step: renaming this orphans every existing keychain entry (including the
// DB key, which makes the old encrypted store unreadable).
const SERVICE: &str = "org.itsatlas.pm";
const OPENROUTER_KEY: &str = "openrouter_api_key";
/// A separate key for non-interactive background work (sorting-review proposals,
/// the Learning-You profile distillation), so the user can see at a glance which
/// OpenRouter spend is interactive chat vs background processing (Step 4).
const OPENROUTER_BACKGROUND_KEY: &str = "openrouter_background_key";
const DB_KEY: &str = "db_encryption_key";
/// The backup passphrase, stored ONLY when the user opts in to automatic (scheduled) backups —
/// unattended runs can't prompt for it. Same keychain trust as [`DB_KEY`]: it is the sole secret
/// protecting the `.pmbackup` archives, so opting in makes the backups' confidentiality depend on
/// the OS keychain. Each archive still derives a fresh Argon2id salt, so reuse is safe. Manual
/// backups never touch this — they always prompt.
const BACKUP_PASSPHRASE: &str = "backup_passphrase";
/// Google OAuth (Step 6): the user's BYO "Desktop app" client (id + secret) and the
/// resulting token blob. No Google secret ships in the repo (rule #1) — the user
/// supplies their own client; everything lives only in the keychain.
const GOOGLE_CLIENT_ID: &str = "google_oauth_client_id";
const GOOGLE_CLIENT_SECRET: &str = "google_oauth_client_secret";
/// Legacy single Google OAuth token key (pre per-service connectors). Migrated once on
/// startup to the per-service calendar key by [`migrate_legacy_google_token`], then never
/// written again.
const GOOGLE_TOKEN: &str = "google_oauth_token";
/// Per-service Google OAuth token keys. The client credentials above are provider-level
/// (shared by every Google service), but each SERVICE — and each Drive ACCOUNT — gets its
/// own token, so connecting or disconnecting one never disturbs another. Calendar has a
/// fixed key; a Drive account's key is `GOOGLE_TOKEN_DRIVE_PREFIX + <account-email>`.
pub const GOOGLE_TOKEN_CALENDAR: &str = "google_oauth_token_calendar";
pub const GOOGLE_TOKEN_DRIVE_PREFIX: &str = "google_oauth_token_drive::";
/// Per-ACCOUNT Google Calendar token key prefix (`<prefix><email>`) — Calendar went multi-account
/// (cards 6A/6B) like Drive, so each connected Google account gets its own token. The fixed
/// `GOOGLE_TOKEN_CALENDAR` above is now LEGACY: an existing single-account connection is re-keyed to
/// `<prefix><that-account-email>` lazily on the first calendar overview/sync (the email is learned
/// from the account's primary calendar), then never written again.
pub const GOOGLE_TOKEN_CALENDAR_PREFIX: &str = "google_oauth_token_calendar::";
/// Subscribed .ics feed URLs (the no-OAuth calendar path). These are secret bearer
/// links, so the whole JSON list lives in the keychain, not the DB.
const CALENDAR_ICS_FEEDS: &str = "calendar_ics_feeds";

fn entry(name: &str) -> Result<Entry> {
    Entry::new(SERVICE, name).map_err(Error::from)
}

fn get(name: &str) -> Result<Option<String>> {
    match entry(name)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::from(e)),
    }
}

fn set(name: &str, value: &str) -> Result<()> {
    entry(name)?.set_password(value).map_err(Error::from)
}

/// Remove an entry; absent is success (so disconnect is idempotent).
fn delete(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::from(e)),
    }
}

/// The stored backup passphrase for unattended (scheduled) backups, if the user opted in.
pub fn get_backup_passphrase() -> Result<Option<Secret>> {
    Ok(get(BACKUP_PASSPHRASE)?.map(Secret::from))
}

/// Store the backup passphrase for unattended backups (explicit opt-in only).
pub fn set_backup_passphrase(value: &str) -> Result<()> {
    set(BACKUP_PASSPHRASE, value)
}

/// Forget the stored backup passphrase; absent is success (so "turn off" is idempotent).
pub fn delete_backup_passphrase() -> Result<()> {
    delete(BACKUP_PASSPHRASE)
}

pub fn get_openrouter_key() -> Result<Option<Secret>> {
    Ok(get(OPENROUTER_KEY)?.map(Secret::from))
}

pub fn set_openrouter_key(value: &str) -> Result<()> {
    set(OPENROUTER_KEY, value)
}

pub fn get_openrouter_background_key() -> Result<Option<Secret>> {
    Ok(get(OPENROUTER_BACKGROUND_KEY)?.map(Secret::from))
}

pub fn set_openrouter_background_key(value: &str) -> Result<()> {
    set(OPENROUTER_BACKGROUND_KEY, value)
}

/// The key for background work: the dedicated background key if the user set one,
/// otherwise the primary key as a fallback (so proposals/learning work before a
/// second key is configured). Errors only if neither is set.
pub fn get_background_or_primary_key() -> Result<Option<Secret>> {
    match get_openrouter_background_key()? {
        Some(key) => Ok(Some(key)),
        None => get_openrouter_key(),
    }
}

// --- Google OAuth (Step 6) ---

pub fn get_google_client_id() -> Result<Option<String>> {
    get(GOOGLE_CLIENT_ID)
}

pub fn get_google_client_secret() -> Result<Option<Secret>> {
    Ok(get(GOOGLE_CLIENT_SECRET)?.map(Secret::from))
}

/// Store the user's BYO Google client credentials together.
pub fn set_google_client(client_id: &str, client_secret: &str) -> Result<()> {
    set(GOOGLE_CLIENT_ID, client_id)?;
    set(GOOGLE_CLIENT_SECRET, client_secret)
}

/// Forget the client credentials (used when the user clears the connector).
pub fn clear_google_client() -> Result<()> {
    delete(GOOGLE_CLIENT_ID)?;
    delete(GOOGLE_CLIENT_SECRET)
}

/// **Per-account** Google client (id + secret). Most users share the one client above, but a Google
/// **Advanced Protection** account can't authorize a third-party Cloud project — it must use a client
/// from a project the account itself owns. So each such account can carry its OWN client, keyed by its
/// email; the OAuth flow and every later token refresh for that account use it instead of the shared
/// one (resolved in `google::client_creds_for_key`). Absent → the account falls back to the shared
/// client. The `::` keeps these distinct from the shared keys (which have no suffix).
const GOOGLE_CLIENT_ID_PREFIX: &str = "google_oauth_client_id::";
const GOOGLE_CLIENT_SECRET_PREFIX: &str = "google_oauth_client_secret::";

pub fn get_google_client_id_for_account(email: &str) -> Result<Option<String>> {
    get(&format!("{GOOGLE_CLIENT_ID_PREFIX}{email}"))
}

pub fn get_google_client_secret_for_account(email: &str) -> Result<Option<Secret>> {
    Ok(get(&format!("{GOOGLE_CLIENT_SECRET_PREFIX}{email}"))?.map(Secret::from))
}

/// Store an account's own client credentials together (overwrites — reconnecting re-sets the same).
pub fn set_google_client_for_account(
    email: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<()> {
    set(&format!("{GOOGLE_CLIENT_ID_PREFIX}{email}"), client_id)?;
    set(
        &format!("{GOOGLE_CLIENT_SECRET_PREFIX}{email}"),
        client_secret,
    )
}

/// Forget an account's own client credentials (idempotent).
pub fn clear_google_client_for_account(email: &str) -> Result<()> {
    delete(&format!("{GOOGLE_CLIENT_ID_PREFIX}{email}"))?;
    delete(&format!("{GOOGLE_CLIENT_SECRET_PREFIX}{email}"))
}

/// Read a per-service Google OAuth token blob by its keychain key (calendar, or a Drive account).
pub fn get_google_token_for(key: &str) -> Result<Option<Secret>> {
    Ok(get(key)?.map(Secret::from))
}

/// Store a per-service Google OAuth token blob under its keychain key.
pub fn set_google_token_for(key: &str, value: &str) -> Result<()> {
    set(key, value)
}

/// Forget a per-service Google OAuth token (disconnect that service/account). The provider
/// client credentials are left in place so the user can reconnect without re-entering them.
/// Absent is success, so disconnect is idempotent.
pub fn clear_google_token_for(key: &str) -> Result<()> {
    delete(key)
}

/// One-time migration: move the calendar token from the legacy shared `google_oauth_token`
/// key to its per-service key, so an existing Calendar connection survives the move to the
/// per-service token model without a reconnect. Idempotent — a no-op once migrated (or when
/// there was never a legacy token).
pub fn migrate_legacy_google_token() -> Result<()> {
    if get(GOOGLE_TOKEN_CALENDAR)?.is_none() {
        if let Some(token) = get(GOOGLE_TOKEN)? {
            set(GOOGLE_TOKEN_CALENDAR, &token)?;
            delete(GOOGLE_TOKEN)?;
        }
    }
    Ok(())
}

// --- Microsoft OAuth (board card 4B — OneDrive) ---
//
// The user's BYO Microsoft Entra "Mobile & desktop" app registration. Microsoft desktop apps are
// PUBLIC clients: PKCE auth-code with NO client secret, so only a client id is stored (there is no
// secret to ship — rule #1 holds for free). Each connected OneDrive account gets its own token blob
// under `MICROSOFT_TOKEN_ONEDRIVE_PREFIX + <email>`, mirroring the per-account Drive tokens, so
// connecting or disconnecting one account never disturbs another.
const MICROSOFT_CLIENT_ID: &str = "microsoft_oauth_client_id";
pub const MICROSOFT_TOKEN_ONEDRIVE_PREFIX: &str = "microsoft_oauth_token_onedrive::";
/// Per-account Microsoft (Outlook) Calendar token key prefix (`<prefix><email>`) — the Graph
/// `Calendars.Read` OAuth path (card 6A). Mirrors the OneDrive prefix: the same provider-level client
/// id (shared with OneDrive — no new client setup), one token blob per connected account.
pub const MICROSOFT_TOKEN_CALENDAR_PREFIX: &str = "microsoft_oauth_token_calendar::";

pub fn get_microsoft_client_id() -> Result<Option<String>> {
    get(MICROSOFT_CLIENT_ID)
}

/// Store the user's BYO Microsoft client id (public client — there is no secret).
pub fn set_microsoft_client(client_id: &str) -> Result<()> {
    set(MICROSOFT_CLIENT_ID, client_id)
}

/// Forget the Microsoft client id (used when the user clears the connector).
pub fn clear_microsoft_client() -> Result<()> {
    delete(MICROSOFT_CLIENT_ID)
}

/// Read a per-account Microsoft OAuth token blob by its keychain key.
pub fn get_microsoft_token_for(key: &str) -> Result<Option<Secret>> {
    Ok(get(key)?.map(Secret::from))
}

/// Store a per-account Microsoft OAuth token blob under its keychain key.
pub fn set_microsoft_token_for(key: &str, value: &str) -> Result<()> {
    set(key, value)
}

/// Forget a per-account Microsoft OAuth token (disconnect that account). Absent is success, so
/// disconnect is idempotent.
pub fn clear_microsoft_token_for(key: &str) -> Result<()> {
    delete(key)
}

pub fn get_ics_feeds() -> Result<Option<Secret>> {
    Ok(get(CALENDAR_ICS_FEEDS)?.map(Secret::from))
}

pub fn set_ics_feeds(value: &str) -> Result<()> {
    set(CALENDAR_ICS_FEEDS, value)
}

/// Returns the database encryption key, generating and persisting a fresh
/// 256-bit random key on first run. The key never touches disk in plaintext.
/// Returned as a [`Secret`] so the in-memory copy is zeroized once the caller drops
/// it (as before) and can never be printed to a log or error.
pub fn get_or_create_db_key() -> Result<Secret> {
    if let Some(key) = get(DB_KEY)? {
        return Ok(Secret::from(key));
    }
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut_slice()).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    let key = Zeroizing::new(hex::encode(*bytes));
    set(DB_KEY, &key)?;
    // Defence-in-depth against a first-run race (belt-and-braces with the
    // single-instance guard): if another launch stored a key between our `get` and
    // `set`, return whatever the keychain now holds so we open the store with the
    // persisted key — never a local one that an overwrite could have orphaned.
    match get(DB_KEY)? {
        Some(stored) => Ok(Secret::from(stored)),
        None => Ok(Secret::from(key)),
    }
}

// --- Per-profile cache of a shareable vault's derived key (spec §2.2) ---
//
// A shareable vault's key is derived from the passphrase, but after the first
// successful unlock in a profile we cache it in THAT profile's own keychain so the
// passphrase is needed only the first time (or on a new device, or if the cache is
// lost). Keyed by the vault's stable id — not its path, which can move — so two
// profiles pointing at the same shared folder each cache independently, and no profile
// ever reads another's keychain.

fn vault_key_entry(vault_id: &str) -> String {
    format!("vault_key::{vault_id}")
}

/// This profile's cached derived key for a shareable vault, if it has unlocked it before.
pub fn get_cached_vault_key(vault_id: &str) -> Result<Option<Secret>> {
    Ok(get(&vault_key_entry(vault_id))?.map(Secret::from))
}

/// Cache the derived key (64-hex) for a shareable vault in this profile's keychain.
pub fn set_cached_vault_key(vault_id: &str, key_hex: &str) -> Result<()> {
    set(&vault_key_entry(vault_id), key_hex)
}

/// Forget this profile's cached key for a vault ("forget passphrase on this device").
pub fn clear_cached_vault_key(vault_id: &str) -> Result<()> {
    delete(&vault_key_entry(vault_id))
}
