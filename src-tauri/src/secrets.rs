// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! All secrets live in the OS keychain — never on disk, never in the repo
//! (spec §6, §8.7). Entries are namespaced under a reverse-DNS service id that
//! matches the app's bundle identifier, so they can't collide with other apps'
//! keychain entries.

use keyring::Entry;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

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
/// Google OAuth (Step 6): the user's BYO "Desktop app" client (id + secret) and the
/// resulting token blob. No Google secret ships in the repo (rule #1) — the user
/// supplies their own client; everything lives only in the keychain.
const GOOGLE_CLIENT_ID: &str = "google_oauth_client_id";
const GOOGLE_CLIENT_SECRET: &str = "google_oauth_client_secret";
const GOOGLE_TOKEN: &str = "google_oauth_token";
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

pub fn get_openrouter_key() -> Result<Option<String>> {
    get(OPENROUTER_KEY)
}

pub fn set_openrouter_key(value: &str) -> Result<()> {
    set(OPENROUTER_KEY, value)
}

pub fn get_openrouter_background_key() -> Result<Option<String>> {
    get(OPENROUTER_BACKGROUND_KEY)
}

pub fn set_openrouter_background_key(value: &str) -> Result<()> {
    set(OPENROUTER_BACKGROUND_KEY, value)
}

/// The key for background work: the dedicated background key if the user set one,
/// otherwise the primary key as a fallback (so proposals/learning work before a
/// second key is configured). Errors only if neither is set.
pub fn get_background_or_primary_key() -> Result<Option<String>> {
    match get_openrouter_background_key()? {
        Some(key) => Ok(Some(key)),
        None => get_openrouter_key(),
    }
}

// --- Google OAuth (Step 6) ---

pub fn get_google_client_id() -> Result<Option<String>> {
    get(GOOGLE_CLIENT_ID)
}

pub fn get_google_client_secret() -> Result<Option<String>> {
    get(GOOGLE_CLIENT_SECRET)
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

pub fn get_google_token() -> Result<Option<String>> {
    get(GOOGLE_TOKEN)
}

pub fn set_google_token(value: &str) -> Result<()> {
    set(GOOGLE_TOKEN, value)
}

/// Forget the OAuth token (disconnect). The client credentials are left in place so
/// the user can reconnect without re-entering them.
pub fn clear_google_token() -> Result<()> {
    delete(GOOGLE_TOKEN)
}

pub fn get_ics_feeds() -> Result<Option<String>> {
    get(CALENDAR_ICS_FEEDS)
}

pub fn set_ics_feeds(value: &str) -> Result<()> {
    set(CALENDAR_ICS_FEEDS, value)
}

/// Returns the database encryption key, generating and persisting a fresh
/// 256-bit random key on first run. The key never touches disk in plaintext.
/// Wrapped in `Zeroizing` so the in-memory copy is wiped once the caller drops it.
pub fn get_or_create_db_key() -> Result<Zeroizing<String>> {
    if let Some(key) = get(DB_KEY)? {
        return Ok(Zeroizing::new(key));
    }
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut_slice()).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    let key = Zeroizing::new(hex::encode(&*bytes));
    set(DB_KEY, &key)?;
    // Defence-in-depth against a first-run race (belt-and-braces with the
    // single-instance guard): if another launch stored a key between our `get` and
    // `set`, return whatever the keychain now holds so we open the store with the
    // persisted key — never a local one that an overwrite could have orphaned.
    match get(DB_KEY)? {
        Some(stored) => Ok(Zeroizing::new(stored)),
        None => Ok(key),
    }
}
