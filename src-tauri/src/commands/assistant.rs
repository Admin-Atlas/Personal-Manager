// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The personal assistant: project milestones, the tray and briefing windows, the daily
//! briefing, flag resolution and the focus box.

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::calendar;
use crate::error::{Error, Result};
use crate::ingest;
use crate::llm_gateway::{self, Role};
use crate::milestones::{self, Milestone};
use crate::project_activity;
use crate::projects;
use crate::tray;
use crate::{briefing, clock, entities, flags, preferences, AppState};

use super::shared::resolve_zone;
use super::spend::log_usage;

// --- personal assistant: project milestones (multi-deadline — card 7) ---
//
// A project carries zero or more dated milestones (each its own stable-id row); the focus view's
// single status is derived from the nearest unmet one. PM-native milestones have a user-set date;
// calendar-linked ones (event_uid set) sync their date from the read-only calendar mirror. All quick
// synchronous DB work — no model calls, so the lock is held only briefly (rule #4).

/// Bump the activity date of the project owning milestone `id` — editing a milestone counts
/// as engaging with its project. Best-effort: an unknown id is a no-op.
fn touch_milestone_project(conn: &Connection, id: i64) -> Result<()> {
    if let Some(project) = milestones::project_of(conn, id)? {
        projects::touch(conn, &project)?;
        project_activity::record(conn, &project, project_activity::Kind::Milestone, Some(id));
    }
    Ok(())
}

/// One project's milestones, resolved (calendar-linked dates synced) and date-ordered.
#[tauri::command]
pub fn list_milestones(state: State<'_, AppState>, project: String) -> Result<Vec<Milestone>> {
    let conn = state.conn()?;
    let today = clock::today_sql_in(resolve_zone(&conn));
    milestones::list_for_project(&conn, project.trim(), &today)
}

/// Every project's milestones, resolved — read-only, for the calendar overlay (each carries its
/// `project_name` for click-to-open).
#[tauri::command]
pub fn list_all_milestones(state: State<'_, AppState>) -> Result<Vec<Milestone>> {
    let conn = state.conn()?;
    let today = clock::today_sql_in(resolve_zone(&conn));
    milestones::list_all(&conn, &today)
}

/// Add a milestone to a project (creating the project's metadata row if needed). A non-empty
/// `event_uid` makes it calendar-linked. Returns the new stable id.
#[tauri::command]
pub fn add_milestone(
    state: State<'_, AppState>,
    project: String,
    label: String,
    due_date: Option<String>,
    event_uid: Option<String>,
) -> Result<i64> {
    let project = project.trim();
    if project.is_empty() {
        return Err(Error::Other("project name is empty".into()));
    }
    let conn = state.conn()?;
    let id = milestones::add(&conn, project, &label, due_date, event_uid)?;
    projects::touch(&conn, project)?;
    briefing::nudge(&state);
    project_activity::record(&conn, project, project_activity::Kind::Milestone, Some(id));
    Ok(id)
}

/// Edit a milestone's label and (for a PM-native milestone) its date. A calendar-linked
/// milestone keeps its calendar-owned date regardless.
#[tauri::command]
pub fn update_milestone(
    state: State<'_, AppState>,
    id: i64,
    label: String,
    due_date: Option<String>,
) -> Result<()> {
    let conn = state.conn()?;
    milestones::update(&conn, id, &label, due_date)?;
    touch_milestone_project(&conn, id)?;
    briefing::nudge(&state);
    Ok(())
}

/// Link a milestone to a calendar event (`event_uid` Some, `cached_date` seeds the offline cache)
/// or unlink it (None — the date becomes editable again).
#[tauri::command]
pub fn set_milestone_event(
    state: State<'_, AppState>,
    id: i64,
    event_uid: Option<String>,
    cached_date: Option<String>,
) -> Result<()> {
    let conn = state.conn()?;
    milestones::set_event(&conn, id, event_uid, cached_date)?;
    touch_milestone_project(&conn, id)?;
    Ok(())
}

/// Mark a milestone met or unmet.
#[tauri::command]
pub fn set_milestone_state(state: State<'_, AppState>, id: i64, met: bool) -> Result<()> {
    let conn = state.conn()?;
    milestones::set_state(&conn, id, met)?;
    // Un-marking a milestone done is the "I made a mistake" undo: clear any flag the user asserted done
    // on it, so the next briefing refresh's detection can surface the deadline again. A completion vouched
    // done is otherwise a protected record the re-scan can't re-open. Ticking it done needs no such step —
    // detection prunes the now-met milestone's active flag on its own.
    if !met {
        flags::reopen_milestone(&conn, id)?;
    }
    touch_milestone_project(&conn, id)?;
    briefing::nudge(&state);
    Ok(())
}

/// Set a milestone's progress status (v42) — the four-level counterpart to the met/unmet tick-box.
/// `milestones::set_status` carries `state` along, so this goes through exactly the same
/// flag-reopening step `set_milestone_state` does: moving OFF `done` is the same "I made a mistake"
/// undo as un-ticking the box, and must clear a user-asserted completion so detection can surface
/// the deadline again. Skipping that here would make the two controls behave differently on the
/// same transition.
#[tauri::command]
pub fn set_milestone_status(state: State<'_, AppState>, id: i64, status: String) -> Result<()> {
    let conn = state.conn()?;
    milestones::set_status(&conn, id, &status)?;
    if status != milestones::STATUS_DONE {
        flags::reopen_milestone(&conn, id)?;
    }
    touch_milestone_project(&conn, id)?;
    briefing::nudge(&state);
    Ok(())
}

/// Delete a milestone by id.
#[tauri::command]
pub fn delete_milestone(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn()?;
    // Resolve the owning project before the row is gone, then bump its activity.
    let project = milestones::project_of(&conn, id)?;
    milestones::remove(&conn, id)?;
    briefing::nudge(&state);
    if let Some(project) = project {
        projects::touch(&conn, &project)?;
        // The row is gone, but `source_ref` is a plain pointer (not an FK), so the deleted
        // milestone's id is still a valid historical reference for the observation.
        project_activity::record(&conn, &project, project_activity::Kind::Milestone, Some(id));
    }
    Ok(())
}

/// Persist a new ordering of a project's milestones (ids in display order).
#[tauri::command]
pub fn reorder_milestones(
    state: State<'_, AppState>,
    project: String,
    ordered_ids: Vec<i64>,
) -> Result<()> {
    let conn = state.conn()?;
    let project = project.trim();
    milestones::reorder(&conn, project, &ordered_ids)?;
    projects::touch(&conn, project)?;
    // A bulk reorder has no single milestone id, so the observation is project-level (source_ref None).
    project_activity::record(&conn, project, project_activity::Kind::Milestone, None);
    Ok(())
}

// --- daily briefing (Step 7, spec §4 P1) ---

/// The stored "here's your picture today" briefing + whether it's due a refresh, for
/// the focus view. Read-only — no model call, so it's cheap on every mount.
/// Whether the tray / menu-bar icon is switched on. Backend-owned because Rust reads it at boot
/// (to decide the icon's visibility, and whether closing the main window quits or hides).
#[tauri::command]
pub fn get_tray_enabled(app: AppHandle) -> bool {
    tray::tray_enabled(&app)
}

/// Switch the tray icon on or off, persisting the choice. Also hides the briefing window when the
/// tray goes off, so no floating panel is left with no way back to it.
#[tauri::command]
pub fn set_tray_enabled(app: AppHandle, enabled: bool) -> Result<()> {
    tray::set_tray_enabled(&app, enabled)
}

/// Put the always-on-top briefing window into an explicit state — what the Settings control wants,
/// since "Floating briefing = inside PM" must HIDE the OS window rather than flip it.
#[tauri::command]
pub fn set_briefing_window_visible(app: AppHandle, visible: bool) -> Result<()> {
    tray::set_briefing_window_visible(&app, visible)
}

/// Dismiss the always-on-top briefing window from its own ✕. Hides it and emits `briefing://closed`
/// so the main window puts the "Floating briefing" setting back to Off. The briefing webview holds no
/// capability entry, so it can neither hide itself nor listen — Rust owns both halves.
#[tauri::command]
pub fn close_briefing_window(app: AppHandle) -> Result<()> {
    tray::close_briefing_window(&app)
}

/// Destroy the briefing window before "Remove PM data" clears the webview store.
///
/// Distinct from [`close_briefing_window`], which only HIDES it: a hidden webview still runs, and
/// this one is a second JS context persisting theme preferences into the same origin store the main
/// window is about to empty. It cannot be signalled — the briefing webview holds no capability entry
/// — so the erase removes it rather than asking it to be quiet.
#[tauri::command]
pub fn destroy_briefing_window(app: AppHandle) -> Result<()> {
    tray::destroy_briefing_window(&app)
}

/// Bring the main window to the front — the briefing window's "Open PM" button.
///
/// It has to be a PM command rather than `getCurrentWindow()`/`getAllWebviewWindows()` from
/// `@tauri-apps/api/window`: those are `plugin:`-prefixed and ACL-gated, and the briefing webview's
/// capability grants only dragging and event listen/unlisten. A plugin call from there would fail at
/// runtime with nothing in `just check` catching it. PM's own commands are not ACL-gated.
#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<()> {
    tray::show_main_window(&app);
    Ok(())
}

#[tauri::command]
pub fn get_daily_briefing(state: State<'_, AppState>) -> Result<briefing::DailyBriefing> {
    let conn = state.conn()?;
    briefing::get_briefing(&conn)
}

/// Regenerate the daily briefing unconditionally — the "Refresh" button in every surface that
/// shows it. Returns the new briefing.
#[tauri::command]
pub async fn refresh_daily_briefing(app: AppHandle) -> Result<briefing::DailyBriefing> {
    run_briefing_refresh(&app, true).await
}

/// Regenerate the briefing ONLY if the facts it was written from have actually moved — the launch
/// check, and what the frontend calls after an edit that feeds it (a milestone ticked, a flag
/// resolved). Returns the current briefing either way.
///
/// This is the entry every automatic trigger uses, and the reason they can be frequent: the whole
/// check is a DB pass and a fingerprint comparison, so an hour (or a calendar sync) in which
/// nothing actually changed costs no tokens at all.
#[tauri::command]
pub async fn sync_daily_briefing(app: AppHandle) -> Result<briefing::DailyBriefing> {
    run_briefing_refresh(&app, false).await
}

/// [`sync_daily_briefing`] for the background scheduler, which holds a borrowed `AppHandle` rather
/// than a command's owned one.
pub(crate) async fn refresh_briefing_auto(app: &AppHandle) -> Result<briefing::DailyBriefing> {
    run_briefing_refresh(app, false).await
}

/// Regenerate the daily briefing from the current focus-view state. Background work: runs on the
/// background API key, never holds the DB lock across the model call (rule #4), and is a no-op
/// (returns the stored value) when there's nothing to summarise.
///
/// `force` separates the user asking from the app checking. Forced, it always calls the model and
/// surfaces a missing-provider error. Unforced, it calls the model only when
/// [`briefing::auto_refresh_due`] says the facts moved, and stays quiet when there's no provider —
/// an hourly scheduler must not manufacture an error the user never triggered.
async fn run_briefing_refresh(app: &AppHandle, force: bool) -> Result<briefing::DailyBriefing> {
    let state = app.state::<AppState>();

    // SINGLE-FLIGHT. The briefing renders in up to three places at once (Focus card, sidebar
    // panel, always-on-top window) on top of three background triggers, and each webview's own
    // guard is blind to the others — so overlap has to be stopped here or two model calls race on
    // the stored trio and can leave an OLDER body wearing a NEWER timestamp.
    //
    // A second caller waits for the running generation, then decides whether it still has work.
    // Folding unconditionally would be wrong: an automatic check that regenerates NOTHING (the
    // common case) would swallow a Refresh the user clicked while it ran, and the click would look
    // dead. So the waiter folds only when the wait actually produced a newer briefing — which
    // covers "both windows clicked Refresh" with a single model call, while an explicit Refresh
    // that waited on a no-op check goes on to do the work.
    let _guard = match state.briefing_refresh.try_lock() {
        Ok(g) => g,
        Err(_) => {
            // Read where the briefing stands BEFORE blocking (and drop the guard first — no DB
            // lock may cross an `.await`, rule #4).
            let before = {
                let conn = state.conn()?;
                briefing::get_briefing(&conn)?.updated_at
            };
            let guard = state.briefing_refresh.lock().await;
            let landed = {
                let conn = state.conn()?;
                briefing::get_briefing(&conn)?
            };
            if !force || landed.updated_at != before {
                return Ok(landed);
            }
            guard
        }
    };

    let Some(plan) = llm_gateway::resolve(app, Role::Background)? else {
        if force {
            return Err(Error::Other(llm_gateway::no_provider_message()));
        }
        let conn = state.conn()?;
        return briefing::get_briefing(&conn);
    };

    let (snapshot, profile) = {
        let conn = state.conn()?;
        let zone = resolve_zone(&conn);
        let now = clock::now_local_iso(zone);
        let today = clock::today_sql_in(zone);
        let projects = projects::list_overviews(&conn, &today)?;
        let events = calendar::list_upcoming(&conn, briefing::BRIEFING_AGENDA_DAYS, &today)?;
        // Evaluate the structured flag layer BEFORE rendering (card 9): reconcile the stored flag
        // set to the current projects + calendar, then render the ACTIVE (unresolved) flags as the
        // briefing's facts. Best-effort — a detection hiccup must never fail the briefing, so a
        // failure just leaves the prior flag set in place and briefs from it.
        if let Err(e) = flags::detect_and_store(&conn, &projects, &events, &today, zone) {
            eprintln!("flag detection skipped during briefing refresh: {e}");
        }
        let active = flags::list_active(&conn, None)?;
        // Resolved prepare-ahead flags let a still-active happening-today render "you're prepared —
        // file's here" (card 9, decision 3) instead of the line simply disappearing on resolution.
        let resolved_prep = flags::list_resolved(&conn, flags::TYPE_PREPARE_AHEAD)?;
        let snapshot =
            briefing::build_flag_snapshot(&active, &resolved_prep, &projects, &events, &now, zone);
        // The briefing is the whole-picture view, so global + context preferences shape its voice.
        let profile = preferences::preferences_preamble(&conn, preferences::PrefContext::global())?;
        (snapshot, profile)
    };

    // Nothing to brief on yet — leave any prior briefing in place.
    let Some(snapshot) = snapshot else {
        let conn = state.conn()?;
        return briefing::get_briefing(&conn);
    };

    // The cost gate. Everything above this line is DB work; everything below spends a model call.
    let fingerprint = briefing::snapshot_fingerprint(&snapshot);
    if !force {
        let conn = state.conn()?;
        let stored = briefing::get_briefing(&conn)?;
        if !briefing::auto_refresh_due(
            briefing::stored_fingerprint(&conn)?.as_deref(),
            &fingerprint,
            stored.stale,
        ) {
            return Ok(stored);
        }
    }

    let (text, usage, served, meta) =
        briefing::generate(app, &plan, &snapshot, profile.as_deref()).await?;

    let fresh = {
        let conn = state.conn()?;
        let now = ingest::iso_now(&conn)?;
        log_usage(
            &conn,
            "background",
            served.as_deref().or(Some(plan.primary_model_id())),
            &usage,
            &meta,
        );
        briefing::save_briefing(&conn, &text, &now, &fingerprint)?;
        briefing::get_briefing(&conn)?
    };
    // Tell every window, not only the caller: a scheduled regeneration has no caller at all, and a
    // Refresh clicked in one surface should land in the others rather than leaving them stale.
    let _ = app.emit(briefing::BRIEFING_UPDATED_EVENT, ());
    Ok(fresh)
}

/// Mark a flag done — a deliberate user assertion (card 9). Assertion outranks detection, so the flag
/// leaves the active set the briefing/chat render (resolution is a *filter*, not a text edit) and a
/// later re-detection can't reopen it. When the user names the satisfying artifact, its rename-stable
/// `source_id` and current open URL are recorded, so a downstream `happening-today` on the same anchor
/// can surface "you're prepared — file's here" (decision 3). Returns the resolved flag.
#[tauri::command]
pub fn resolve_flag(
    state: State<'_, AppState>,
    flag_id: i64,
    artifact_source_id: Option<String>,
) -> Result<flags::Flag> {
    let conn = state.conn()?;
    // The artifact's current open URL is display-only (it moves on rename, whereas source_id is the
    // rename-survives identity). Looked up here, then handed to `assert_done` purely as stored state.
    let artifact_url: Option<String> = match artifact_source_id.as_deref() {
        Some(sid) => conn
            .query_row(
                "SELECT external_ref FROM documents WHERE source_id = ?1",
                params![sid],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten(),
        None => None,
    };
    // Assert the flag done AND write the "done" through to its milestone in one transaction — a
    // milestone-anchored flag and its milestone are one fact (card 9 centralisation), so this keeps the
    // project view, the governing status and future detection in step with the briefing. `assert_done`
    // returns the milestone it ticked (if any), so we bump that project's activity like a direct
    // milestone edit does.
    let (flag, milestone_id) = flags::assert_done(
        &conn,
        flag_id,
        artifact_source_id.as_deref(),
        artifact_url.as_deref(),
    )?;
    if let Some(mid) = milestone_id {
        touch_milestone_project(&conn, mid)?;
    }
    // A resolved flag leaves the active set the briefing renders, so the briefing that still names
    // it is now wrong about the user's day — exactly the case worth re-briefing for.
    briefing::nudge(&state);
    Ok(flag)
}

/// Classify one line the user typed in the polymorphic focus box (card 9, decisions 6–7) and route it:
/// mark a visible flag done, capture a durable preference, ask a (flag-grounded) question, or edit a
/// project. ONE background classification call over the CLOSED candidate set of active flags; the
/// frontend then acts on the returned route — `resolve`/`prefer` on the user's confirm (those are
/// writes), `ask`/`edit` navigate. This command itself never writes flag/preference state; a `prefer`
/// route only carries the draft the confirm step stores. The user's line is their own request, but the
/// ingested titles in the candidate list stay DATA (rule #6). Background key, no DB lock across the
/// await (rule #4). Returns [`flags::FocusRoute::Unclear`] for blank input or an unreadable reply.
#[tauri::command]
pub async fn route_focus_input(app: AppHandle, text: String) -> Result<flags::FocusRoute> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(flags::FocusRoute::Unclear);
    }
    let (candidates, project_names) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let zone = resolve_zone(&conn);
        let today = clock::today_sql_in(zone);
        let candidates = flags::describe_active(&conn, &today, zone)?;
        let project_names = entities::canonical_project_names(&conn)?;
        (candidates, project_names)
    };
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };

    let messages = flags::render_route_request(&text, &candidates, &project_names);
    let llm_gateway::LlmOutcome { completion, meta } =
        llm_gateway::complete(&app, &plan, &messages, false).await?;
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        log_usage(
            &conn,
            "background",
            completion
                .model
                .as_deref()
                .or(Some(plan.primary_model_id())),
            &completion.usage,
            &meta,
        );
    }
    // This router's output is an action — `resolve` crosses a flag off — so a reply that stopped
    // mid-word must not be read as a decision. `parse_route` is defensive and would fall through to
    // its own default, but "the model did not finish" and "the model chose the default" are not the
    // same thing and only one of them should act.
    let route = match completion.usable_text() {
        Some(text_out) => flags::parse_route(text_out, &candidates, &text),
        None => flags::FocusRoute::Unclear,
    };

    // Resolve the entity for a project-scoped preference draft (read-only — never invent an entity the
    // user hasn't confirmed; a name that doesn't resolve falls back to a global preference, exactly like
    // `parse_preference_statement`). Other routes pass straight through.
    if let flags::FocusRoute::Prefer { draft } = &route {
        if draft.scope == preferences::SCOPE_PROJECT {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            let resolved = draft
                .project_name
                .as_deref()
                .and_then(|n| entities::resolve_project(&conn, n, false).ok().flatten());
            let mut draft = draft.clone();
            match resolved {
                Some(id) => {
                    draft.entity_id = Some(id);
                    draft.project_name = Some(entities::canonical_name(&conn, id)?);
                }
                None => {
                    draft.scope = preferences::SCOPE_GLOBAL.to_string();
                    draft.entity_id = None;
                    draft.project_name = None;
                }
            }
            return Ok(flags::FocusRoute::Prefer { draft });
        }
    }
    Ok(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared milestone-recency helper (used by update / set_event / set_state) appends a
    /// `kind='milestone'` activity observation for the owning project, ref = the milestone id
    /// (Stage-3 activity log). The direct-touch commands (add / delete / reorder) emit the same way.
    #[test]
    fn touch_milestone_project_logs_a_milestone_observation() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // Adding a milestone also mints its project, giving us a real id to touch.
        let id = crate::milestones::add(&conn, "Atlas", "pitch", Some("2026-07-01".into()), None)
            .unwrap();

        touch_milestone_project(&conn, id).unwrap();

        let (project, kind, source_ref): (String, String, Option<i64>) = conn
            .query_row(
                "SELECT project, kind, source_ref FROM project_activity",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (project.as_str(), kind.as_str(), source_ref),
            ("Atlas", "milestone", Some(id))
        );
    }
}
