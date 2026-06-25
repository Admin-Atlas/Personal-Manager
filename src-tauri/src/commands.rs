// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command surface exposed to the frontend. DB access locks the shared
//! connection only for quick synchronous work — never across an `.await` — so
//! the streaming chat command stays responsive.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::calendar::{self, CalendarEvent, CalendarInfo, IcsFeedInfo};
use crate::error::{Error, Result};
use crate::google;
use crate::ingest::{self, Document, IngestEvent};
use crate::projects::{self, ProjectOverview, ProjectProposalEvent};
use crate::retrieval::{self, Citation, RetrievedChunk};
use crate::review::{self, ReviewDecision, ReviewEvent};
use crate::sidecar::SidecarStatus;
use crate::{
    applock, briefing, clock, cost, db, learning, lock_session, openrouter, paths, recommend,
    secrets, vault, AppState, VaultRuntime,
};

/// Fallback model when the user hasn't chosen one. Swappable in Settings and
/// stored as a plain string (spec §6 — never locked into a model).
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4.6";

/// Settings keys for the two model roles. Each holds a JSON array of model ids
/// (ordered, first = primary); the `*_AUTO_SWITCH` keys hold "true"/"false".
const CHAT_MODELS_KEY: &str = "chat_models";
const BACKGROUND_MODELS_KEY: &str = "background_models";
const CHAT_AUTO_SWITCH_KEY: &str = "chat_auto_switch";
const BACKGROUND_AUTO_SWITCH_KEY: &str = "background_auto_switch";

/// The user's IANA time-zone name (e.g. "America/New_York"), supplied by the
/// frontend via `Intl.DateTimeFormat().resolvedOptions().timeZone`. Empty/unset →
/// the backend reasons in UTC (see `resolve_zone`).
const TIME_ZONE_KEY: &str = "time_zone";

/// Whether the optional biometric app-lock is on ("true"/"false", default off). A soft
/// UI gate only — it never gates the DB key (see `applock`). Lives in `settings`
/// (security preference → backend), not localStorage.
const APP_LOCK_ENABLED_KEY: &str = "app_lock_enabled";

/// Optional, user-editable denylist (provider or model slugs) the recommender excludes
/// as defense-in-depth — JSON array of strings. The real privacy boundary is the
/// request-level ZDR enforcement in `openrouter::chat_body`; this is secondary (spec §6).
const RECOMMEND_DENYLIST_KEY: &str = "recommend_denylist";

/// Caps for chat: the most we'll store for a single message, and how many prior
/// turns we replay into a request. A long conversation or one giant pasted message
/// would otherwise inflate every call (the spend lands on the user's own key).
/// Both are generous — far beyond any normal chat turn or history depth.
const MAX_MESSAGE_CHARS: usize = 100_000;
const MAX_HISTORY_MESSAGES: usize = 40;

#[derive(Serialize)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    /// The project this chat is scoped to (Step 5), or `None` for a global chat.
    /// A scoped chat's retrieval is confined to this project's documents.
    pub project: Option<String>,
}

#[derive(Serialize)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub created_at: String,
    /// Source documents this answer drew from (assistant turns only).
    pub citations: Option<Vec<Citation>>,
}

#[derive(Serialize)]
pub struct Settings {
    /// Ordered preferred models for chat (user-facing) and background work
    /// (sorting proposals + Learning You). First = primary; the rest are
    /// auto-switch fallbacks when the matching toggle is on.
    pub chat_models: Vec<String>,
    pub background_models: Vec<String>,
    pub chat_auto_switch: bool,
    pub background_auto_switch: bool,
    /// When on, the UI shows an explanation panel for whatever section the user
    /// hovers — a learn-the-app affordance (Step 4b). Defaults off.
    pub help_mode: bool,
    /// The user's IANA time zone (e.g. "Europe/London"), or "" when not yet set —
    /// the focus-view day boundaries and the briefing/agenda "now" reason in it.
    pub time_zone: String,
    /// Whether query-time reranking is on (a cross-encoder re-scores search hits for sharper
    /// relevance). Default on; stateless, so toggling it never triggers a Rebuild.
    pub reranking: bool,
}

/// Streamed back to the UI over a Tauri channel as the assistant replies.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Token {
        text: String,
    },
    Done {
        message_id: i64,
        content: String,
        citations: Vec<Citation>,
    },
    Error {
        message: String,
    },
}

// --- secrets ---

#[tauri::command]
pub fn has_openrouter_key() -> Result<bool> {
    Ok(secrets::get_openrouter_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    secrets::set_openrouter_key(key)
}

#[tauri::command]
pub fn has_openrouter_background_key() -> Result<bool> {
    Ok(secrets::get_openrouter_background_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_background_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    secrets::set_openrouter_background_key(key)
}

// --- settings ---

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    let conn = state.conn()?;
    Ok(Settings {
        chat_models: models_for(&conn, CHAT_MODELS_KEY)?,
        background_models: models_for(&conn, BACKGROUND_MODELS_KEY)?,
        chat_auto_switch: db::get_setting(&conn, CHAT_AUTO_SWITCH_KEY)?.as_deref() == Some("true"),
        background_auto_switch: db::get_setting(&conn, BACKGROUND_AUTO_SWITCH_KEY)?.as_deref()
            == Some("true"),
        help_mode: db::get_setting(&conn, "help_mode")?.as_deref() == Some("true"),
        time_zone: db::get_setting(&conn, TIME_ZONE_KEY)?.unwrap_or_default(),
        reranking: db::reranking_enabled(&conn)?,
    })
}

#[tauri::command]
pub fn set_chat_models(state: State<'_, AppState>, models: Vec<String>) -> Result<()> {
    let conn = state.conn()?;
    save_models(&conn, CHAT_MODELS_KEY, models)
}

#[tauri::command]
pub fn set_background_models(state: State<'_, AppState>, models: Vec<String>) -> Result<()> {
    let conn = state.conn()?;
    save_models(&conn, BACKGROUND_MODELS_KEY, models)
}

#[tauri::command]
pub fn set_chat_auto_switch(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_setting(
        &conn,
        CHAT_AUTO_SWITCH_KEY,
        if enabled { "true" } else { "false" },
    )
}

#[tauri::command]
pub fn set_background_auto_switch(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_setting(
        &conn,
        BACKGROUND_AUTO_SWITCH_KEY,
        if enabled { "true" } else { "false" },
    )
}

/// Toggle the UI help/explain mode (Step 4b). Stored in `settings` so it persists.
#[tauri::command]
pub fn set_help_mode(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_setting(&conn, "help_mode", if enabled { "true" } else { "false" })
}

/// Turn query-time reranking on or off (a cross-encoder re-scores search hits). Stateless — never
/// triggers a Rebuild — so this just flips the setting; the effect lands on the next query.
#[tauri::command]
pub fn set_reranking(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_reranking(&conn, enabled)
}

/// One language/embedder choice offered at vault creation.
#[derive(Serialize)]
pub struct LanguageOption {
    pub id: String,
    pub label: String,
    pub multilingual: bool,
}

/// The vault's search-language choices: the selectable embedders, the current selection, and
/// whether the vault already has documents. `has_documents` is true when switching the language
/// means a re-index (the frontend confirms + launches the guided Re-index) rather than a free
/// choice on an empty vault.
#[derive(Serialize)]
pub struct LanguageOptions {
    pub options: Vec<LanguageOption>,
    pub selected: String,
    pub has_documents: bool,
}

/// The search-language options + current selection — for the onboarding picker and the Settings
/// language switcher.
#[tauri::command]
pub fn language_options(state: State<'_, AppState>) -> Result<LanguageOptions> {
    let conn = state.conn()?;
    let selected = db::selected_embedder(&conn)?.id.to_string();
    let has_documents: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))?;
    let options = crate::registry::selectable_embedders()
        .into_iter()
        .map(|m| LanguageOption {
            id: m.id.to_string(),
            label: m.label.to_string(),
            multilingual: m.multilingual,
        })
        .collect();
    Ok(LanguageOptions {
        options,
        selected,
        has_documents,
    })
}

/// Choose the vault's embedder (its "search language"). Validates the id against the selectable
/// embedders so an arbitrary/incompatible model can't be stored, then:
///
/// - **Empty vault** (onboarding, or nothing ingested yet): record the selection and resize the
///   empty `chunk_vec` to the chosen embedder's width straight away, so the very first ingest
///   already matches (a 1024-d multilingual choice would otherwise trip `ingest::run`'s width
///   guard against the migration's 384-d table).
/// - **Populated vault**: record the selection only. The retrieval stamp now mismatches (embedder
///   id + dimension changed), so the vault surfaces its one-time Rebuild prompt and the frontend
///   launches the guided Re-index — which downloads the model if needed, resizes the vector
///   column, and re-embeds from the Markdown source of truth. We never resize a populated table
///   here: that would drop live vectors with no rebuild.
#[tauri::command]
pub fn set_vault_embedder(state: State<'_, AppState>, embedder_id: String) -> Result<()> {
    if !crate::registry::selectable_embedders()
        .iter()
        .any(|m| m.id == embedder_id)
    {
        return Err(Error::Other(format!(
            "'{embedder_id}' is not a selectable embedder"
        )));
    }
    let embedder = crate::registry::embedder_or_default(&embedder_id);
    let conn = state.conn()?;
    let has_docs: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))?;
    db::set_selected_embedder(&conn, &embedder_id)?;
    if !has_docs {
        // Empty vault: safe to resize the (empty) vector column to the chosen width now.
        db::ensure_vec_dim(&conn, embedder.dimension)?;
    }
    Ok(())
}

/// The stored IANA time zone (empty string = none set; the backend then uses UTC).
#[tauri::command]
pub fn get_time_zone(state: State<'_, AppState>) -> Result<String> {
    let conn = state.conn()?;
    Ok(db::get_setting(&conn, TIME_ZONE_KEY)?.unwrap_or_default())
}

/// Persist the IANA zone the frontend resolved via `Intl`. Validated against the
/// chrono-tz database so a garbage string can't be stored; an empty value clears it
/// (the backend falls back to UTC). This is correctness state, not appearance, so it
/// lives in the backend `settings` table where the SQL/date logic reads it.
#[tauri::command]
pub fn set_time_zone(state: State<'_, AppState>, zone: String) -> Result<()> {
    use std::str::FromStr;
    let zone = zone.trim();
    if !zone.is_empty() && chrono_tz::Tz::from_str(zone).is_err() {
        return Err(Error::Other(format!("unrecognised time zone: {zone}")));
    }
    let conn = state.conn()?;
    db::set_setting(&conn, TIME_ZONE_KEY, zone)
}

/// Webview-owned UI preferences that may be persisted via `set_pref`. Holding the
/// list here (rather than letting the webview write any key) keeps presentation
/// state out of the schema-critical rows: the webview must never be able to
/// rewrite e.g. `embedding_dim` and silently corrupt the index.
const WRITABLE_PREFS: &[&str] = &["appearance", "pinboard"];

/// Read a UI preference blob the webview previously stored (theme axes, pinboard
/// layout). These live in the encrypted `settings` table — not the webview's
/// `localStorage` — so they travel with the data folder when it's backed up or
/// moved to another machine. Returns `None` when nothing is stored yet.
#[tauri::command]
pub fn get_pref(state: State<'_, AppState>, key: String) -> Result<Option<String>> {
    let conn = state.conn()?;
    db::get_setting(&conn, &key)
}

/// Persist a UI preference blob (see [`get_pref`]). Restricted to [`WRITABLE_PREFS`]
/// so the webview can only touch presentation state, never schema-critical keys.
#[tauri::command]
pub fn set_pref(state: State<'_, AppState>, key: String, value: String) -> Result<()> {
    if !WRITABLE_PREFS.contains(&key.as_str()) {
        return Err(Error::Other(format!("preference '{key}' is not writable")));
    }
    let conn = state.conn()?;
    db::set_setting(&conn, &key, &value)
}

/// The optional biometric app-lock's state for Settings + the launch gate. `available`
/// reflects whether the OS can actually verify (Windows Hello enrolled / Touch ID) — the
/// toggle is disabled when it's false so the lock can't be switched on where it could
/// never open. `locked` is the launch gate: enabled and not yet verified this session.
#[derive(Serialize)]
pub struct AppLockStatus {
    pub enabled: bool,
    pub available: bool,
    pub locked: bool,
}

/// Read the app-lock preference, whether the OS can verify, and whether the UI should be
/// gated right now. The gate is computed backend-side (`applock::should_lock`) from the
/// stored preference + this process's verified flag so it can't be flipped from the webview.
#[tauri::command]
pub fn app_lock_status(state: State<'_, AppState>) -> Result<AppLockStatus> {
    let enabled = {
        let conn = state.conn()?;
        db::get_setting(&conn, APP_LOCK_ENABLED_KEY)?.as_deref() == Some("true")
    };
    let verified = state
        .app_unlocked
        .load(std::sync::atomic::Ordering::Relaxed);
    Ok(AppLockStatus {
        enabled,
        available: applock::available(),
        locked: applock::should_lock(enabled, verified),
    })
}

/// Turn the soft app-lock on or off. Enabling is refused when the OS can't verify, so a
/// user can't strand themselves behind a gate that will never open (e.g. no Hello
/// enrolled, or macOS where it isn't implemented yet).
#[tauri::command]
pub fn set_app_lock(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    if enabled && !applock::available() {
        return Err(Error::Other(
            "this device can't perform a biometric/Windows Hello check, so the app-lock can't be enabled".into(),
        ));
    }
    let conn = state.conn()?;
    db::set_setting(
        &conn,
        APP_LOCK_ENABLED_KEY,
        if enabled { "true" } else { "false" },
    )
}

/// Run the OS verification (Windows Hello / Touch ID) to lift the launch lock. Returns
/// `true` on success, `false` when the user cancels/fails. The HWND is read on the UI
/// thread (it's `!Send`) and the blocking WinRT wait runs on a worker thread so the UI
/// stays responsive while the system prompt is up.
#[tauri::command]
pub async fn unlock_app(state: State<'_, AppState>, window: tauri::WebviewWindow) -> Result<bool> {
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
            // Unused off Windows (the stubs ignore it), but keep the binding so the
            // worker closure is identical across platforms.
            let _ = &window;
            0isize
        }
    };
    let verified =
        tauri::async_runtime::spawn_blocking(move || applock::verify(raw_handle, "Unlock PM"))
            .await
            .map_err(|e| Error::Other(format!("verification task failed: {e}")))??;
    if verified {
        state
            .app_unlocked
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(verified)
}

// --- vault (shareable / portable) ---

/// Build the session's Markdown runtime for a freshly opened vault: the resolved
/// Markdown dir plus the policy-aware cipher (derived from the master that the DB key
/// hex carries). Shared by unlock and open-existing so both install an identical runtime.
fn vault_runtime_for(
    resolved: &vault::ResolvedVault,
    meta: &vault::VaultMeta,
    key_hex: &str,
) -> Result<VaultRuntime> {
    let master = vault::master_from_db_key_hex(key_hex)?;
    Ok(VaultRuntime {
        markdown_dir: resolved.markdown_dir.clone(),
        cipher: vault::MarkdownCipher::from_meta(meta, &master),
    })
}

/// What the frontend needs to decide whether to show the unlock screen and how to
/// label the vault. Non-secret: mode, whether the store is currently locked, whether
/// Markdown is encrypted at rest, the vault location, and the stable vault id.
#[derive(Serialize)]
pub struct VaultStatus {
    pub mode: vault::KeyMode,
    pub needs_unlock: bool,
    pub markdown_encrypted: bool,
    pub location: String,
    pub vault_id: Option<String>,
    /// Whether the stored index was produced by a different retrieval config than this build
    /// (a model, chunk-rule, or splitter change) — i.e. a one-time Rebuild is recommended. The
    /// Documents view surfaces this as a dismissible banner. False when the vault is locked or
    /// has no documents yet.
    pub retrieval_rebuild_needed: bool,
}

/// Report the current vault's mode and whether it needs unlocking (a passphrase vault
/// whose key isn't cached in this profile yet).
#[tauri::command]
pub fn vault_status(app: AppHandle, state: State<'_, AppState>) -> Result<VaultStatus> {
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?;
    let (mode, markdown_encrypted, vault_id) = match &meta {
        Some(m) => (
            m.key_mode,
            m.markdown.encryption != vault::MarkdownEncryption::None,
            Some(m.vault_id.clone()),
        ),
        None => (vault::KeyMode::Device, false, None),
    };
    // A populated vault whose stored index was produced by a different retrieval config than
    // this build (a model/chunk/splitter change, or a pre-stamp vault) gets a one-time Rebuild
    // prompt. Only meaningful when the store is open and has documents.
    let retrieval_rebuild_needed = if state.is_unlocked() {
        let conn = state.conn()?;
        let has_docs: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))?;
        // Compare against what this build would produce for *this vault's* embedder, so a
        // multilingual vault isn't wrongly flagged stale against the English default.
        let current = crate::retrieval_config::RetrievalConfig::current_for(
            &crate::db::selected_embedder(&conn)?,
        );
        has_docs && crate::db::get_retrieval_stamp(&conn)?.as_ref() != Some(&current)
    } else {
        false
    };
    Ok(VaultStatus {
        mode,
        needs_unlock: !state.is_unlocked(),
        markdown_encrypted,
        location: resolved.vault_root.to_string_lossy().into_owned(),
        vault_id,
        retrieval_rebuild_needed,
    })
}

/// Convert this profile's device vault into a shareable, passphrase-protected one. Runs
/// through the one migration routine (derive the key, re-key the store, encrypt the
/// Markdown), so it is crash-recoverable. The device-only default is untouched for users
/// who never opt in; changing an existing passphrase is `change_vault_passphrase`.
#[tauri::command]
pub async fn create_shareable_vault(app: AppHandle, passphrase: String) -> Result<()> {
    if passphrase.trim().is_empty() {
        return Err(Error::Other("a passphrase is required".into()));
    }
    {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        if meta.key_mode == vault::KeyMode::Passphrase {
            return Err(Error::Other("this vault is already shareable".into()));
        }
    }
    let plan = vault::migrate::MigrationPlan {
        target_key_mode: vault::KeyMode::Passphrase,
        new_passphrase: Some(passphrase),
        target_markdown: vault::MarkdownEncryption::XChaCha20Poly1305,
        target_location: None,
    };
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || vault::migrate::migrate_vault(&app2, plan))
        .await
        .map_err(|e| Error::Other(format!("migration task panicked: {e}")))??;
    // Re-engage the writer lock for the vault's new state: acquire if it became shareable,
    // release if it went device-only, or re-acquire at the new location after a move.
    lock_session::engage(&app)?;
    Ok(())
}

/// Change a shareable vault's passphrase: re-derive the key (new salt + verifier),
/// re-key the store, and re-encrypt the Markdown under the new subkey — one atomic,
/// crash-recoverable migration. Only valid for an already-shareable vault.
#[tauri::command]
pub async fn change_vault_passphrase(app: AppHandle, new_passphrase: String) -> Result<()> {
    if new_passphrase.trim().is_empty() {
        return Err(Error::Other("a passphrase is required".into()));
    }
    {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        if meta.key_mode != vault::KeyMode::Passphrase {
            return Err(Error::Other(
                "this vault has no passphrase; make it shareable first".into(),
            ));
        }
    }
    let plan = vault::migrate::MigrationPlan {
        target_key_mode: vault::KeyMode::Passphrase,
        new_passphrase: Some(new_passphrase),
        target_markdown: vault::MarkdownEncryption::XChaCha20Poly1305,
        target_location: None,
    };
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || vault::migrate::migrate_vault(&app2, plan))
        .await
        .map_err(|e| Error::Other(format!("migration task panicked: {e}")))??;
    // Re-engage the writer lock for the vault's new state: acquire if it became shareable,
    // release if it went device-only, or re-acquire at the new location after a move.
    lock_session::engage(&app)?;
    Ok(())
}

/// Make a shareable vault private again: re-key it to a random device key (held only in
/// this profile's keychain) and decrypt the Markdown back to plaintext. Reverses
/// `create_shareable_vault`; a no-op-style error if the vault is already device-only.
#[tauri::command]
pub async fn make_vault_private(app: AppHandle) -> Result<()> {
    {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        if meta.key_mode == vault::KeyMode::Device {
            return Err(Error::Other(
                "this vault is already private to this device".into(),
            ));
        }
    }
    let plan = vault::migrate::MigrationPlan {
        target_key_mode: vault::KeyMode::Device,
        new_passphrase: None,
        target_markdown: vault::MarkdownEncryption::None,
        target_location: None,
    };
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || vault::migrate::migrate_vault(&app2, plan))
        .await
        .map_err(|e| Error::Other(format!("migration task panicked: {e}")))??;
    // Re-engage the writer lock for the vault's new state: acquire if it became shareable,
    // release if it went device-only, or re-acquire at the new location after a move.
    lock_session::engage(&app)?;
    Ok(())
}

/// Move the vault to a new folder (e.g. a shared location), keeping its key and Markdown
/// policy unchanged. Copy-verify-delete with the pointer flipped last, so an interrupted
/// move leaves the vault safely at its current location.
#[tauri::command]
pub async fn move_vault(app: AppHandle, folder: String) -> Result<()> {
    let target = std::path::PathBuf::from(folder);
    let plan = {
        let meta = vault::load_meta(&vault::resolve(&app)?.vault_root)?
            .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
        vault::migrate::MigrationPlan {
            target_key_mode: meta.key_mode,
            new_passphrase: None,
            target_markdown: meta.markdown.encryption,
            target_location: Some(target),
        }
    };
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || vault::migrate::migrate_vault(&app2, plan))
        .await
        .map_err(|e| Error::Other(format!("migration task panicked: {e}")))??;
    // Re-engage the writer lock for the vault's new state: acquire if it became shareable,
    // release if it went device-only, or re-acquire at the new location after a move.
    lock_session::engage(&app)?;
    Ok(())
}

/// Unlock the current (passphrase) vault: derive + verify, open the store, and cache
/// the derived key in this profile so the next launch is silent.
#[tauri::command]
pub fn unlock_vault(app: AppHandle, state: State<'_, AppState>, passphrase: String) -> Result<()> {
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata to unlock".into()))?;
    let (conn, key) = vault::open_with_passphrase(&resolved, &meta, &passphrase)?;
    secrets::set_cached_vault_key(&meta.vault_id, key.expose())?;
    let runtime = vault_runtime_for(&resolved, &meta, key.expose())?;
    state.open_session(conn, runtime)?;
    // Now that the store is open, engage the cooperative writer lock for this vault.
    lock_session::engage(&app)?;
    Ok(())
}

/// Point this profile at an existing vault folder (e.g. a shared one) and open it.
/// Device-only vaults are bound to their originating profile's keychain, so they can't
/// be opened here — they must be converted to shareable first.
#[tauri::command]
pub fn open_existing_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
    passphrase: Option<String>,
) -> Result<()> {
    let root = std::path::PathBuf::from(folder);
    let meta = vault::load_meta(&root)?
        .ok_or_else(|| Error::Other("no PM vault found in that folder".into()))?;
    let resolved = vault::ResolvedVault {
        db_path: root.join("pm.sqlite"),
        markdown_dir: root.join("vault"),
        vault_root: root.clone(),
    };
    let (conn, runtime) = match meta.key_mode {
        vault::KeyMode::Device => {
            return Err(Error::Other(
                "that vault is device-only and tied to the profile that created it; \
                 convert it to shareable first"
                    .into(),
            ))
        }
        vault::KeyMode::Passphrase => {
            let passphrase = passphrase.ok_or_else(|| {
                Error::Other("a passphrase is required to open this vault".into())
            })?;
            let (conn, key) = vault::open_with_passphrase(&resolved, &meta, &passphrase)?;
            secrets::set_cached_vault_key(&meta.vault_id, key.expose())?;
            let runtime = vault_runtime_for(&resolved, &meta, key.expose())?;
            (conn, runtime)
        }
    };
    // Point this profile here so the next launch opens it directly.
    let data_dir = paths::data_dir(&app)?;
    vault::pointer::store(&data_dir, &vault::pointer::VaultPointer::new(root))?;
    state.open_session(conn, runtime)?;
    lock_session::engage(&app)?;
    Ok(())
}

/// Forget this profile's cached key for the current vault, so the passphrase is needed
/// again next launch. Does not lock the current session (the store stays open until exit).
#[tauri::command]
pub fn forget_vault_passphrase(app: AppHandle) -> Result<()> {
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    secrets::clear_cached_vault_key(&meta.vault_id)?;
    Ok(())
}

/// Grant another account on this machine access to the shared vault folder — the
/// Settings "link a second account" action. Takes an account name (e.g. `PC\alice`) or
/// a SID. Only a shareable vault can be linked; ACLs are defence in depth (encryption
/// is the real protection), so on platforms without support this surfaces as a clear
/// error the UI can show as a warning.
#[tauri::command]
pub fn link_vault_account(app: AppHandle, account: String) -> Result<()> {
    let resolved = vault::resolve(&app)?;
    let meta = vault::load_meta(&resolved.vault_root)?
        .ok_or_else(|| Error::Other("this vault has no metadata".into()))?;
    if meta.key_mode != vault::KeyMode::Passphrase {
        return Err(Error::Other(
            "only a shareable vault can be linked to another account; make it shareable first"
                .into(),
        ));
    }
    vault::acl::grant_access(&resolved.vault_root, &account)
}

/// The cooperative writer-lock status for a shared vault: whether this instance is the
/// active writer, whether another live profile holds it, and whether that holder looks
/// crashed (so the UI can offer a warned force-take). A device vault always reports active.
#[tauri::command]
pub fn vault_lock_status(app: AppHandle) -> Result<lock_session::VaultLockStatus> {
    Ok(lock_session::status(&app))
}

/// "Continue here" on the curtain: ask the other live profile to hand the vault over (the
/// watcher takes it once they release), or take it immediately if they've already gone.
#[tauri::command]
pub fn continue_here(app: AppHandle) -> Result<()> {
    lock_session::continue_here(&app)
}

/// Force-take a vault whose holder looks crashed (a stale heartbeat). The UI shows the
/// "the other instance may not have saved its last change" warning before calling this.
#[tauri::command]
pub fn force_take_vault(app: AppHandle) -> Result<()> {
    lock_session::force_take(&app)
}

/// The OpenRouter model catalogue (public endpoint, no key needed) so the user can
/// browse, search, and pick a model with pricing in Settings (spec §6 — any model,
/// swappable).
#[tauri::command]
pub async fn list_models() -> Result<Vec<openrouter::ModelInfo>> {
    openrouter::list_models().await
}

// --- conversations & messages ---

#[tauri::command]
pub fn list_conversations(state: State<'_, AppState>) -> Result<Vec<Conversation>> {
    let conn = state.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, project FROM conversations \
         ORDER BY updated_at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([], row_to_conversation)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Start a conversation. `project` scopes it to one project (Step 5) — the
/// per-project view passes it so the chat's retrieval narrows to that project;
/// `None` is a normal global chat.
#[tauri::command]
pub fn create_conversation(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<Conversation> {
    let project = project
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    let conn = state.conn()?;
    conn.execute(
        "INSERT INTO conversations(project) VALUES (?1)",
        params![project],
    )?;
    let id = conn.last_insert_rowid();
    Ok(conn.query_row(
        "SELECT id, title, created_at, updated_at, project FROM conversations WHERE id = ?1",
        params![id],
        row_to_conversation,
    )?)
}

#[tauri::command]
pub fn get_messages(state: State<'_, AppState>, conversation_id: i64) -> Result<Vec<Message>> {
    let conn = state.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, conversation_id, role, content, model, created_at, citations \
         FROM messages WHERE conversation_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![conversation_id], row_to_message)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Persist the user's turn, stream the assistant's reply from OpenRouter (tokens
/// pushed over `on_event`), then persist the assistant's turn.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: i64,
    content: String,
    on_event: Channel<ChatEvent>,
) -> Result<()> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(Error::Other("message is empty".into()));
    }
    // Cap the stored/sent message so one multi-MB paste can't bloat the store and
    // every following request.
    let content: String = content.chars().take(MAX_MESSAGE_CHARS).collect();

    let api_key = secrets::get_openrouter_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    // Save the user turn and gather history + models + the learned profile + the
    // conversation's project scope. Scope the lock so the guard is dropped before
    // the network await below.
    let (history, models, profile, scope, agenda) = {
        let conn = state.conn()?;

        let prior: i64 = conn.query_row(
            "SELECT count(*) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;

        // A project-scoped chat (Step 5) confines retrieval to that project's docs.
        let scope: Option<String> = conn.query_row(
            "SELECT project FROM conversations WHERE id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;

        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1, 'user', ?2)",
            params![conversation_id, content],
        )?;

        // Name a fresh conversation after its first message.
        if prior == 0 {
            let title: String = content.chars().take(48).collect();
            conn.execute(
                "UPDATE conversations SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![title, conversation_id],
            )?;
        }

        let models = effective_models(&conn, CHAT_MODELS_KEY, CHAT_AUTO_SWITCH_KEY)?;
        // Replay only the most recent turns (newest N by id, then back into
        // chronological order) so a long conversation can't grow every request.
        let mut stmt = conn.prepare(
            "SELECT role, content FROM \
                 (SELECT id, role, content FROM messages WHERE conversation_id = ?1 \
                  ORDER BY id DESC LIMIT ?2) \
             ORDER BY id",
        )?;
        let history = stmt
            .query_map(
                params![conversation_id, MAX_HISTORY_MESSAGES as i64],
                |row| {
                    Ok(openrouter::ChatMessage {
                        role: row.get(0)?,
                        content: row.get(1)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let profile = learning::profile_preamble(&conn)?;
        // Give a global (unscoped) chat the user's upcoming agenda so it can answer
        // "what's on at 3pm?" (Step 6). A project-scoped chat stays on its documents.
        let agenda = if scope.is_none() {
            calendar::agenda_preamble(&conn, 7, resolve_zone(&conn))?
        } else {
            None
        };
        (history, models, profile, scope, agenda)
    };

    // Ground the answer in the user's files (best-effort): retrieve the most
    // relevant chunks and prepend them as a system message the model must cite.
    // If retrieval yields nothing (no docs / engine not ready), chat proceeds
    // exactly as before. A scoped chat draws only from its project.
    let retrieved = retrieve_grounding(&app, content.clone(), scope).await;
    let citations = retrieval::citations_from(&retrieved);

    let mut messages = Vec::with_capacity(history.len() + 2);
    // The learned profile goes first so the model carries the user's habits into
    // every answer (Step 4b, spec §4.5); then the grounding sources, then history.
    if let Some(profile) = &profile {
        messages.push(openrouter::ChatMessage {
            role: "system".into(),
            content: profile.clone(),
        });
    }
    if let Some(agenda) = &agenda {
        messages.push(openrouter::ChatMessage {
            role: "system".into(),
            content: agenda.clone(),
        });
    }
    if !retrieved.is_empty() {
        messages.push(openrouter::ChatMessage {
            role: "system".into(),
            content: retrieval::grounding_prompt(&retrieved),
        });
    }
    messages.extend(history);

    // Stream the reply, forwarding each token to the UI.
    let result = openrouter::stream_chat(api_key.expose(), &models, &messages, |token| {
        let _ = on_event.send(ChatEvent::Token {
            text: token.to_string(),
        });
    })
    .await;

    let completion = match result {
        Ok(c) => c,
        Err(e) => {
            let _ = on_event.send(ChatEvent::Error {
                message: e.to_string(),
            });
            return Err(e);
        }
    };
    let reply = completion.text;
    let usage = completion.usage;
    // Record the model that actually answered — the served one (so a fallback is
    // reflected), falling back to the requested primary if it wasn't reported.
    let used_model = completion
        .model
        .unwrap_or_else(|| models.first().cloned().unwrap_or_default());

    // Persist the assistant turn with the documents it cited (JSON, or NULL).
    let citations_json = if citations.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&citations).map_err(|e| Error::Other(e.to_string()))?)
    };
    let message_id = {
        let conn = state.conn()?;
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content, model, citations) \
             VALUES (?1, 'assistant', ?2, ?3, ?4)",
            params![conversation_id, reply, used_model, citations_json],
        )?;
        let id = conn.last_insert_rowid();
        log_usage(&conn, "chat", Some(&used_model), &usage);
        conn.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            params![conversation_id],
        )?;
        id
    };

    let _ = on_event.send(ChatEvent::Done {
        message_id,
        content: reply,
        citations,
    });
    Ok(())
}

/// Retrieve grounding chunks for a chat query — best-effort. Returns an empty
/// list (so chat falls back to ungrounded answering) if there are no documents
/// or the document engine isn't ready yet; never errors out the chat. Runs the
/// blocking embed + search off the async runtime, and never holds the DB lock
/// across the sidecar embed call (AGENTS rule #4).
async fn retrieve_grounding(
    app: &AppHandle,
    query: String,
    project: Option<String>,
) -> Vec<RetrievedChunk> {
    let app = app.clone();
    let task = tokio::task::spawn_blocking(move || -> Result<Vec<RetrievedChunk>> {
        let state = app.state::<AppState>();

        // Nothing to ground on?
        let has_docs: bool = {
            let conn = state.conn()?;
            conn.query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))?
        };
        if !has_docs {
            return Ok(Vec::new());
        }
        // Don't trigger a slow first-run install mid-chat — only embed if ready.
        if !matches!(state.sidecar.status(), SidecarStatus::Ready) {
            return Ok(Vec::new());
        }

        // Resolve the vault's models + the reranking toggle in one short lock, then drop it so
        // neither the query embed nor the rerank holds the DB lock across a sidecar call (#4).
        let (gateway, rerank_on) = {
            let conn = state.conn()?;
            (state.gateway(&conn)?, crate::db::reranking_enabled(&conn)?)
        };

        let embeddings = gateway.embed_query(std::slice::from_ref(&query))?;
        let Some(query_vec) = embeddings.into_iter().next() else {
            return Ok(Vec::new());
        };

        let q = retrieval::RetrieveQuery {
            text: &query,
            embedding: &query_vec,
            k: retrieval::DEFAULT_TOP_K,
            filters: retrieval::Filters {
                project: project.clone(),
                ..Default::default()
            },
            strategy: retrieval::Strategy::HybridRrf,
        };
        // Fuse under the lock, then drop it before reranking — the cross-encoder is a sidecar
        // call that can block on a model download. Reranking off (toggle) skips it entirely.
        let fused = {
            let conn = state.conn()?;
            retrieval::retrieve_fused(&conn, &q)?
        };
        let reranker = rerank_on.then_some(&gateway as &dyn retrieval::Reranker);
        retrieval::rerank(reranker, &query, fused)
    })
    .await;

    match task {
        Ok(Ok(chunks)) => chunks,
        _ => Vec::new(),
    }
}

// --- archivist: documents ---

/// Where the document engine (Python sidecar) is in its lifecycle, so the UI can
/// show first-run setup.
#[tauri::command]
pub fn sidecar_status(state: State<'_, AppState>) -> SidecarStatus {
    state.sidecar.status()
}

/// Provision the managed venv if needed (slow on first run). Run off the async
/// runtime so the UI stays responsive.
#[tauri::command]
pub async fn ensure_sidecar(app: AppHandle) -> Result<()> {
    tokio::task::spawn_blocking(move || app.state::<AppState>().sidecar.ensure_installed())
        .await
        .map_err(|e| Error::Other(format!("setup task panicked: {e}")))?
}

/// Ingest files/folders: convert → chunk → embed → index. Progress streams over
/// `on_event`. The whole pipeline is blocking, so it runs on a blocking thread.
///
/// `paths` are raw filesystem paths, so this is effectively an arbitrary-file-read
/// primitive — deliberately trusted: the only caller is PM's own webview, and the
/// paths come from the user's drag-drop / file-dialog (the same reach the dialog
/// already grants). It is not exposed to any external/untrusted caller.
#[tauri::command]
pub async fn ingest_paths(
    app: AppHandle,
    paths: Vec<String>,
    on_event: Channel<IngestEvent>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || ingest::run(&app, paths, on_event))
        .await
        .map_err(|e| Error::Other(format!("ingest task panicked: {e}")))?
}

/// Drop the index and rebuild it from the Markdown vault (spec §3 acceptance).
#[tauri::command]
pub async fn rebuild_index(app: AppHandle, on_event: Channel<IngestEvent>) -> Result<()> {
    tokio::task::spawn_blocking(move || ingest::rebuild(&app, on_event))
        .await
        .map_err(|e| Error::Other(format!("rebuild task panicked: {e}")))?
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> Result<Vec<Document>> {
    let conn = state.conn()?;
    ingest::list_documents(&conn)
}

/// Direct hybrid search over the store — the retrieval loop exposed on its own
/// (a search surface, and the way to verify exact-term recall independently of
/// chat). Embeds the query via the sidecar, so it ensures the engine is set up.
#[tauri::command]
pub async fn search_documents(
    app: AppHandle,
    query: String,
    k: Option<usize>,
) -> Result<Vec<RetrievedChunk>> {
    const MAX_K: usize = 50;
    let k = k.unwrap_or(retrieval::DEFAULT_TOP_K).min(MAX_K);
    tokio::task::spawn_blocking(move || -> Result<Vec<RetrievedChunk>> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;

        // Resolve models + the reranking toggle in one short lock, then drop it so neither the
        // query embed nor the rerank holds the DB lock across a sidecar call (#4).
        let (gateway, rerank_on) = {
            let conn = state.conn()?;
            (state.gateway(&conn)?, crate::db::reranking_enabled(&conn)?)
        };
        let embeddings = gateway.embed_query(std::slice::from_ref(&query))?;
        let query_vec = embeddings.into_iter().next().unwrap_or_default();

        let q = retrieval::RetrieveQuery {
            text: &query,
            embedding: &query_vec,
            k,
            filters: retrieval::Filters::default(),
            strategy: retrieval::Strategy::HybridRrf,
        };
        // Fuse under the lock, then rerank off it (the cross-encoder is a sidecar call).
        let fused = {
            let conn = state.conn()?;
            retrieval::retrieve_fused(&conn, &q)?
        };
        let reranker = rerank_on.then_some(&gateway as &dyn retrieval::Reranker);
        retrieval::rerank(reranker, &query, fused)
    })
    .await
    .map_err(|e| Error::Other(format!("search task panicked: {e}")))?
}

/// Transcribe a recorded voice clip to text for the chat box (spec §4 P1 — voice
/// input). The webview records the clip and sends it base64-encoded; we decode it
/// to a temp file inside the data dir, transcribe it locally via the sidecar's
/// Whisper model, and delete the file. An explicit user action, so it ensures the
/// engine is installed first (mirrors `search_documents`). Fully on-device — the
/// audio never leaves the machine. All blocking, so it runs off the async runtime.
#[tauri::command]
pub async fn transcribe_audio(app: AppHandle, audio_base64: String) -> Result<String> {
    use base64::Engine;

    tokio::task::spawn_blocking(move || -> Result<String> {
        // Bound the untrusted webview payload before allocating the decode buffer
        // (every other webview input is capped). ~32 MiB of base64 ≈ 24 MiB of
        // audio — far more than a dictation clip, but it stops a hostile/oversized
        // string from ballooning memory on a low-RAM machine.
        const MAX_AUDIO_B64_CHARS: usize = 32 * 1024 * 1024;
        let b64 = audio_base64.trim();
        if b64.len() > MAX_AUDIO_B64_CHARS {
            return Err(Error::Other(
                "the recording is too large to transcribe".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::Other(format!("could not decode the recording: {e}")))?;
        if bytes.is_empty() {
            return Ok(String::new());
        }

        // Keep the clip inside PM's data dir (not the system temp) so it shares the
        // user's at-rest disk encryption. A random-named NamedTempFile deletes
        // itself on drop (RAII), so even a crash mid-transcribe can't leave the raw
        // audio behind under a predictable name.
        use std::io::Write;
        let tmp_dir = paths::data_dir(&app)?.join("runtime").join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let mut clip = tempfile::Builder::new()
            .prefix("voice-")
            .suffix(".webm")
            .tempfile_in(&tmp_dir)?;
        clip.write_all(&bytes)?;
        clip.flush()?;

        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
        let text = state.sidecar.transcribe(clip.path());

        // `clip` drops at end of scope, deleting the temp file on success or error.
        text
    })
    .await
    .map_err(|e| Error::Other(format!("transcription task panicked: {e}")))?
}

// --- archivist: sorting review & organisation (Step 4) ---

/// Distinct project labels across all documents — feeds the review project picker
/// and biases the AI proposal toward projects that already exist.
#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<String>> {
    let conn = state.conn()?;
    db::distinct_projects(&conn)
}

/// Documents still awaiting the sorting review (`reviewed = 0`).
#[tauri::command]
pub fn review_queue(state: State<'_, AppState>) -> Result<Vec<Document>> {
    let conn = state.conn()?;
    ingest::review_queue(&conn)
}

/// Propose project/tags/importance for the unreviewed documents, on demand (so a
/// big folder import doesn't auto-fire model calls). Proposals stream back over
/// `on_event`; they're transient — the user confirms them via `commit_review`.
/// Runs on the background API key; never holds the DB lock across a model call.
#[tauri::command]
pub async fn propose_metadata(
    app: AppHandle,
    document_ids: Option<Vec<i64>>,
    on_event: Channel<ReviewEvent>,
) -> Result<()> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    // Bound the (untrusted webview) id list: it expands to one SQL placeholder
    // each, so an unbounded list would blow SQLITE_MAX_VARIABLE_NUMBER. Far above
    // any real review selection.
    const MAX_PROPOSE_IDS: usize = 10_000;
    if document_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > MAX_PROPOSE_IDS)
    {
        return Err(Error::Other("too many documents selected at once".into()));
    }

    struct Pending {
        id: i64,
        title: String,
        body: String,
    }

    // Gather the documents + existing projects + learned profile under a short
    // lock, then drop it before any network call (rule #4).
    let (pending, projects, models, profile) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let profile = learning::profile_preamble(&conn)?;
        let projects = db::distinct_projects(&conn)?;
        let pending = {
            let base_sql = "SELECT d.id, d.title, \
                    COALESCE((SELECT content FROM chunks c WHERE c.document_id = d.id ORDER BY ordinal LIMIT 1), '') \
             FROM documents d WHERE d.reviewed = 0";

            let pending_sql = if let Some(ids) = document_ids.as_ref() {
                if ids.is_empty() {
                    format!("{base_sql} AND 1=0 ORDER BY d.ingested_at DESC, d.id DESC")
                } else {
                    let placeholders = std::iter::repeat_n("?", ids.len())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{base_sql} AND d.id IN ({placeholders}) ORDER BY d.ingested_at DESC, d.id DESC")
                }
            } else {
                format!("{base_sql} ORDER BY d.ingested_at DESC, d.id DESC")
            };

            let mut stmt = conn.prepare(&pending_sql)?;
            if let Some(ids) = document_ids.as_ref().filter(|ids| !ids.is_empty()) {
                stmt.query_map(rusqlite::params_from_iter(ids), |r| {
                    Ok(Pending {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            } else {
                stmt.query_map([], |r| {
                    Ok(Pending {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        (pending, projects, models, profile)
    };

    let mut proposed = 0;
    let mut usage_rows: Vec<(Option<String>, openrouter::Usage)> = Vec::new();
    for p in pending {
        let (proposal, usage_info) = review::propose(
            api_key.expose(),
            &models,
            &p.title,
            &p.body,
            &projects,
            profile.as_deref(),
        )
        .await;
        if let Some((usage, served)) = usage_info {
            usage_rows.push((served, usage));
        }
        let _ = on_event.send(ReviewEvent::Proposed {
            document_id: p.id,
            proposal,
        });
        proposed += 1;
    }
    log_background_usage(&app, &models, &usage_rows);
    let _ = on_event.send(ReviewEvent::Finished { proposed });
    Ok(())
}

/// Commit a review pass: for each decision, log the fields the user changed from
/// the AI proposal, then write the confirmed metadata to the vault + DB and mark
/// the document reviewed. Blocking (file rewrites), so it runs off the runtime.
#[tauri::command]
pub async fn commit_review(app: AppHandle, decisions: Vec<ReviewDecision>) -> Result<()> {
    let blocking_app = app.clone();
    let logged = tokio::task::spawn_blocking(move || -> Result<usize> {
        let state = blocking_app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let now = iso_now(&state)?;

        // The whole pass is all-or-nothing: corrections, vault rewrites, and the
        // `reviewed` flags commit together, or the DB transaction rolls back and
        // every vault file we touched is restored. Otherwise a failure partway
        // through would leave earlier docs marked reviewed (dropped from the queue
        // on retry, their corrections never re-logged) and mid-batch vault/DB drift.
        let mut conn = state.conn()?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        let result: Result<usize> = (|| {
            let mut logged = 0usize;
            for d in &decisions {
                let title: String = tx
                    .query_row(
                        "SELECT title FROM documents WHERE id = ?1",
                        params![d.document_id],
                        |r| r.get(0),
                    )
                    .unwrap_or_default();
                logged += review::log_corrections(&tx, d, &title)?;
                let importance = review::normalize_importance(d.importance.clone());
                let w = ingest::rewrite_vault_metadata(
                    &tx,
                    &vault,
                    &cipher,
                    d.document_id,
                    &d.project,
                    &d.tags,
                    importance.as_deref(),
                    true,
                    &now,
                )?;
                written.push(w);
            }
            Ok(logged)
        })();

        match result {
            Ok(logged) => match tx.commit() {
                Ok(()) => Ok(logged),
                Err(e) => {
                    ingest::restore_vault_files(written);
                    Err(e.into())
                }
            },
            Err(e) => {
                drop(tx); // roll back the DB side
                ingest::restore_vault_files(written);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| Error::Other(format!("commit task panicked: {e}")))??;

    // If the user corrected anything, refresh the Learning-You profile in the
    // background — one model call per review pass (not per document), best-effort
    // and non-blocking so the UI returns immediately (Step 4b, spec §4.5).
    if logged > 0 {
        tauri::async_runtime::spawn(async move {
            let _ = run_profile_refresh(app).await;
        });
    }
    Ok(())
}

/// Edit one already-reviewed document's metadata (the after-the-fact "this is
/// Project 2, not 3"). Logs the change against the currently stored values.
#[tauri::command]
pub async fn set_document_metadata(
    app: AppHandle,
    document_id: i64,
    project: String,
    tags: Vec<String>,
    importance: Option<String>,
) -> Result<Document> {
    let importance = review::normalize_importance(importance);
    tokio::task::spawn_blocking(move || -> Result<Document> {
        let state = app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let now = iso_now(&state)?;

        // Log the correction + rewrite the vault file + update the row atomically,
        // restoring the vault file if the DB side fails (the file write lands first).
        let mut conn = state.conn()?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        let work = (|| -> Result<()> {
            let (cur_project, cur_tags_json, cur_importance, title): (
                String,
                String,
                Option<String>,
                String,
            ) = tx.query_row(
                "SELECT project, tags, importance, title FROM documents WHERE id = ?1",
                params![document_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            let decision = ReviewDecision {
                document_id,
                project: project.clone(),
                tags: tags.clone(),
                importance: importance.clone(),
                proposed_project: cur_project,
                proposed_tags: serde_json::from_str(&cur_tags_json).unwrap_or_default(),
                proposed_importance: cur_importance,
            };
            review::log_corrections(&tx, &decision, &title)?;
            written.push(ingest::rewrite_vault_metadata(
                &tx,
                &vault,
                &cipher,
                document_id,
                &project,
                &tags,
                importance.as_deref(),
                true,
                &now,
            )?);
            Ok(())
        })();

        if let Err(e) = work {
            drop(tx);
            ingest::restore_vault_files(written);
            return Err(e);
        }
        if let Err(e) = tx.commit() {
            ingest::restore_vault_files(written);
            return Err(e.into());
        }
        ingest::load_document(&conn, document_id)
    })
    .await
    .map_err(|e| Error::Other(format!("update task panicked: {e}")))?
}

// --- personal assistant: projects & focus view (Step 5) ---

/// Every active project with its triage metadata and one derived status — the
/// focus view's data (spec §4.1).
#[tauri::command]
pub fn list_project_overviews(state: State<'_, AppState>) -> Result<Vec<ProjectOverview>> {
    let conn = state.conn()?;
    let today = clock::today_sql_in(resolve_zone(&conn));
    projects::list_overviews(&conn, &today)
}

/// Set (or update) a project's triage metadata — the user confirming/correcting an
/// AI proposal, or editing by hand in the focus/project view. Creates the row on
/// first set; blanks clear a field.
#[tauri::command]
pub fn set_project_metadata(
    state: State<'_, AppState>,
    name: String,
    deadline: Option<String>,
    size: Option<String>,
    blocked_by: Option<String>,
    parent: Option<String>,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Other("project name is empty".into()));
    }
    let conn = state.conn()?;
    projects::set_metadata(&conn, name, deadline, size, blocked_by, parent)
}

/// Propose triage metadata (size/parent/blocked-by/deadline) for projects, on
/// demand — the AI-proposes-you-confirm half of the focus view, mirroring
/// `propose_metadata`. `names` limits it to specific projects (default: all).
/// Proposals stream over `on_event`; the user confirms via `set_project_metadata`.
/// Runs on the background API key; never holds the DB lock across a model call.
#[tauri::command]
pub async fn propose_project_metadata(
    app: AppHandle,
    names: Option<Vec<String>>,
    on_event: Channel<ProjectProposalEvent>,
) -> Result<()> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    // Bound the (untrusted webview) name list — one model call per name, so this
    // also caps runaway spend. Far above any real project count.
    const MAX_PROPOSE_NAMES: usize = 2_000;
    if names.as_ref().is_some_and(|n| n.len() > MAX_PROPOSE_NAMES) {
        return Err(Error::Other("too many projects selected at once".into()));
    }

    struct Target {
        name: String,
        samples: Vec<String>,
    }

    // Gather targets + their document samples + the full project list (for picking
    // a real parent/blocker) + models under a short lock, then drop it (rule #4).
    let (targets, all_projects, models) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let all_projects: Vec<String> = db::distinct_projects(&conn)?;
        let target_names = match names {
            Some(n) if !n.is_empty() => n,
            _ => all_projects.clone(),
        };
        let mut targets = Vec::new();
        for name in target_names {
            let samples = projects::document_samples(&conn, &name)?;
            targets.push(Target { name, samples });
        }
        (targets, all_projects, models)
    };

    let mut proposed = 0;
    let mut usage_rows: Vec<(Option<String>, openrouter::Usage)> = Vec::new();
    for t in targets {
        let others: Vec<String> = all_projects
            .iter()
            .filter(|p| **p != t.name)
            .cloned()
            .collect();
        let (proposal, usage_info) =
            projects::propose(api_key.expose(), &models, &t.name, &t.samples, &others).await;
        if let Some((usage, served)) = usage_info {
            usage_rows.push((served, usage));
        }
        let _ = on_event.send(ProjectProposalEvent::Proposed {
            project: t.name,
            proposal,
        });
        proposed += 1;
    }
    log_background_usage(&app, &models, &usage_rows);
    let _ = on_event.send(ProjectProposalEvent::Finished { proposed });
    Ok(())
}

// --- personal assistant: calendar (Step 6) ---

/// The calendar connector's state, for the Settings panel. Covers both paths: the
/// simple .ics feeds and the advanced Google OAuth sign-in.
#[derive(Serialize)]
pub struct CalendarStatus {
    /// How many .ics feeds are subscribed (the no-OAuth path).
    pub ics_feeds: usize,
    /// The user has pasted a Google client id + secret.
    pub oauth_client_configured: bool,
    /// A Google OAuth token is stored (sign-in completed).
    pub oauth_connected: bool,
    /// How many Google calendars are selected to sync.
    pub calendars_selected: usize,
    /// ISO timestamp of the last successful sync, if any.
    pub last_sync: Option<String>,
    /// How far ahead PM mirrors events (and the agenda horizon), in days.
    pub window_days: i64,
}

#[tauri::command]
pub fn calendar_status(state: State<'_, AppState>) -> Result<CalendarStatus> {
    let conn = state.conn()?;
    Ok(CalendarStatus {
        ics_feeds: calendar::load_feeds()?.len(),
        oauth_client_configured: google::has_client()?,
        oauth_connected: google::is_connected()?,
        calendars_selected: calendar::selected_calendar_ids(&conn)?.len(),
        last_sync: calendar::last_sync(&conn)?,
        window_days: calendar::AGENDA_DAYS,
    })
}

// .ics feeds — the no-OAuth path (works under Advanced Protection).

/// Subscribed feeds without their secret URLs, for Settings.
#[tauri::command]
pub fn list_ics_feeds() -> Result<Vec<IcsFeedInfo>> {
    calendar::feed_infos()
}

/// Add an .ics feed and sync it immediately so its events appear. If it can't be
/// fetched/parsed, it's rolled back so a broken feed isn't left behind.
#[tauri::command]
pub async fn add_ics_feed(app: AppHandle, label: String, url: String) -> Result<()> {
    let feed = calendar::add_feed(&label, &url)?;
    // Resolve the user's zone (for floating/all-day ICS times) under a short lock,
    // then drop it before the network sync (rule #4).
    let tz = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        resolve_zone(&conn)
    };
    match calendar::sync_feed(&feed, tz).await {
        Ok(events) => {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            calendar::replace_events(&conn, &feed.id, &events)?;
            calendar::set_last_sync(&conn)?;
            Ok(())
        }
        Err(e) => {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            let _ = calendar::remove_feed(&conn, &feed.id);
            Err(e)
        }
    }
}

/// Remove a feed and its mirrored events.
#[tauri::command]
pub fn remove_ics_feed(state: State<'_, AppState>, id: String) -> Result<()> {
    let conn = state.conn()?;
    calendar::remove_feed(&conn, &id)
}

/// Store the user's BYO Google "Desktop app" client credentials (keychain only).
#[tauri::command]
pub fn set_google_client(client_id: String, client_secret: String) -> Result<()> {
    let id = client_id.trim();
    let secret = client_secret.trim();
    if id.is_empty() || secret.is_empty() {
        return Err(Error::Other(
            "Both the Client ID and Client secret are required.".into(),
        ));
    }
    secrets::set_google_client(id, secret)
}

/// Forget the client credentials (also disconnects + clears the mirror, since the
/// token belongs to that client).
#[tauri::command]
pub fn clear_google_client(state: State<'_, AppState>) -> Result<()> {
    secrets::clear_google_token().ok();
    secrets::clear_google_client()?;
    let conn = state.conn()?;
    calendar::clear_all_events(&conn)
}

/// Run the OAuth consent flow (opens the system browser; resolves once the user
/// signs in or it times out).
#[tauri::command]
pub async fn connect_google() -> Result<()> {
    google::connect().await
}

/// Sign out: forget the token and clear the mirrored events. Client credentials stay
/// so the user can reconnect without re-entering them.
#[tauri::command]
pub fn disconnect_google(state: State<'_, AppState>) -> Result<()> {
    secrets::clear_google_token()?;
    let conn = state.conn()?;
    calendar::clear_all_events(&conn)
}

/// The user's calendars, with PM's current selection applied (for the picker).
#[tauri::command]
pub async fn list_google_calendars(app: AppHandle) -> Result<Vec<CalendarInfo>> {
    let raw = calendar::fetch_calendar_list().await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let selected = calendar::selected_calendar_ids(&conn)?;
    Ok(calendar::to_calendar_infos(raw, &selected))
}

/// Choose which calendars to sync.
#[tauri::command]
pub fn set_google_calendar_ids(state: State<'_, AppState>, ids: Vec<String>) -> Result<()> {
    let conn = state.conn()?;
    calendar::set_selected_calendar_ids(&conn, &ids)
}

/// Pull events from both sources (subscribed .ics feeds + selected Google calendars)
/// into the local mirror. Returns the number of events synced. Best-effort per source
/// and never holds the DB lock across a fetch (rule #4); surfaces an error only if
/// every source failed (so a transient miss keeps the last-good events).
#[tauri::command]
pub async fn sync_calendar(app: AppHandle) -> Result<usize> {
    let (oauth_ids, feeds, time_min, time_max, tz) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let oauth_ids = if google::is_connected()? {
            calendar::selected_calendar_ids(&conn)?
        } else {
            Vec::new()
        };
        let feeds = calendar::load_feeds()?;
        let (min, max) = calendar::time_window(&conn)?;
        (oauth_ids, feeds, min, max, resolve_zone(&conn))
    };

    // Every calendar/feed we intend to keep events for — anything else is pruned.
    let mut active: Vec<String> = oauth_ids.clone();
    active.extend(feeds.iter().map(|f| f.id.clone()));

    if active.is_empty() {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        calendar::clear_all_events(&conn)?;
        calendar::set_last_sync(&conn)?;
        return Ok(0);
    }

    let mut total = 0usize;
    let mut last_err: Option<Error> = None;

    for id in &oauth_ids {
        match calendar::fetch_events(id, &time_min, &time_max).await {
            Ok(events) => {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                calendar::replace_events(&conn, id, &events)?;
                total += events.len();
            }
            Err(e) => last_err = Some(e),
        }
    }
    for feed in &feeds {
        match calendar::sync_feed(feed, tz).await {
            Ok(events) => {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                calendar::replace_events(&conn, &feed.id, &events)?;
                total += events.len();
            }
            Err(e) => last_err = Some(e),
        }
    }

    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        // Reconcile deselected calendars. A source that failed *this* round keeps
        // its last-good events (standard cache behaviour) rather than being blanked.
        calendar::prune_unselected(&conn, &active)?;
        // Only record a clean sync when every selected source refreshed — a partial
        // failure must not hide behind a fresh "last synced" timestamp.
        if last_err.is_none() {
            calendar::set_last_sync(&conn)?;
        }
    }

    // Surface any source failure (auth/expired, or a bad feed URL) even when other
    // sources succeeded; the successful ones are already committed above.
    if let Some(e) = last_err {
        return Err(e);
    }
    Ok(total)
}

/// The upcoming events in the mirror, for the focus-view agenda.
#[tauri::command]
pub fn list_calendar_events(state: State<'_, AppState>) -> Result<Vec<CalendarEvent>> {
    let conn = state.conn()?;
    calendar::list_upcoming(&conn, calendar::AGENDA_DAYS)
}

// --- learning you (Step 4b) ---

/// The distilled Learning-You profile + when it was last updated and how many
/// corrections back it, for display in Settings.
#[tauri::command]
pub fn get_learning_profile(state: State<'_, AppState>) -> Result<learning::LearningProfile> {
    let conn = state.conn()?;
    learning::get_profile(&conn)
}

/// Re-distil the Learning-You profile from the logged corrections, on demand
/// (the "Refresh now" button). Returns the refreshed profile.
#[tauri::command]
pub async fn refresh_learning_profile(app: AppHandle) -> Result<learning::LearningProfile> {
    run_profile_refresh(app.clone()).await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    learning::get_profile(&conn)
}

/// Gather corrections + the current profile, distil an updated profile via the
/// background model, and persist it. Background work: runs on the background API
/// key and never holds the DB lock across the model call (rule #4). A no-op when
/// there are no corrections to learn from yet.
async fn run_profile_refresh(app: AppHandle) -> Result<()> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    let (current, corrections, models) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let current = learning::get_profile(&conn)?.profile;
        let corrections = learning::recent_corrections(&conn, learning::MAX_CORRECTIONS)?;
        (current, corrections, models)
    };

    if corrections.is_empty() {
        return Ok(());
    }

    let (updated, usage, served) =
        learning::distill(api_key.expose(), &models, &current, &corrections).await?;

    let state = app.state::<AppState>();
    let now = iso_now(&state)?;
    let conn = state.conn()?;
    log_usage(
        &conn,
        "background",
        served
            .as_deref()
            .or_else(|| models.first().map(String::as_str)),
        &usage,
    );
    learning::save_profile(&conn, &updated, &now)
}

// --- daily briefing (Step 7, spec §4 P1) ---

/// The stored "here's your picture today" briefing + whether it's due a refresh, for
/// the focus view. Read-only — no model call, so it's cheap on every mount.
#[tauri::command]
pub fn get_daily_briefing(state: State<'_, AppState>) -> Result<briefing::DailyBriefing> {
    let conn = state.conn()?;
    briefing::get_briefing(&conn)
}

/// Regenerate the daily briefing from the current focus-view state (the "Refresh"
/// button, and the focus view's once-a-day auto-refresh when stale). Returns the new
/// briefing. Background work: runs on the background API key, never holds the DB lock
/// across the model call (rule #4), and is a no-op (returns the stored value) when
/// there's nothing to summarise.
#[tauri::command]
pub async fn refresh_daily_briefing(app: AppHandle) -> Result<briefing::DailyBriefing> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    let (snapshot, profile, models) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let zone = resolve_zone(&conn);
        let now = clock::now_local_iso(zone);
        let today = clock::today_sql_in(zone);
        let projects = projects::list_overviews(&conn, &today)?;
        let events = calendar::list_upcoming(&conn, briefing::BRIEFING_AGENDA_DAYS)?;
        let snapshot = briefing::build_snapshot(&projects, &events, &now, zone);
        let profile = learning::profile_preamble(&conn)?;
        (snapshot, profile, models)
    };

    // Nothing to brief on yet — leave any prior briefing in place.
    let Some(snapshot) = snapshot else {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        return briefing::get_briefing(&conn);
    };

    let (text, usage, served) =
        briefing::generate(api_key.expose(), &models, &snapshot, profile.as_deref()).await?;

    let state = app.state::<AppState>();
    let now = iso_now(&state)?;
    let conn = state.conn()?;
    log_usage(
        &conn,
        "background",
        served
            .as_deref()
            .or_else(|| models.first().map(String::as_str)),
        &usage,
    );
    briefing::save_briefing(&conn, &text, &now)?;
    briefing::get_briefing(&conn)
}

// --- cost logger (spec §11.2 / §17.1) ---

/// Spend for one model over a window. `cost_usd` is `None` when the model isn't in
/// the price cache yet — surfaced as "—", never an understated $0.
#[derive(Serialize)]
pub struct ModelSpend {
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Option<f64>,
}

/// The Settings "Usage & cost" payload: per-model spend over two windows + totals,
/// plus when the cached pricing was last refreshed.
#[derive(Serialize)]
pub struct CostSummary {
    pub last_30d: Vec<ModelSpend>,
    pub all_time: Vec<ModelSpend>,
    pub total_30d_usd: Option<f64>,
    pub total_all_time_usd: Option<f64>,
    pub pricing_updated_at: Option<String>,
}

/// Per-model spend (trailing 30 days + all time) joined against the cached OpenRouter
/// prices. CHECK-ON-READ: if the price cache is empty or older than a day, refresh it
/// from the public catalogue first (no key, no model call, no scheduler — mirrors the
/// briefing's staleness rule). Read-mostly; safe on every Settings open.
#[tauri::command]
pub async fn cost_summary(app: AppHandle) -> Result<CostSummary> {
    // Best-effort refresh: if it fails (offline, etc.) still return the summary —
    // token counts come from the local log and need no network; only the priced
    // costs fall back to "unknown". The explicit "Refresh prices" button surfaces
    // the error instead.
    let _ = ensure_pricing_fresh(&app).await;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    build_cost_summary(&conn)
}

/// Force a re-pull of OpenRouter's public pricing into the cache, then return the
/// refreshed summary (the Settings "Refresh prices" action).
#[tauri::command]
pub async fn refresh_pricing(app: AppHandle) -> Result<CostSummary> {
    refresh_pricing_now(&app).await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    build_cost_summary(&conn)
}

// --- model recommender (spec §6) ---

/// PM's two live model recommendations for the Settings cards. `day_to_day` /`advanced`
/// are `null` when the cache can't yet produce a pick (e.g. offline before any fetch) —
/// the UI shows "unavailable", never a silent non-compliant fallback. `zdr_enforced` is
/// always true (PM sends Zero-Data-Retention on every request — see `openrouter::chat_body`);
/// the UI shows it on each card so the user sees why a model is safe. `stale` flags a cache
/// older than the daily refresh window (a failed/offline refresh), so the UI can mark the
/// picks possibly out of date.
#[derive(Serialize)]
pub struct ModelRecommendations {
    pub day_to_day: Option<recommend::Recommendation>,
    pub advanced: Option<recommend::Recommendation>,
    pub denylist: Vec<String>,
    pub zdr_enforced: bool,
    pub stale: bool,
}

/// Compute the two recommendations from the cached catalogue + curated tier list + the
/// user denylist. Reuses the cost logger's daily price refresh (no second fetch) and is
/// fail-safe: a best-effort refresh keeps the last-good list when offline, and an
/// empty/stale cache yields `null` picks with `stale = true` rather than an invented one.
#[tauri::command]
pub async fn model_recommendations(app: AppHandle) -> Result<ModelRecommendations> {
    let _ = ensure_catalogue_fresh(&app).await; // best-effort; offline keeps the last-good list
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let catalogue = cached_catalogue(&conn)?;
    let denylist = recommend_denylist(&conn)?;
    // Reuse the cost logger's staleness rule: if the best-effort refresh above couldn't
    // freshen the cache (offline), the newest fetch is old → flag the picks as stale.
    let hours: Option<f64> = conn
        .query_row(
            "SELECT (julianday('now') - julianday(replace(MAX(fetched_at),'Z',''))) * 24.0 FROM model_pricing",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let stale = cost::pricing_is_stale(hours);
    let curated = curated_tiers();
    let (day_to_day, advanced) = recommend::recommend(&catalogue, &curated, &denylist);
    Ok(ModelRecommendations {
        day_to_day,
        advanced,
        denylist,
        zdr_enforced: true,
        stale,
    })
}

/// Persist the recommender denylist (provider/model slugs). Cleaned like model lists:
/// trimmed, empties dropped, capped.
#[tauri::command]
pub fn set_recommend_denylist(state: State<'_, AppState>, denylist: Vec<String>) -> Result<()> {
    let cleaned: Vec<String> = denylist
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(100)
        .collect();
    let json = serde_json::to_string(&cleaned).map_err(|e| Error::Other(e.to_string()))?;
    let conn = state.conn()?;
    db::set_setting(&conn, RECOMMEND_DENYLIST_KEY, &json)
}

/// The curated faithfulness list, embedded at compile time so it ships in the binary (a
/// relocatable app has no fixed runtime path). A parse failure degrades to an empty list —
/// the live intelligence index still drives the Advanced pick — never a crash.
fn curated_tiers() -> recommend::CuratedTiers {
    const RAW: &str = include_str!("../recommend_tiers.json");
    serde_json::from_str(RAW).unwrap_or_default()
}

/// Read the stored recommender denylist (empty when unset/unparseable).
fn recommend_denylist(conn: &Connection) -> Result<Vec<String>> {
    Ok(db::get_setting(conn, RECOMMEND_DENYLIST_KEY)?
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default())
}

/// Reconstruct the recommender's view of the catalogue from the daily price/signal cache
/// (`model_pricing`, extended in migration v8). Reading from the cache — not a live fetch —
/// is what lets recommendations survive offline. Only the **latest refresh batch** is in
/// scope (`fetched_at = MAX(fetched_at)`): a model that has left OpenRouter keeps an older
/// timestamp and is excluded, so the recommender never surfaces a model that can no longer
/// be served under PM's ZDR enforcement. (The cost-summary join reads `model_pricing`
/// unfiltered, so historical spend on a now-removed model is still priced.)
fn cached_catalogue(conn: &Connection) -> Result<Vec<openrouter::ModelDetail>> {
    let mut stmt = conn.prepare(
        "SELECT model, COALESCE(name, ''), context_length, prompt_price, completion_price, \
                cache_read_price, supported_parameters, input_modalities, intelligence_index \
         FROM model_pricing \
         WHERE fetched_at = (SELECT MAX(fetched_at) FROM model_pricing)",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let context_length: Option<i64> = r.get(2)?;
            let supported: Option<String> = r.get(6)?;
            let modalities: Option<String> = r.get(7)?;
            Ok(openrouter::ModelDetail {
                id: r.get(0)?,
                name: r.get(1)?,
                description: String::new(),
                context_length: context_length.map(|v| v as u64),
                prompt_price: r.get(3)?,
                completion_price: r.get(4)?,
                cache_read_price: r.get(5)?,
                input_modalities: modalities
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                supported_parameters: supported
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                intelligence_index: r.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Append a `usage_log` row — best-effort: cost logging must never fail a model call,
/// so errors are swallowed. `model = None` is allowed (an unreported served model).
fn log_usage(conn: &Connection, kind: &str, model: Option<&str>, usage: &openrouter::Usage) {
    let _ = conn.execute(
        "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens) VALUES (?1, ?2, ?3, ?4)",
        params![model, kind, usage.prompt_tokens, usage.completion_tokens],
    );
}

/// Write collected background usage rows under one short lock (best-effort), each
/// attributed to its served model, or the requested primary when none was reported.
fn log_background_usage(
    app: &AppHandle,
    models: &[String],
    rows: &[(Option<String>, openrouter::Usage)],
) {
    if rows.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let Ok(conn) = state.conn() else { return };
    for (served, usage) in rows {
        let model = served
            .as_deref()
            .or_else(|| models.first().map(String::as_str));
        log_usage(&conn, "background", model, usage);
    }
}

/// Refresh the cached pricing when it's stale (check-on-read). Resolves staleness
/// under a short lock, then does the network fetch + upsert without holding it (rule #4).
async fn ensure_pricing_fresh(app: &AppHandle) -> Result<()> {
    let stale = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let hours: Option<f64> = conn
            .query_row(
                "SELECT (julianday('now') - julianday(replace(MAX(fetched_at),'Z',''))) * 24.0 FROM model_pricing",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        cost::pricing_is_stale(hours)
    };
    if stale {
        refresh_pricing_now(app).await?;
    }
    Ok(())
}

/// Like [`ensure_pricing_fresh`], but for the recommender: also force a refresh when the
/// cached catalogue is missing the v8 signal columns. An install that ran the cost logger
/// before this feature has a price-only cache with a recent `fetched_at` (so the age check
/// alone would skip the refresh) but NULL recommender signals — without this the cards would
/// show "no recommendations yet" until the next daily refresh. Resolves the decision under a
/// short lock, then fetches without holding it (rule #4).
async fn ensure_catalogue_fresh(app: &AppHandle) -> Result<()> {
    let needs_refresh = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let hours: Option<f64> = conn
            .query_row(
                "SELECT (julianday('now') - julianday(replace(MAX(fetched_at),'Z',''))) * 24.0 FROM model_pricing",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        // Does the newest batch actually carry the recommender signals? A price-only cache
        // written before this feature won't, so treat that as needing a refresh even if fresh.
        let has_signals: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM model_pricing \
                 WHERE fetched_at = (SELECT MAX(fetched_at) FROM model_pricing) \
                   AND context_length IS NOT NULL AND supported_parameters IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        cost::pricing_is_stale(hours) || !has_signals
    };
    if needs_refresh {
        refresh_pricing_now(app).await?;
    }
    Ok(())
}

/// Pull the public OpenRouter catalogue (no key) and upsert every model's prices — and
/// the recommender's signals (cache rate, context, supported params, capability indices) —
/// into the cache. One fetch serves both the cost logger and the recommender (no second
/// fetch/scheduler). Never holds the DB lock across the network call (rule #4).
async fn refresh_pricing_now(app: &AppHandle) -> Result<()> {
    let models = openrouter::fetch_catalogue().await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let tx = conn.unchecked_transaction()?;
    // One timestamp for the whole batch, so every model in this pull shares an identical
    // `fetched_at`. That lets the recommender read only the latest batch (a model that left
    // OpenRouter keeps an older timestamp and drops out of candidacy — see `cached_catalogue`),
    // and keeps the staleness check exact.
    let fetched_at: String =
        tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?;
    for m in &models {
        let supported =
            serde_json::to_string(&m.supported_parameters).unwrap_or_else(|_| "[]".into());
        let modalities = serde_json::to_string(&m.input_modalities).unwrap_or_else(|_| "[]".into());
        tx.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, name, context_length, \
                cache_read_price, supported_parameters, input_modalities, intelligence_index, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(model) DO UPDATE SET \
                prompt_price = ?2, completion_price = ?3, name = ?4, context_length = ?5, \
                cache_read_price = ?6, supported_parameters = ?7, input_modalities = ?8, \
                intelligence_index = ?9, fetched_at = ?10",
            params![
                m.id,
                m.prompt_price,
                m.completion_price,
                m.name,
                m.context_length.map(|v| v as i64),
                m.cache_read_price,
                supported,
                modalities,
                m.intelligence_index,
                fetched_at,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Assemble the cost summary from `usage_log` × the cached `model_pricing`.
fn build_cost_summary(conn: &Connection) -> Result<CostSummary> {
    let last_30d = spend_rows(conn, true)?;
    let all_time = spend_rows(conn, false)?;
    let total_30d_usd = total_cost(&last_30d);
    let total_all_time_usd = total_cost(&all_time);
    let pricing_updated_at: Option<String> = conn
        .query_row("SELECT MAX(fetched_at) FROM model_pricing", [], |r| {
            r.get(0)
        })
        .ok()
        .flatten();
    Ok(CostSummary {
        last_30d,
        all_time,
        total_30d_usd,
        total_all_time_usd,
        pricing_updated_at,
    })
}

/// Per-model token sums + request counts (optionally only the last 30 days), priced
/// from the cache; ordered by request count desc. Rows with a NULL model are excluded.
fn spend_rows(conn: &Connection, last_30d: bool) -> Result<Vec<ModelSpend>> {
    let window = if last_30d {
        "AND u.created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT u.model, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price \
         FROM usage_log u LEFT JOIN model_pricing p ON p.model = u.model \
         WHERE u.model IS NOT NULL {window} \
         GROUP BY u.model \
         ORDER BY COUNT(*) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map([], |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let completion_tokens: i64 = r.get(2)?;
            let prompt_price: Option<f64> = r.get(4)?;
            let completion_price: Option<f64> = r.get(5)?;
            Ok(ModelSpend {
                model: r.get(0)?,
                prompt_tokens,
                completion_tokens,
                request_count: r.get(3)?,
                cost_usd: cost::call_cost(
                    Some(prompt_tokens),
                    Some(completion_tokens),
                    prompt_price,
                    completion_price,
                ),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // Rank by cost (most expensive first); unpriced models (unknown cost) sort last,
    // then by request count — so the breakdown reads as a spend ranking.
    rows.sort_by(|a, b| {
        let ak = a.cost_usd.unwrap_or(f64::NEG_INFINITY);
        let bk = b.cost_usd.unwrap_or(f64::NEG_INFINITY);
        bk.partial_cmp(&ak)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.request_count.cmp(&a.request_count))
    });
    Ok(rows)
}

/// Total spend across rows: `Some(0)` with no usage, `None` when there's usage but no
/// model is priced yet, else the sum of the priced rows (unpriced models shown "—").
fn total_cost(rows: &[ModelSpend]) -> Option<f64> {
    if rows.is_empty() {
        return Some(0.0);
    }
    let known: Vec<f64> = rows.iter().filter_map(|r| r.cost_usd).collect();
    if known.is_empty() {
        return None;
    }
    Some(known.iter().sum())
}

// --- helpers ---

/// Current UTC time in the store's ISO8601 format (matches ingest timestamps).
fn iso_now(state: &AppState) -> Result<String> {
    let conn = state.conn()?;
    Ok(
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?,
    )
}

/// Resolve the user's stored IANA zone to a `chrono_tz::Tz`. Falls back to UTC when
/// the key is unset, empty, or unparseable — chrono `Local` only yields an offset
/// (no IANA name, DST-unstable), so the canonical zone is supplied by the frontend
/// (`Intl`) and stored; UTC is the stable default matching every `strftime('now')`.
/// Infallible by design (worst case UTC) so call sites stay one-liners.
fn resolve_zone(conn: &Connection) -> chrono_tz::Tz {
    use std::str::FromStr;
    db::get_setting(conn, TIME_ZONE_KEY)
        .ok()
        .flatten()
        .and_then(|s| chrono_tz::Tz::from_str(s.trim()).ok())
        .unwrap_or(chrono_tz::Tz::UTC)
}

fn current_model(conn: &Connection) -> Result<String> {
    Ok(db::get_setting(conn, "default_model")?.unwrap_or_else(|| DEFAULT_MODEL.to_string()))
}

/// The ordered model list stored for a role, parsed from its JSON setting. Falls
/// back to the single legacy `default_model` (or `DEFAULT_MODEL`) when unset or
/// empty, so existing installs keep working. Never returns an empty list.
fn models_for(conn: &Connection, key: &str) -> Result<Vec<String>> {
    if let Some(raw) = db::get_setting(conn, key)? {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
            let list: Vec<String> = list
                .into_iter()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect();
            if !list.is_empty() {
                return Ok(list);
            }
        }
    }
    Ok(vec![current_model(conn)?])
}

/// The effective list to send for a role: the full ordered list when auto-switch
/// is on (so OpenRouter can fall through to the next model on a rate-limit/quota
/// error), otherwise just the primary. Never empty.
fn effective_models(conn: &Connection, models_key: &str, auto_key: &str) -> Result<Vec<String>> {
    let models = models_for(conn, models_key)?;
    let auto = db::get_setting(conn, auto_key)?.as_deref() == Some("true");
    if auto {
        Ok(models)
    } else {
        Ok(vec![models
            .into_iter()
            .next()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())])
    }
}

/// Clean (trim, drop empties + over-long ids, de-dup preserving order, cap count)
/// and persist a role's model list as a JSON array. An empty result is stored as
/// `[]`; `models_for` then falls back to the default, so the role always resolves
/// to a usable model. The caps bound a frontend-supplied list that's persisted.
fn save_models(conn: &Connection, key: &str, models: Vec<String>) -> Result<()> {
    const MAX_MODELS: usize = 50;
    const MAX_MODEL_ID_CHARS: usize = 200;
    let mut seen = std::collections::HashSet::new();
    let cleaned: Vec<String> = models
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m.chars().count() <= MAX_MODEL_ID_CHARS)
        .filter(|m| seen.insert(m.clone()))
        .take(MAX_MODELS)
        .collect();
    let json = serde_json::to_string(&cleaned).map_err(|e| Error::Other(e.to_string()))?;
    db::set_setting(conn, key, &json)
}

fn row_to_conversation(row: &rusqlite::Row) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        project: row.get(4)?,
    })
}

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
    // Citations are stored as a JSON array string; tolerate NULL / malformed.
    let citations_raw: Option<String> = row.get(6)?;
    let citations = citations_raw.and_then(|s| serde_json::from_str(&s).ok());
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        model: row.get(4)?,
        created_at: row.get(5)?,
        citations,
    })
}

// --- data folder: reveal + export ---

/// Reveal the data folder (the encrypted store + the Markdown vault) in the OS file
/// manager — Explorer on Windows, Finder on macOS — so the user can find, back up,
/// or copy it. Uses the same `open` crate that launches the OAuth browser.
#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> Result<()> {
    let dir = paths::data_dir(&app)?;
    open::that(dir).map_err(Error::from)
}

/// Bundle the user's data into a single `.zip` at `dest_path`: the encrypted store
/// plus the Markdown vault. The regenerable `runtime/` (the Python venv + model
/// cache) is deliberately excluded.
///
/// The live `pm.sqlite` is never copied directly — WAL means freshly committed pages
/// can still live in the `-wal` sidecar — so we `VACUUM INTO` a consistent snapshot
/// first (which preserves SQLCipher encryption and folds in all WAL pages) under the
/// DB lock, then archive that snapshot as `pm.sqlite`. The lock is released before the
/// slower zip walk. The exported store stays encrypted with the same key, so it opens
/// only on a machine whose keychain holds this app's DB key.
#[tauri::command]
pub async fn export_all_data(
    app: AppHandle,
    state: State<'_, AppState>,
    dest_path: String,
) -> Result<()> {
    // A temp *directory* (not file) so `VACUUM INTO` writes a fresh file into an empty
    // dir — it refuses a pre-existing target. The dir (and snapshot) is removed on drop.
    let tmp = tempfile::Builder::new().prefix("pm-export-").tempdir()?;
    let snapshot = tmp.path().join("pm.sqlite");
    {
        let conn = state.conn()?;
        // VACUUM INTO takes a literal SQL string, not a bound parameter; escape any
        // single quote in the (tool-generated) path so it can't break out.
        let escaped = snapshot.to_string_lossy().replace('\'', "''");
        conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
    }
    let data_dir = paths::data_dir(&app)?;
    let dest = dest_path;
    tokio::task::spawn_blocking(move || {
        write_export_zip(&data_dir, &snapshot, std::path::Path::new(&dest))
    })
    .await
    .map_err(|e| Error::Other(format!("export task panicked: {e}")))?
}

/// Export the Markdown vault as plaintext `.md` files to `dest_dir` — the spec's "you
/// are never locked in" escape hatch (§3). Reads every vault file, decrypting any
/// encrypted ones with the in-session key, and writes a clean tree with no `.pmenc`
/// files, so the user can walk away with their notes in the open at any time. The vault
/// must be unlocked (the Markdown key has to be loaded). Returns the number of files
/// written. Unlike `export_all_data`, this is a *plaintext* escape hatch, not an
/// encrypted backup — it deliberately strips the at-rest protection.
#[tauri::command]
pub async fn export_plaintext_markdown(
    state: State<'_, AppState>,
    dest_dir: String,
) -> Result<usize> {
    let (vault, cipher) = state.markdown_io()?;
    let dest = std::path::PathBuf::from(dest_dir);
    tokio::task::spawn_blocking(move || ingest::export_plaintext(&vault, &cipher, &dest))
        .await
        .map_err(|e| Error::Other(format!("export task panicked: {e}")))?
}

/// Write the export archive: the DB snapshot as `pm.sqlite`, then the vault tree.
fn write_export_zip(
    data_dir: &std::path::Path,
    db_snapshot: &std::path::Path,
    dest: &std::path::Path,
) -> Result<()> {
    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("pm.sqlite", opts)?;
    let mut snap = std::fs::File::open(db_snapshot)?;
    std::io::copy(&mut snap, &mut zip)?;

    let vault = data_dir.join("vault");
    if vault.is_dir() {
        add_dir_to_zip(&mut zip, &vault, "vault", opts)?;
    }
    zip.finish()?;
    Ok(())
}

/// Recursively add `dir` to the archive under `prefix`, preserving relative paths.
fn add_dir_to_zip<W: std::io::Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &std::path::Path,
    prefix: &str,
    opts: zip::write::SimpleFileOptions,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let zip_path = format!("{prefix}/{}", name.to_string_lossy());
        if path.is_dir() {
            add_dir_to_zip(zip, &path, &zip_path, opts)?;
        } else {
            zip.start_file(zip_path, opts)?;
            let mut f = std::fs::File::open(&path)?;
            std::io::copy(&mut f, zip)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway encrypted store (also exercises the migration-in-transaction
    /// path in `db::open`).
    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, key).unwrap();
        (dir, conn)
    }

    #[test]
    fn save_models_caps_count_and_id_length_and_dedups() {
        let (_dir, conn) = temp_db();
        let mut models: Vec<String> = (0..100).map(|i| format!("vendor/model-{i}")).collect();
        models.push("vendor/model-0".into()); // duplicate of the first
        models.push("x".repeat(500)); // an absurdly long id
        save_models(&conn, CHAT_MODELS_KEY, models).unwrap();

        let stored = models_for(&conn, CHAT_MODELS_KEY).unwrap();
        assert!(stored.len() <= 50, "model count is capped");
        assert!(
            stored.iter().all(|m| m.chars().count() <= 200),
            "over-long id dropped"
        );
        assert_eq!(
            stored.iter().filter(|m| *m == "vendor/model-0").count(),
            1,
            "de-duped"
        );
    }
}
