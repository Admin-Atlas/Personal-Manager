// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Settings-tab command surface: the user preferences the Settings UI reads and writes (the two
//! model roles + their auto-switch, retrieval knobs, search language, indexing speed, time zone, help
//! mode, and the soft app-lock), plus the model-list resolution those role settings feed. Split out of
//! `commands.rs` so new settings tabs land here rather than growing the monolith. Every command's wire
//! name is its bare `fn` identifier, unchanged by this move — the frontend contract is untouched.

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::error::{Error, Result};
use crate::{applock, db, AppState};

/// Fallback model when the user hasn't chosen one, for BOTH roles. Swappable in
/// Settings and stored as a plain string (spec §6 — never locked into a model).
///
/// Whatever this names MUST have a zero-data-retention endpoint: `chat_body` pins
/// `zdr: true` + `data_collection: "deny"` on every request, so a model with no
/// compliant endpoint fails closed and the default would brick chat out of the box.
/// It is checked at runtime: `openrouter::list_models` filters the picker against
/// `/api/v1/endpoints/zdr`, so a non-compliant id can't be chosen from the list — but
/// this const bypasses the picker, so verify it against that endpoint when changing it.
/// Ling-2.6-flash qualifies via Novita (training/retention both off) and costs
/// ~$0.01/$0.03 per Mtok against Sonnet 4.6's $3/$15, which the background role
/// (titles, summaries, sorting proposals) spends on unattended.
pub(crate) const DEFAULT_MODEL: &str = "inclusionai/ling-2.6-flash";

/// Settings keys for the two model roles. Each holds a JSON array of model ids
/// (ordered, first = primary); the `*_AUTO_SWITCH` keys hold "true"/"false".
pub(crate) const CHAT_MODELS_KEY: &str = "chat_models";
pub(crate) const BACKGROUND_MODELS_KEY: &str = "background_models";
pub(crate) const CHAT_AUTO_SWITCH_KEY: &str = "chat_auto_switch";
pub(crate) const BACKGROUND_AUTO_SWITCH_KEY: &str = "background_auto_switch";

/// The user's IANA time-zone name (e.g. "America/New_York"), supplied by the
/// frontend via `Intl.DateTimeFormat().resolvedOptions().timeZone`. Empty/unset →
/// the backend reasons in UTC (see `resolve_zone`).
pub(crate) const TIME_ZONE_KEY: &str = "time_zone";

/// Whether the optional biometric app-lock is on ("true"/"false", default off). A soft
/// UI gate only — it never gates the DB key (see `applock`). Lives in `settings`
/// (security preference → backend), not localStorage.
const APP_LOCK_ENABLED_KEY: &str = "app_lock_enabled";

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
    /// Indexing speed: "fast" (default, max throughput) or "gentle" (paced so a low-end machine
    /// stays usable while indexing runs in the background).
    pub indexing_speed: String,
    /// Retrieval depth `k` — how many fused candidates reach the reranker (card 7H). The lever the
    /// in-chat Retrieval-explain panel tunes; default [`retrieval::DEFAULT_TOP_K`], stateless.
    pub retrieval_k: usize,
    /// The EFFECTIVE confidence-gate threshold — the minimum top rerank score for PM to trust its
    /// grounding — or `None` when a dev has disabled the gate. ON by default (card #402); the value and
    /// on/off are tuned by the Developer-mode control. See [`db::retrieval_confidence_threshold`].
    pub retrieval_confidence_threshold: Option<f32>,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    let conn = state.conn()?;
    Ok(Settings {
        chat_models: models_for(&conn, CHAT_MODELS_KEY)?,
        background_models: models_for(&conn, BACKGROUND_MODELS_KEY)?,
        chat_auto_switch: db::get_bool(&conn, CHAT_AUTO_SWITCH_KEY, false)?,
        background_auto_switch: db::get_bool(&conn, BACKGROUND_AUTO_SWITCH_KEY, false)?,
        help_mode: db::get_bool(&conn, "help_mode", false)?,
        time_zone: db::get_setting(&conn, TIME_ZONE_KEY)?.unwrap_or_default(),
        reranking: db::reranking_enabled(&conn)?,
        indexing_speed: db::get_setting(&conn, db::INDEXING_SPEED_KEY)?
            .unwrap_or_else(|| "fast".into()),
        retrieval_k: db::retrieval_k(&conn),
        retrieval_confidence_threshold: db::retrieval_confidence_threshold(&conn),
    })
}

/// Set the confidence-gate threshold (card #402): `Some(n)` turns the gate ON at `n`, `None` turns it
/// OFF. Below the threshold, PM is told the sources are weak and hedges instead of fabricating. ON by
/// default when unset. Stateless — lands on the next query, no Rebuild. Calibrated in Developer mode.
#[tauri::command]
pub fn set_retrieval_confidence_threshold(
    state: State<'_, AppState>,
    threshold: Option<f64>,
) -> Result<()> {
    let conn = state.conn()?;
    db::set_retrieval_confidence_threshold(&conn, threshold.map(|t| t as f32))
}

/// Set the indexing-speed preference. "gentle" paces indexing (Drive sync + file import) so a low-end
/// machine stays usable while it works in the background; "fast" runs at full throughput. Anything
/// else is treated as "fast".
#[tauri::command]
pub fn set_indexing_speed(state: State<'_, AppState>, speed: String) -> Result<()> {
    let value = if speed == "gentle" { "gentle" } else { "fast" };
    let conn = state.conn()?;
    db::set_setting(&conn, db::INDEXING_SPEED_KEY, value)
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
    db::set_bool(&conn, CHAT_AUTO_SWITCH_KEY, enabled)
}

#[tauri::command]
pub fn set_background_auto_switch(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_bool(&conn, BACKGROUND_AUTO_SWITCH_KEY, enabled)
}

/// Toggle the UI help/explain mode (Step 4b). Stored in `settings` so it persists.
#[tauri::command]
pub fn set_help_mode(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_bool(&conn, "help_mode", enabled)
}

/// Turn query-time reranking on or off (a cross-encoder re-scores search hits). Stateless — never
/// triggers a Rebuild — so this just flips the setting; the effect lands on the next query.
#[tauri::command]
pub fn set_reranking(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.conn()?;
    db::set_reranking(&conn, enabled)
}

/// Set the retrieval depth `k` — the candidate pool that reaches the reranker (card 7H). The user
/// commits this from the in-chat Retrieval-explain panel ("Use this depth for retrieval"). Clamped
/// in `db::set_retrieval_k`; stateless, so the effect lands on the next chat turn's retrieval.
#[tauri::command]
pub fn set_retrieval_k(state: State<'_, AppState>, k: usize) -> Result<()> {
    let conn = state.conn()?;
    db::set_retrieval_k(&conn, k)
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

/// The webview-owned UI preference blobs — the ONLY `settings` keys the webview may read or write.
/// Holding the list here (rather than letting the webview touch any key) keeps schema-critical and
/// sensitive rows out of reach: the webview must never rewrite e.g. `embedding_dim` (silently
/// corrupting the index) NOR read e.g. the archived `learning_profile`, cursors, or model lists
/// (I-04 — the read side was previously ungated). Read and write share one allowlist because the
/// readable set is exactly the webview's own blobs.
const WEBVIEW_PREFS: &[&str] = &["appearance", "pinboard", "dev_mode", "map", "project_ui"];

/// Read a UI preference blob the webview previously stored (theme axes, pinboard
/// layout). These live in the encrypted `settings` table — not the webview's
/// `localStorage` — so they travel with the data folder when it's backed up or
/// moved to another machine. Returns `None` when nothing is stored yet. Gated on
/// [`WEBVIEW_PREFS`] (I-04) so a compromised webview can't read arbitrary settings rows.
#[tauri::command]
pub fn get_pref(state: State<'_, AppState>, key: String) -> Result<Option<String>> {
    if !WEBVIEW_PREFS.contains(&key.as_str()) {
        return Err(Error::Other(format!("preference '{key}' is not readable")));
    }
    let conn = state.conn()?;
    db::get_setting(&conn, &key)
}

/// Persist a UI preference blob (see [`get_pref`]). Restricted to [`WEBVIEW_PREFS`]
/// so the webview can only touch presentation state, never schema-critical keys.
#[tauri::command]
pub fn set_pref(state: State<'_, AppState>, key: String, value: String) -> Result<()> {
    if !WEBVIEW_PREFS.contains(&key.as_str()) {
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
        db::get_bool(&conn, APP_LOCK_ENABLED_KEY, false)?
    };
    let verified = state
        .app_unlocked
        .load(std::sync::atomic::Ordering::Relaxed);
    let available = applock::available();
    Ok(AppLockStatus {
        enabled,
        available,
        locked: applock::should_lock(enabled, available, verified),
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
    db::set_bool(&conn, APP_LOCK_ENABLED_KEY, enabled)
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
pub(crate) fn effective_models(
    conn: &Connection,
    models_key: &str,
    auto_key: &str,
) -> Result<Vec<String>> {
    let models = models_for(conn, models_key)?;
    let auto = db::get_bool(conn, auto_key, false)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway encrypted store, mirroring `commands`'s test fixture.
    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, key).unwrap();
        (dir, conn)
    }

    #[test]
    fn webview_prefs_allowlist_excludes_sensitive_settings() {
        // I-04: get_pref/set_pref gate on this list, so a compromised webview can read or write ONLY
        // its own UI blobs. Lock it: the five UI keys are in, and schema-critical / sensitive rows are
        // out — a future edit that accidentally adds one trips this test.
        for ui in ["appearance", "pinboard", "dev_mode", "map", "project_ui"] {
            assert!(WEBVIEW_PREFS.contains(&ui), "{ui} should be webview-owned");
        }
        for sensitive in [
            "embedding_dim",
            "embedding_model",
            "learning_profile",
            "last_backup_at",
            "backup_gdrive_account",
        ] {
            assert!(
                !WEBVIEW_PREFS.contains(&sensitive),
                "{sensitive} must never be readable/writable from the webview"
            );
        }
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
