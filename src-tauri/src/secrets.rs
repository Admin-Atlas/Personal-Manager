// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! All secrets live in the OS keychain — never on disk, never in the repo
//! (spec §6, §8.7). Entries are namespaced under a reverse-DNS service id that
//! matches the app's bundle identifier, so they can't collide with other apps'
//! keychain entries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex, MutexGuard};

use keyring::Entry;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::secret::Secret;

// Reverse-DNS to match the Tauri bundle identifier (`tauri.conf.json`). Keep the
// two in step: renaming this orphans every existing keychain entry (including the
// DB key, which makes the old encrypted store unreadable).
const SERVICE: &str = "org.itsatlas.pm";
/// The single keychain item that holds **every** PM secret as one JSON `{name: value}` map. macOS
/// shows one keychain-consent dialog per keychain *item*, so collapsing the ~dozen per-secret items
/// into this one means a single "Always Allow" covers all of PM's secrets at once — interim relief
/// for the unsigned-build prompt storm (one dialog per item, features loading in behind each) until
/// Developer-ID signing lands (`docs/MACOS-SIGNING.md`; the storm's real cause is an unsigned build's
/// unstable code identity, not this storage shape). **Honest trade-off, stated plainly:** that one
/// grant is coarser than the old per-item grants — any code running *as PM*, once the user allows it,
/// can read every secret, not just the one it asked for. Windows Credential Manager never prompts, so
/// this only changes anything on macOS. The `<name>` constants below are the *logical* keys inside
/// the map (and the *legacy* per-item entries, read once and folded into the bundle on first access).
const BUNDLE_KEY: &str = "pm_secrets";
const OPENROUTER_KEY: &str = "openrouter_api_key";
/// A separate key for non-interactive background work (sorting-review proposals,
/// the Learning-You profile distillation), so the user can see at a glance which
/// OpenRouter spend is interactive chat vs background processing (Step 4).
const OPENROUTER_BACKGROUND_KEY: &str = "openrouter_background_key";
/// Optional bearer token for a user-configured local OpenAI-compatible endpoint (#297). Most
/// loopback servers need none; a remote (LAN / Tailscale) one may. Kept in the keychain like every
/// other credential — never handed to the webview, never in settings.
const LOCAL_LLM_ENDPOINT_TOKEN: &str = "local_llm_endpoint_token";
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

// --- Raw single-item keychain primitives (one OS keychain item per call) --------------------
// These touch a keychain item DIRECTLY. Only the bundle machinery and the one-time legacy
// migration use them; every typed accessor goes through the cached `get`/`set`/`delete` below.

fn kc_get(name: &str) -> Result<Option<String>> {
    match entry(name)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::from(e)),
    }
}

fn kc_set(name: &str, value: &str) -> Result<()> {
    entry(name)?.set_password(value).map_err(Error::from)
}

/// Remove a keychain item; absent is success (so cleanup/disconnect is idempotent).
fn kc_delete(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::from(e)),
    }
}

// --- The bundle: all secrets in one keychain item, cached in memory for the process ----------
//
// Every secret lives in ONE keychain item ([`BUNDLE_KEY`]) as a JSON `{name: value}` map, read
// exactly once per process into [`CACHE`] and served from memory thereafter — which is what collapses
// the macOS "one keychain prompt per item" storm to a single grant. `get`/`set`/`delete` keep the
// exact signatures the typed accessors already call, so the whole change hides behind this seam.
//
// Legacy per-item entries (the old on-keychain shape) are folded in LAZILY and additively: a cache
// miss reads the matching legacy item once, writes it into the bundle, VERIFIES the bundle persisted,
// and only then deletes the legacy item — so a failed bundle write can never orphan a value (the DB
// key most of all). No value is ever re-keyed; a fresh install simply starts with an empty bundle.

/// Serialise the in-memory map to the bundle's on-keychain JSON. `BTreeMap` for deterministic output
/// (stable across writes, and unit-testable). Wrapped in `Zeroizing` so the transient
/// all-secrets-in-one-string plaintext is wiped once written.
fn encode_bundle(present: &HashMap<String, Secret>) -> Zeroizing<String> {
    let ordered: BTreeMap<&str, &str> = present
        .iter()
        .map(|(k, v)| (k.as_str(), v.expose()))
        .collect();
    Zeroizing::new(serde_json::to_string(&ordered).unwrap_or_else(|_| "{}".to_string()))
}

/// Parse the bundle JSON back into the in-memory map. A missing/empty/corrupt/wrong-shape bundle
/// reads as an empty map (best-effort — never an error), so a damaged item degrades to "nothing
/// cached yet" (and any still-present legacy items re-migrate) rather than bricking secret access.
fn decode_bundle(raw: &str) -> HashMap<String, Secret> {
    serde_json::from_str::<HashMap<String, String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, Secret::from(v)))
        .collect()
}

#[derive(Default)]
struct SecretCache {
    loaded: bool,
    /// Mirrors the persisted bundle: logical key -> value.
    present: HashMap<String, Secret>,
    /// Session-only negative cache: keys proven absent (bundle AND any legacy item), so a
    /// never-set / already-migrated key is not re-read from the keychain on every call.
    absent: HashSet<String>,
}

impl SecretCache {
    /// Read the bundle item once. Errors only if the keychain itself is unreachable.
    fn load(&mut self) -> Result<()> {
        if self.loaded {
            return Ok(());
        }
        if let Some(raw) = kc_get(BUNDLE_KEY)? {
            let raw = Zeroizing::new(raw);
            self.present = decode_bundle(&raw);
        }
        self.loaded = true;
        Ok(())
    }

    /// Write the whole map back to the single bundle item.
    fn persist(&self) -> Result<()> {
        let json = encode_bundle(&self.present);
        kc_set(BUNDLE_KEY, json.as_str())
    }

    /// Confirm the bundle on the keychain actually holds `name` after a write — the guard that lets
    /// migration delete a legacy item without risk of orphaning its value.
    fn bundle_has(&self, name: &str) -> bool {
        matches!(kc_get(BUNDLE_KEY), Ok(Some(raw)) if decode_bundle(&raw).contains_key(name))
    }

    fn get(&mut self, name: &str) -> Result<Option<String>> {
        self.load()?;
        if let Some(v) = self.present.get(name) {
            return Ok(Some(v.expose().to_string()));
        }
        if self.absent.contains(name) {
            return Ok(None);
        }
        // First miss: fold a legacy per-item entry into the bundle, if one exists.
        match kc_get(name)? {
            Some(value) => {
                self.present
                    .insert(name.to_string(), Secret::from(value.clone()));
                // Persist + verify BEFORE removing the legacy item, so a write failure keeps the
                // legacy copy as the fallback — never orphan a value (the DB key above all).
                if self.persist().is_ok() && self.bundle_has(name) {
                    let _ = kc_delete(name);
                }
                Ok(Some(value))
            }
            None => {
                self.absent.insert(name.to_string());
                Ok(None)
            }
        }
    }

    fn set(&mut self, name: &str, value: &str) -> Result<()> {
        self.load()?;
        let previous = self.present.insert(name.to_string(), Secret::from(value));
        self.absent.remove(name);
        if let Err(e) = self.persist() {
            // Roll back so the cache never claims a value the keychain doesn't hold.
            match previous {
                Some(old) => {
                    self.present.insert(name.to_string(), old);
                }
                None => {
                    self.present.remove(name);
                }
            }
            return Err(e);
        }
        // A value written before the bundle existed may also sit in a legacy item; clear it.
        let _ = kc_delete(name);
        Ok(())
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        self.load()?;
        let removed = self.present.remove(name);
        self.absent.insert(name.to_string());
        if removed.is_some() {
            if let Err(e) = self.persist() {
                if let Some(v) = removed {
                    self.present.insert(name.to_string(), v); // roll back
                }
                self.absent.remove(name);
                return Err(e);
            }
        }
        // Remove any legacy item too (idempotent — absent is success).
        let _ = kc_delete(name);
        Ok(())
    }

    /// Drop every cached secret. Used by the wipe after the keychain items are deleted, so a later
    /// read can't serve a just-erased secret; next access reloads from the now-empty bundle.
    fn clear(&mut self) {
        self.present.clear();
        self.absent.clear();
        self.loaded = false;
    }
}

/// Process-wide secret cache: one bundle read, served from memory (see [`BUNDLE_KEY`]). A poisoned
/// lock is recovered rather than propagated — a panic elsewhere must not make every secret
/// unreadable and brick the app.
static CACHE: LazyLock<Mutex<SecretCache>> = LazyLock::new(|| Mutex::new(SecretCache::default()));

fn cache() -> MutexGuard<'static, SecretCache> {
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn get(name: &str) -> Result<Option<String>> {
    cache().get(name)
}

fn set(name: &str, value: &str) -> Result<()> {
    cache().set(name, value)
}

/// Remove a secret; absent is success (so disconnect is idempotent).
fn delete(name: &str) -> Result<()> {
    cache().delete(name)
}

/// Drop the in-memory secret cache — the wipe calls this after erasing the keychain items so a
/// later read can't serve a just-deleted secret from memory (the app quits right after a wipe, but
/// the invariant shouldn't depend on that timing).
fn clear_cache() {
    cache().clear();
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
    // Treat a blank/whitespace stored key as absent: otherwise every guard that only checks for a
    // `Some` entry passes, `bearer_auth("")` sends an empty Authorization header, and OpenRouter
    // answers 401 "Missing Authentication header" instead of the friendly "no key set" message.
    Ok(get(OPENROUTER_KEY)?
        .filter(|v| !v.trim().is_empty())
        .map(Secret::from))
}

pub fn set_openrouter_key(value: &str) -> Result<()> {
    set(OPENROUTER_KEY, value)
}

pub fn get_openrouter_background_key() -> Result<Option<Secret>> {
    // A blank background key reads as absent (see get_openrouter_key), so get_background_or_primary_key
    // falls back to the primary key rather than short-circuiting on an empty string.
    Ok(get(OPENROUTER_BACKGROUND_KEY)?
        .filter(|v| !v.trim().is_empty())
        .map(Secret::from))
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

/// The bearer token for a user-configured local endpoint, if one was set. A blank stored value reads
/// as absent (matching the OpenRouter keys), so a saved-then-cleared token doesn't send `Bearer `.
pub fn get_local_llm_endpoint_token() -> Result<Option<Secret>> {
    Ok(get(LOCAL_LLM_ENDPOINT_TOKEN)?
        .filter(|v| !v.trim().is_empty())
        .map(Secret::from))
}

pub fn set_local_llm_endpoint_token(value: &str) -> Result<()> {
    set(LOCAL_LLM_ENDPOINT_TOKEN, value)
}

pub fn clear_local_llm_endpoint_token() -> Result<()> {
    delete(LOCAL_LLM_ENDPOINT_TOKEN)
}

/// Whether a local-endpoint bearer token is stored (presence only — the value never leaves Rust).
pub fn has_local_llm_endpoint_token() -> Result<bool> {
    Ok(get_local_llm_endpoint_token()?.is_some())
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

/// The per-account keychain token key (`<prefix><email>`) for a token-bearing
/// `connector_sources` (provider, service) pair — the single owner of the mapping, which the
/// connectors' `account_token_key` helpers and the wipe's key reconstruction all delegate to so
/// the strings can never drift apart. These strings are keychain identity: changing one orphans
/// every stored token (same class of one-way door as the bundle id). `None` for source kinds
/// that hold no OAuth token (Apple subscriptions, local folders).
pub fn token_key_for(provider: &str, service: &str, email: &str) -> Option<String> {
    let prefix = match (provider, service) {
        ("google", "drive") => GOOGLE_TOKEN_DRIVE_PREFIX,
        ("google", "calendar") => GOOGLE_TOKEN_CALENDAR_PREFIX,
        ("microsoft", "onedrive") => MICROSOFT_TOKEN_ONEDRIVE_PREFIX,
        ("microsoft", "calendar") => MICROSOFT_TOKEN_CALENDAR_PREFIX,
        _ => return None,
    };
    Some(format!("{prefix}{email}"))
}

pub fn get_microsoft_client_id() -> Result<Option<String>> {
    // A blank stored id reads as absent, exactly like the OpenRouter keys: the setter now refuses
    // one, but an entry written before that guard existed must still resolve to the honest "no
    // client set" rather than a `Some("")` that makes every OAuth attempt fail opaquely.
    Ok(get(MICROSOFT_CLIENT_ID)?.filter(|v| !v.trim().is_empty()))
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

/// The ids of every vault this profile has cached a key for, as a JSON array in the keychain.
///
/// It exists because the keychain cannot be enumerated: a wipe deletes exactly the keys it can
/// NAME, and it used to reconstruct only the *currently resolved* vault's id. Every other cached
/// key outlived "Remove PM data" — a shared vault the profile adopted and then detached from
/// keeps its cache deliberately (for a silent rejoin), so its raw SQLCipher master key survived
/// an explicit erase-everything, for a vault that may still exist in a shared folder. This
/// registry is the list of names the wipe needs.
const VAULT_KEY_IDS: &str = "vault_key_ids";

/// The registry's parsed contents; an unreadable or absent registry reads as empty, never an
/// error — it is a best-effort index over the real entries, not a source of truth.
fn cached_vault_ids() -> Vec<String> {
    get(VAULT_KEY_IDS)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// Add an id to a registry list, deduplicated and sorted. Pure — the string handling is the part
/// that can be wrong, and it is the part the keychain can't be tested against.
fn with_id(ids: &[String], vault_id: &str) -> Vec<String> {
    let mut out: Vec<String> = ids.to_vec();
    if !out.iter().any(|i| i == vault_id) {
        out.push(vault_id.to_string());
    }
    out.sort();
    out
}

/// Remove an id from a registry list. Pure.
fn without_id(ids: &[String], vault_id: &str) -> Vec<String> {
    ids.iter().filter(|i| *i != vault_id).cloned().collect()
}

/// Record (or forget) a vault id in the registry. Best-effort by design: it mirrors the
/// cache-first posture of every caller — a registry failure must never fail an unlock or an
/// adopt, because the cost of a miss is one extra passphrase prompt, while the cost of a hard
/// error here is a vault the user can't open at all.
fn remember_vault_id(vault_id: &str, present: bool) {
    let ids = cached_vault_ids();
    let next = if present {
        with_id(&ids, vault_id)
    } else {
        without_id(&ids, vault_id)
    };
    if next == ids {
        return;
    }
    if let Ok(json) = serde_json::to_string(&next) {
        let _ = set(VAULT_KEY_IDS, &json);
    }
}

/// This profile's cached derived key for a shareable vault, if it has unlocked it before.
pub fn get_cached_vault_key(vault_id: &str) -> Result<Option<Secret>> {
    let value = get(&vault_key_entry(vault_id))?;
    // Self-heal: a key cached before the registry existed is still real, and a successful read
    // is proof it is there. This is how pre-existing installs become wipeable without a migration.
    if value.is_some() {
        remember_vault_id(vault_id, true);
    }
    Ok(value.map(Secret::from))
}

/// Cache the derived key (64-hex) for a shareable vault in this profile's keychain.
pub fn set_cached_vault_key(vault_id: &str, key_hex: &str) -> Result<()> {
    set(&vault_key_entry(vault_id), key_hex)?;
    remember_vault_id(vault_id, true);
    Ok(())
}

/// Forget this profile's cached key for a vault ("forget passphrase on this device").
pub fn clear_cached_vault_key(vault_id: &str) -> Result<()> {
    delete(&vault_key_entry(vault_id))?;
    remember_vault_id(vault_id, false);
    Ok(())
}

/// Every vault id this profile has ever cached a key for, for the wipe's key reconstruction.
/// Best-effort: the caller unions this with the ids it can resolve from disk, so a lost registry
/// degrades to the old behaviour rather than skipping the current vault.
pub fn known_cached_vault_ids() -> Vec<String> {
    cached_vault_ids()
}

// --- Full keychain teardown ("Remove PM data" → OS keychain, spec §6/§8.7) ---
//
// Delete EVERY secret PM has ever written under the `org.itsatlas.pm` service. The OS
// keychain can't be enumerated (the crate looks up by exact key), so this is the single
// authoritative list of what PM stores: the fixed keys are named here, and the dynamic
// per-account / per-vault keys are reconstructed from ids the caller reads out of the DB
// before the store is torn down. Every delete is idempotent (absent = success), so a
// partial install (never connected a Drive account, etc.) wipes cleanly with no errors.

/// The fixed (non-parameterised) keychain keys PM writes. Keep in sync with the `const`s above —
/// a new fixed secret must be added here or it would survive a "remove everything" wipe.
const FIXED_KEYS: &[&str] = &[
    OPENROUTER_KEY,
    OPENROUTER_BACKGROUND_KEY,
    LOCAL_LLM_ENDPOINT_TOKEN,
    DB_KEY,
    BACKUP_PASSPHRASE,
    GOOGLE_CLIENT_ID,
    GOOGLE_CLIENT_SECRET,
    GOOGLE_TOKEN,          // legacy shared token (pre per-service)
    GOOGLE_TOKEN_CALENDAR, // legacy fixed calendar token (pre per-account)
    CALENDAR_ICS_FEEDS,
    MICROSOFT_CLIENT_ID,
    // The registry of cached-vault-key ids. It names other entries, so it goes LAST in spirit:
    // the caller reads it (via `known_cached_vault_ids`) to build the wipe list before any
    // deletion runs, and then it is deleted like any other secret PM wrote.
    VAULT_KEY_IDS,
];

/// Every keychain key a wipe must delete: the fixed keys, then the dynamic per-account /
/// per-vault keys reconstructed from ids the caller read out of the DB (in the same order the
/// wipe applied them). Split out of [`wipe_all_secrets`] so the key-list construction — the
/// correctness surface, where a forgotten fixed key or a wrong `::` prefix would leave a secret
/// behind — is unit-testable without touching the live OS keychain.
fn all_secret_keys(
    token_keys: &[String],
    google_client_emails: &[String],
    vault_ids: &[String],
) -> Vec<String> {
    let mut keys: Vec<String> = FIXED_KEYS.iter().map(|k| (*k).to_string()).collect();
    keys.extend(token_keys.iter().cloned());
    for email in google_client_emails {
        keys.push(format!("{GOOGLE_CLIENT_ID_PREFIX}{email}"));
        keys.push(format!("{GOOGLE_CLIENT_SECRET_PREFIX}{email}"));
    }
    for id in vault_ids {
        keys.push(vault_key_entry(id));
    }
    keys
}

/// Delete every PM secret from the OS keychain, returning how many entries were actually present and
/// removed. `token_keys` are the fully-formed per-account OAuth token keys the caller built from
/// connected accounts (Drive/Calendar/OneDrive/Outlook — via the public `*_PREFIX` constants);
/// `google_client_emails` are accounts that carry their OWN Google client (Advanced-Protection);
/// `vault_ids` are vaults whose derived key this profile has cached. Best-effort: a failure on one
/// entry never aborts the rest — this runs when the user is deliberately erasing PM, so "delete as
/// much as possible" is the right posture.
pub fn wipe_all_secrets(
    token_keys: &[String],
    google_client_emails: &[String],
    vault_ids: &[String],
) -> usize {
    let mut deleted = 0usize;

    {
        // Delete one entry, counting it only if it was actually there. An error (rare — the
        // credential store being unavailable) is swallowed so a single stubborn entry can't abort
        // the whole wipe.
        let mut wipe = |key: &str| {
            if let Ok(entry) = entry(key) {
                if entry.delete_credential().is_ok() {
                    deleted += 1;
                }
            }
        };

        // The single bundle item now holds every migrated secret, so this one delete erases them
        // all; the per-key deletes below then mop up any legacy items not yet folded into it (a
        // partial install, or a key never read this session). Absent = success throughout.
        wipe(BUNDLE_KEY);
        for key in all_secret_keys(token_keys, google_client_emails, vault_ids) {
            wipe(&key);
        }
    }

    // Drop the in-memory copies so nothing can be read back from the cache after the erase.
    clear_cache();

    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_secret_keys_covers_every_fixed_key() {
        let keys = all_secret_keys(&[], &[], &[]);
        for fixed in FIXED_KEYS {
            assert!(
                keys.iter().any(|k| k == fixed),
                "fixed key {fixed} must be in the wipe list"
            );
        }
        assert_eq!(
            keys.len(),
            FIXED_KEYS.len(),
            "with nothing connected the wipe list is exactly the fixed keys"
        );
    }

    #[test]
    fn all_secret_keys_reconstructs_the_dynamic_keys() {
        let token_keys = vec![
            format!("{GOOGLE_TOKEN_DRIVE_PREFIX}a@x.com"),
            format!("{GOOGLE_TOKEN_CALENDAR_PREFIX}b@y.com"),
        ];
        let emails = vec!["ap@x.com".to_string()];
        let vaults = vec!["vault-123".to_string()];
        let keys = all_secret_keys(&token_keys, &emails, &vaults);

        // Per-account OAuth token keys pass through verbatim.
        assert!(keys.contains(&format!("{GOOGLE_TOKEN_DRIVE_PREFIX}a@x.com")));
        assert!(keys.contains(&format!("{GOOGLE_TOKEN_CALENDAR_PREFIX}b@y.com")));
        // A BYO-client account's id + secret carry the exact `::`-suffixed prefixes.
        assert!(keys.contains(&"google_oauth_client_id::ap@x.com".to_string()));
        assert!(keys.contains(&"google_oauth_client_secret::ap@x.com".to_string()));
        // The cached vault key is `vault_key::{id}`.
        assert!(keys.contains(&"vault_key::vault-123".to_string()));

        assert_eq!(
            keys.len(),
            FIXED_KEYS.len() + token_keys.len() + 2 * emails.len() + vaults.len(),
            "one key per fixed + token + (id,secret) per email + vault"
        );
    }

    #[test]
    fn the_cached_key_registry_is_in_the_wipe_list() {
        // The registry names the per-vault keys, so a wipe that forgot it would leave a list of
        // exactly which vaults this profile once held keys for — after "remove everything".
        assert!(all_secret_keys(&[], &[], &[]).contains(&VAULT_KEY_IDS.to_string()));
    }

    #[test]
    fn registry_ids_dedupe_sort_and_remove() {
        // The keychain has no test shim, so the string handling — the only part that can be
        // wrong — is pure and tested here.
        let ids = with_id(&[], "v2");
        assert_eq!(ids, vec!["v2".to_string()]);
        let ids = with_id(&ids, "v1");
        assert_eq!(ids, vec!["v1".to_string(), "v2".to_string()], "sorted");
        assert_eq!(with_id(&ids, "v1"), ids, "re-adding an id is a no-op");
        assert_eq!(without_id(&ids, "v1"), vec!["v2".to_string()]);
        assert_eq!(
            without_id(&ids, "nope"),
            ids,
            "removing an absent id is a no-op"
        );
        assert!(without_id(&without_id(&ids, "v1"), "v2").is_empty());
    }

    #[test]
    fn all_secret_keys_has_no_duplicates() {
        let keys = all_secret_keys(
            &[format!("{GOOGLE_TOKEN_DRIVE_PREFIX}a@x.com")],
            &["ap@x.com".to_string()],
            &["v1".to_string()],
        );
        let mut deduped = keys.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            keys.len(),
            "the wipe list must not delete the same key twice"
        );
    }

    // --- The bundle codec: the one piece the keychain can't be tested against, so it's pure ---
    //
    // Deliberately generic, non-secret-shaped fixtures so the repo's gitleaks rules don't fire.

    #[test]
    fn bundle_round_trips_every_entry() {
        let mut m = HashMap::new();
        m.insert(
            "db_encryption_key".to_string(),
            Secret::from("aa00bb11cc22"),
        );
        m.insert(
            "google_oauth_token_calendar::a@x.com".to_string(),
            Secret::from("value-under-test-0002"),
        );
        let encoded = encode_bundle(&m);
        let decoded = decode_bundle(&encoded);
        assert_eq!(decoded.len(), 2);
        assert_eq!(
            decoded.get("db_encryption_key").unwrap().expose(),
            "aa00bb11cc22"
        );
        assert_eq!(
            decoded
                .get("google_oauth_token_calendar::a@x.com")
                .unwrap()
                .expose(),
            "value-under-test-0002"
        );
    }

    #[test]
    fn a_missing_or_corrupt_bundle_reads_as_empty_never_an_error() {
        // The safety valve: a damaged item must degrade to "nothing cached", not brick every read.
        assert!(decode_bundle("").is_empty());
        assert!(decode_bundle("not valid json {{{").is_empty());
        assert!(
            decode_bundle("[1,2,3]").is_empty(),
            "wrong shape reads as empty"
        );
        assert!(decode_bundle("null").is_empty());
    }

    #[test]
    fn an_empty_map_encodes_to_an_empty_object() {
        let empty: HashMap<String, Secret> = HashMap::new();
        assert_eq!(encode_bundle(&empty).as_str(), "{}");
        assert!(decode_bundle(&encode_bundle(&empty)).is_empty());
    }

    #[test]
    fn bundle_output_is_deterministic() {
        // BTreeMap ordering keeps writes stable (idempotent-looking, and reviewable).
        let mut m = HashMap::new();
        m.insert("z_key".to_string(), Secret::from("one"));
        m.insert("a_key".to_string(), Secret::from("two"));
        m.insert("m_key".to_string(), Secret::from("three"));
        assert_eq!(encode_bundle(&m).as_str(), encode_bundle(&m).as_str());
        assert!(
            encode_bundle(&m).find("a_key").unwrap() < encode_bundle(&m).find("z_key").unwrap()
        );
    }
}
