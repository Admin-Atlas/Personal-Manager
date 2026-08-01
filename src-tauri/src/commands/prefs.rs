// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Structured preferences: the typed model that replaced the Learning-You blob.

use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::ingest;
use crate::llm_gateway::{self, Role};
use crate::{db, entities, preferences, AppState};

// --- structured preferences (§4.5 — the typed model that replaces the Learning-You blob) ---

/// One-time migration of the legacy free-text "Learning You" blob into structured preference
/// records, so accumulated profile content isn't lost. Idempotent: guarded by the
/// `preferences_migrated_at` flag and a no-op once it's set or the blob is empty. Background work —
/// runs on the background key and never holds the DB lock across the model call (rule #4),
/// best-effort. The legacy blob is kept ARCHIVED (never deleted). Records land `inferred` +
/// unconfirmed, awaiting the user's vouch in the Teach tab.
async fn migrate_preferences_once(app: AppHandle) -> Result<()> {
    let blob = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        if db::get_setting(&conn, preferences::MIGRATED_FLAG_KEY)?.is_some() {
            return Ok(()); // already migrated
        }
        let blob = db::get_setting(&conn, preferences::LEGACY_PROFILE_KEY)?.unwrap_or_default();
        if blob.trim().is_empty() {
            // Nothing to migrate — stamp the flag so we don't re-read an empty blob each launch.
            // Every fresh vault takes this branch on first boot; re-locking the state here (the old
            // `iso_now(&state)`) self-deadlocked the non-reentrant DB mutex and froze the whole app.
            let now = ingest::iso_now(&conn)?;
            db::set_setting(&conn, preferences::MIGRATED_FLAG_KEY, &now)?;
            return Ok(());
        }
        blob
    };

    // No provider yet → leave the blob untouched and unstamped; a later trigger retries.
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Ok(());
    };

    // A one-shot migration of the legacy blob: nothing else has written records yet, so there is
    // nothing to tell the distiller not to restate.
    let drafts = preferences::distill_blob(&app, &plan, &blob, &[]).await?;

    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let now = ingest::iso_now(&conn)?;
    let tx = conn.unchecked_transaction()?;
    for d in &drafts {
        // The blob has no entity to resolve a project against, so distilled records are global/
        // context (entity_id None) — see `preferences::distill_blob`.
        preferences::add_preference(
            &tx,
            &d.scope,
            None,
            d.condition.as_deref(),
            &d.value,
            preferences::SOURCE_INFERRED,
            preferences::inferred_seed_confidence(),
            false,
        )?;
    }
    db::set_setting(&tx, preferences::MIGRATED_FLAG_KEY, &now)?;
    tx.commit()?;
    Ok(())
}

/// Fire-and-forget the one-time preferences migration: background, idempotent, best-effort. Called
/// at startup and after a review commit (both guaranteed-unlocked moments).
pub fn spawn_preferences_migration(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = migrate_preferences_once(app).await {
            eprintln!("preferences: one-time blob migration skipped ({e})");
        }
    });
}

/// Every structured preference record, for the Teach tab.
#[tauri::command]
pub fn list_preferences(state: State<'_, AppState>) -> Result<Vec<preferences::Preference>> {
    let conn = state.conn()?;
    preferences::list_preferences(&conn)
}

/// Add a preference the user has explicitly stated (the structured form, or a confirmed
/// natural-language parse): stored as user-stated + confirmed. `entity_id` is required for a
/// project-scoped record.
#[tauri::command]
pub fn add_preference(
    state: State<'_, AppState>,
    scope: String,
    entity_id: Option<i64>,
    condition: Option<String>,
    value: String,
) -> Result<i64> {
    let conn = state.conn()?;
    preferences::add_preference(
        &conn,
        &scope,
        entity_id,
        condition.as_deref(),
        &value,
        preferences::SOURCE_USER,
        1.0,
        true,
    )
}

/// Edit a preference's scope / target / condition / value (also marks it user-confirmed).
#[tauri::command]
pub fn update_preference(
    state: State<'_, AppState>,
    id: i64,
    scope: String,
    entity_id: Option<i64>,
    condition: Option<String>,
    value: String,
) -> Result<()> {
    let conn = state.conn()?;
    preferences::update_preference(&conn, id, &scope, entity_id, condition.as_deref(), &value)
}

/// Mark an inferred preference as user-confirmed — the Teach-tab "✓ Confirm" that promotes a
/// migrated/blob-derived record to a trusted one.
#[tauri::command]
pub fn confirm_preference(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn()?;
    preferences::confirm_preference(&conn, id)
}

/// Delete a preference.
#[tauri::command]
pub fn delete_preference(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn()?;
    preferences::delete_preference(&conn, id)
}

/// Parse a free-text sentence into a draft preference (the "in your own words" path). One
/// background model call, then resolve any named project to its entity (read-only — no entity is
/// created for an unconfirmed parse; a name that doesn't resolve falls back to a global preference).
/// The frontend prefills the structured form with the result for the user to confirm before storing.
#[tauri::command]
pub async fn parse_preference_statement(
    app: AppHandle,
    text: String,
) -> Result<preferences::DraftPreference> {
    let projects = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        entities::canonical_project_names(&conn)?
    };
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };

    let mut draft = preferences::parse_statement(&app, &plan, &text, &projects).await?;

    if draft.scope == preferences::SCOPE_PROJECT {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let resolved = draft
            .project_name
            .as_deref()
            .and_then(|n| entities::resolve_project(&conn, n, false).ok().flatten());
        match resolved {
            Some(id) => {
                draft.entity_id = Some(id);
                draft.project_name = Some(entities::canonical_name(&conn, id)?);
            }
            None => {
                // Named a project that doesn't exist yet — keep it as a global preference rather
                // than silently inventing an entity the user hasn't confirmed.
                draft.scope = preferences::SCOPE_GLOBAL.to_string();
                draft.entity_id = None;
                draft.project_name = None;
            }
        }
    }
    Ok(draft)
}

/// Import a memory/preferences export pasted from another AI (ChatGPT / Gemini / Claude): distil it
/// into structured records and stage each as an UNCONFIRMED, `imported`-sourced preference the user
/// reviews and keeps in Teach -> Preferences (withheld from live prompts until kept). The pasted text
/// is untrusted DATA (the distil prompt hardens this). Returns how many NEW records were staged.
/// Distillation yields global/context records only, so there is no project to resolve — this is
/// general "how I like things", not PM-project-specific.
///
/// Re-importing the same export must stage nothing, and that takes two guards, because a second run
/// is a fresh model call that words the same facts differently. The prompt is TOLD what is already on
/// record so it can skip it; then every draft that survives is checked with `near_duplicate_exists`,
/// which compares meaning-bearing tokens rather than characters. The prompt hint catches the heavy
/// rewrites; the pure guard is the backstop, since the model's cooperation is never assumed.
#[tauri::command]
pub async fn import_ai_memory(app: AppHandle, text: String) -> Result<usize> {
    // Bound the paste so a huge export can't balloon the model call.
    const MAX_IMPORT_CHARS: usize = 20_000;
    let text: String = text.trim().chars().take(MAX_IMPORT_CHARS).collect();
    if text.is_empty() {
        return Err(Error::Other("paste your exported memory first".into()));
    }
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };
    // Read the known values and DROP the connection before awaiting — never hold the DB lock across
    // an .await (the model call is a network round-trip).
    let known = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        preferences::all_preference_values(&conn)?
    };
    let drafts = preferences::distill_blob(&app, &plan, &text, &known).await?;

    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let tx = conn.unchecked_transaction()?;
    let mut imported = 0usize;
    for d in &drafts {
        // distill_blob emits only global/context records (no project scope), so entity_id is None.
        // Checked against the transaction, so two drafts that paraphrase EACH OTHER within one
        // import also collapse to one — the second sees the first already inserted.
        if preferences::near_duplicate_exists(
            &tx,
            &d.scope,
            None,
            d.condition.as_deref(),
            &d.value,
        )? {
            continue;
        }
        preferences::add_preference(
            &tx,
            &d.scope,
            None,
            d.condition.as_deref(),
            &d.value,
            preferences::SOURCE_IMPORTED,
            preferences::inferred_seed_confidence(),
            false,
        )?;
        imported += 1;
    }
    tx.commit()?;
    Ok(imported)
}
