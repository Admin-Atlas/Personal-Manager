// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Changing Google events from PM (#884): open an event for editing, save the fields the user
//! changed, delete it. The decisions are the pure core's (`crate::calendar_write`); this module does
//! the I/O around them, in a fixed order every write follows:
//!
//! 1. the main window and the vault owner only (`require_main_window`, `require_owner`);
//! 2. a short DB read: which Google calendar and event the mirror row stands for, and the calendar's
//!    facts. The webview only ever names PM's own row ids and the session ids PM minted;
//! 3. a fresh GET from Google, with no lock held, and the gate again on that copy (the mirror can be
//!    stale, and clips text on purpose);
//! 4. the save rules on the fresh copy: nothing Google changed meanwhile is overwritten (R2);
//! 5. one PATCH or DELETE guarded by the etag, through the private [`google_io`];
//! 6. Google's reply into the mirror (never the draft), remembered so a sync that started earlier
//!    can't put the old version back, and the briefing told to look again.
//!
//! Every outcome comes back as data (`WriteOutcome`), so the editor keeps the draft and explains.
//! Nothing here is reachable from a model: no model call has tools, the commands refuse any window
//! but the main one, and `scripts/check-calendar-write-fence.mjs` keeps the command names and
//! [`google_io`] out of the chat and briefing paths.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::calendar::{self, CalendarEvent, CalendarFacts};
use crate::calendar_editing::{self, EditingStatus};
use crate::calendar_write::classify::Verdict;
use crate::calendar_write::dto::{
    DeleteSettled, DeleteStart, EditLoad, EventForEdit, EventPatchDraft, FieldPermissions,
    HeldDeleteInfo, Notify, ReadOnlyReason, SeenSummary, ShowAs, TimeDraft, Visibility,
    WriteOutcome,
};
use crate::calendar_write::gate::{self, EditFacts};
use crate::calendar_write::patch::{self, DeleteCheck, UpdateCheck};
use crate::calendar_write::reconcile::{RecentWrites, Stamp};
use crate::calendar_write::{plan, time};
use crate::error::{Error, Result};
use crate::{briefing, google, secrets, AppState};

use super::shared::require_main_window;
use super::vaults::require_vault_owner;

/// What editing keeps in memory, in [`AppState`]. Lock order everywhere: the DB, then `recent`.
#[derive(Default)]
pub struct CalendarEditState {
    /// Saves of the last ten minutes, laid over each sync's fetch (rule R4).
    pub(crate) recent: Mutex<RecentWrites>,
    /// Open editors, by the session id PM minted: the copy each was opened on.
    sessions: Mutex<HashMap<String, EditSession>>,
    /// Mirror rows with a write on its way to Google: a second write to the same event is Busy.
    in_flight: Mutex<HashSet<String>>,
    /// Deletes waiting out their Undo window.
    held_deletes: HeldDeletes,
    /// One calendar sync at a time, so a fetch that started earlier can never finish after a later
    /// one has settled a save (plan A23). A second caller waits, then runs.
    pub(crate) sync_lock: tokio::sync::Mutex<()>,
}

/// An open editor: which event, and the copy of it the user saw.
#[derive(Clone)]
struct EditSession {
    target: Target,
    base: Value,
    opened: Instant,
}

/// How long an editor may stay open before its session lapses.
const SESSION_TTL: Duration = Duration::from_secs(8 * 3600);

/// Where a mirror row lives at Google, resolved from the DB alone.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    row_id: String,
    calendar_id: String,
    remote_calendar: String,
    remote_event: String,
    email: String,
    token_key: String,
    facts: CalendarFacts,
}

/// Whether an id stays one path segment of its own in a request URL. `Url::path_segments_mut` drops
/// a "." or ".." segment (and strips tabs and newlines first, so ".\t." becomes ".."), which would
/// send the write to the calendar's event list, or the calendar itself, instead of the event. Google's
/// ids never look like that; a corrupt row isn't written through.
fn is_plain_segment(s: &str) -> bool {
    !s.is_empty() && !matches!(s, "." | "..") && !s.chars().any(|c| c.is_ascii_control())
}

/// Resolve a mirror row to its Google calendar and event. A row from an iCal feed or an Outlook
/// calendar isn't Google OAuth, so it comes back as that reason rather than a target.
fn resolve_target(
    conn: &Connection,
    row_id: &str,
) -> Result<std::result::Result<Target, ReadOnlyReason>> {
    let Some(row) = calendar::event_by_id(conn, row_id)? else {
        return Err(Error::Other(
            "That event is no longer in PM's calendar. Refresh and try again.".into(),
        ));
    };
    let Some(cal) = calendar::calendar_by_id(conn, &row.calendar_id)? else {
        return Err(Error::Other(
            "That event's calendar is no longer connected.".into(),
        ));
    };
    let email = match (cal.provider.as_str(), cal.source_id.strip_prefix("gcal:")) {
        ("google", Some(email)) => email.to_string(),
        _ => return Ok(Err(ReadOnlyReason::NotGoogle)),
    };
    let Some(remote_event) = row
        .id
        .strip_prefix(&format!("{}:", row.calendar_id))
        .filter(|e| is_plain_segment(e))
    else {
        return Ok(Err(ReadOnlyReason::NotGoogle));
    };
    let remote_calendar = cal.remote_id.clone().unwrap_or_else(|| cal.id.clone());
    if !is_plain_segment(&remote_calendar) {
        return Ok(Err(ReadOnlyReason::NotGoogle));
    }
    let Some(token_key) = secrets::token_key_for("google", "calendar", &email) else {
        return Ok(Err(ReadOnlyReason::NotGoogle));
    };
    Ok(Ok(Target {
        row_id: row.id.clone(),
        calendar_id: row.calendar_id.clone(),
        remote_calendar,
        remote_event: remote_event.to_string(),
        email,
        token_key,
        facts: cal.facts,
    }))
}

/// The account's editing status: its stored choice and its calendar token's scope (a keychain read,
/// so called with no DB lock held).
fn editing_status(choice: Option<calendar_editing::Choice>, token_key: &str) -> EditingStatus {
    let scope = choice
        .and_then(|_| google::token_scope(token_key).ok())
        .flatten();
    calendar_editing::status(choice, scope.as_deref())
}

/// The gate's facts from Google's fresh copy and the calendar's facts.
fn facts_from_fresh<'a>(
    fresh: &'a Value,
    calendar: &'a CalendarFacts,
    editing: EditingStatus,
) -> EditFacts<'a> {
    let text = |key: &str| fresh.get(key).and_then(Value::as_str);
    EditFacts {
        google: true,
        editing,
        access_role: calendar.access_role.as_deref(),
        event_type: text("eventType"),
        organizer_self: fresh
            .get("organizer")
            .and_then(|o| o.get("self"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        locked: fresh
            .get("locked")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        visibility: text("visibility"),
        recurring: fresh.get("recurringEventId").is_some() || fresh.get("recurrence").is_some(),
        has_guests: fresh
            .get("attendees")
            .and_then(Value::as_array)
            .is_some_and(|a| {
                a.iter()
                    .any(|g| !g.get("self").and_then(Value::as_bool).unwrap_or(false))
            }),
        html_description: text("description").is_some_and(patch::description_is_html),
    }
}

/// The gate's facts from a mirror row, for the list. Account state isn't read here (that would be
/// a keychain read per row): the list assumes editing is on, and the view merges the account's
/// real status from `calendar_overview`.
fn facts_from_row<'a>(row: &'a CalendarEvent, calendar: &'a calendar::Calendar) -> EditFacts<'a> {
    EditFacts {
        google: calendar.provider == "google" && calendar.source_id.starts_with("gcal:"),
        editing: EditingStatus::On,
        access_role: calendar.facts.access_role.as_deref(),
        event_type: row.event_type.as_deref(),
        organizer_self: row.organizer_self,
        locked: row.locked,
        visibility: row.visibility.as_deref(),
        recurring: row.recurring || row.series_id.is_some(),
        has_guests: row.attendees.iter().any(|a| !a.is_self),
        html_description: false,
    }
}

/// Each row's list-level edit block (see [`facts_from_row`]): the one reason nothing about it may
/// change, or `None`.
pub(super) fn edit_blocks(
    conn: &Connection,
    rows: &[CalendarEvent],
) -> Result<Vec<Option<ReadOnlyReason>>> {
    let calendars: HashMap<String, calendar::Calendar> = calendar::list_calendars(conn)?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    Ok(rows
        .iter()
        .map(|row| match calendars.get(&row.calendar_id) {
            Some(cal) => gate::edit_block(&facts_from_row(row, cal)),
            None => Some(ReadOnlyReason::NotGoogle),
        })
        .collect())
}

/// A start or end node as the editor's date, `HH:MM` and zone: the node's own zone, else
/// `fallback`.
fn wall_parts(node: &Value, fallback: &str) -> Option<(String, String, String)> {
    let at = chrono::DateTime::parse_from_rfc3339(node.get("dateTime")?.as_str()?.trim()).ok()?;
    let zone_name = node
        .get("timeZone")
        .and_then(Value::as_str)
        .unwrap_or(fallback);
    let zone = time::parse_zone(zone_name)
        .or_else(|_| time::parse_zone(fallback))
        .ok()?;
    let local = at.with_timezone(&zone);
    Some((
        local.format("%Y-%m-%d").to_string(),
        local.format("%H:%M").to_string(),
        zone.name().to_string(),
    ))
}

/// The editor's view of Google's fresh copy. `fallback_zone` (the calendar's zone, else the
/// device's) is used only for a node Google sent without a zone.
fn event_for_edit(fresh: &Value, fallback_zone: &str) -> Option<EventForEdit> {
    let start = fresh.get("start")?;
    let end = fresh.get("end")?;
    let time = match start.get("date").and_then(Value::as_str) {
        Some(first) => {
            let first_day = chrono::NaiveDate::parse_from_str(first, "%Y-%m-%d").ok()?;
            let end_day = end
                .get("date")
                .and_then(Value::as_str)
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .unwrap_or(first_day);
            TimeDraft::AllDay {
                first_day: first.to_string(),
                last_day: time::last_day_covered(first_day, end_day)
                    .format("%Y-%m-%d")
                    .to_string(),
            }
        }
        None => {
            let (start_date, start_time, start_zone) = wall_parts(start, fallback_zone)?;
            let (end_date, end_time, end_zone) = wall_parts(end, &start_zone)?;
            TimeDraft::Timed {
                start_date,
                start_time,
                start_zone,
                end_date,
                end_time,
                end_zone,
            }
        }
    };
    let text = |key: &str| {
        fresh
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let description = text("description");
    let instant = |node: &Value| {
        node.get("dateTime")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let all_day = matches!(time, TimeDraft::AllDay { .. });
    Some(EventForEdit {
        summary: text("summary"),
        location: text("location"),
        description_html: patch::description_is_html(&description),
        description,
        time,
        start_at: (!all_day).then(|| instant(start)).flatten(),
        end_at: (!all_day).then(|| instant(end)).flatten(),
        show_as: match fresh.get("transparency").and_then(Value::as_str) {
            Some("transparent") => ShowAs::Free,
            _ => ShowAs::Busy,
        },
        visibility: match fresh.get("visibility").and_then(Value::as_str) {
            Some("public") => Visibility::Public,
            Some("private") => Visibility::Private,
            Some("confidential") => Visibility::Confidential,
            _ => Visibility::Default,
        },
        attachments: fresh
            .get("attachments")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("title").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        html_link: fresh
            .get("htmlLink")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The reason to give when a draft touches a field the gate closed.
fn refused_field(perms: &FieldPermissions, draft: &EventPatchDraft) -> Option<ReadOnlyReason> {
    let touched_closed = (draft.summary.is_some() && !perms.summary)
        || (draft.time.is_some() && !perms.time)
        || (draft.location.is_some() && !perms.location)
        || (draft.description.is_some() && !perms.description)
        || (draft.show_as.is_some() && !perms.show_as)
        || (draft.visibility.is_some() && !perms.visibility);
    touched_closed.then(|| {
        perms
            .reasons
            .first()
            .copied()
            .unwrap_or(ReadOnlyReason::CalendarReadOnly)
    })
}

/// Move an editor's base onto Google's copy after a save, for the fields the save sent and no
/// others. The editor still shows the rest as it was opened, so a change someone made meanwhile to
/// a field this save didn't send must stay a difference from the base: the next save that touches
/// that field then reports it as a conflict instead of overwriting it unseen (R2).
fn rebase_sent(base: &mut Value, saved: &Value, sent: &serde_json::Map<String, Value>) {
    let Some(base) = base.as_object_mut() else {
        return;
    };
    for key in sent.keys() {
        match saved.get(key) {
            Some(value) => {
                base.insert(key.clone(), value.clone());
            }
            None => {
                base.remove(key);
            }
        }
    }
}

/// A fresh random session id: 128 bits, hex.
fn new_session_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Writes go out under the vault owner's Google sign-in, which is in their keychain on this PC: on
/// a shared vault, nobody else's profile can send one.
fn require_owner(app: &AppHandle) -> Result<()> {
    require_vault_owner(app).map_err(|_| {
        Error::Other("On a shared vault, only its owner can change calendar events from PM.".into())
    })
}

// --- the commands ---

/// Open an event for editing: a fresh copy from Google, what may change about it, and a session id
/// for the save. `device_zone` is the webview's IANA zone, required (no UTC fallback, rule R6).
#[tauri::command]
pub async fn get_calendar_event_for_edit(
    app: AppHandle,
    window: tauri::Window,
    event_id: String,
    device_zone: String,
) -> Result<EditLoad> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let device_zone = match time::parse_device_zone(Some(&device_zone)) {
        Ok(zone) => zone,
        Err(e) => {
            return Ok(EditLoad::Failed {
                message: e.to_string(),
            })
        }
    };
    let state = app.state::<AppState>();
    let (target, choice) = {
        let conn = state.conn()?;
        match resolve_target(&conn, &event_id)? {
            Ok(target) => {
                let choice = calendar_editing::choice(&conn, &target.email)?;
                (target, choice)
            }
            Err(reason) => {
                return Ok(EditLoad::Failed {
                    message: read_only_message(reason).into(),
                })
            }
        }
    };
    let editing = editing_status(choice, &target.token_key);
    let fresh = match fetch(&target).await {
        Fetched::Event(fresh) => fresh,
        Fetched::Gone => {
            land_gone(&app, &target, None);
            return Ok(EditLoad::Gone);
        }
        Fetched::Reauth => return Ok(EditLoad::Reauth),
        Fetched::Busy => {
            return Ok(EditLoad::Failed {
                message: "Google is busy right now. Try again in a moment.".into(),
            })
        }
        Fetched::Failed(message) => return Ok(EditLoad::Failed { message }),
    };
    // `get` still returns a deleted event, details and all: it's gone, not editable.
    if patch::is_gone(&fresh) {
        land_gone(&app, &target, Some(&fresh));
        return Ok(EditLoad::Gone);
    }
    let permissions = gate::edit_rights(&facts_from_fresh(&fresh, &target.facts, editing));
    let fallback = target
        .facts
        .time_zone
        .clone()
        .unwrap_or_else(|| device_zone.name().to_string());
    let Some(event) = event_for_edit(&fresh, &fallback) else {
        return Ok(EditLoad::Failed {
            message: "Google sent this event without a start or end PM can read.".into(),
        });
    };
    let session = new_session_id();
    {
        let mut sessions = state
            .calendar_edit
            .sessions
            .lock()
            .map_err(|_| Error::Other("editor sessions lock poisoned".into()))?;
        sessions.retain(|_, s| s.opened.elapsed() < SESSION_TTL);
        sessions.insert(
            session.clone(),
            EditSession {
                target,
                base: fresh,
                opened: Instant::now(),
            },
        );
    }
    Ok(EditLoad::Ready {
        session,
        event: Box::new(event),
        permissions,
    })
}

/// Save the fields the user changed. Built against a fresh copy, sent with its etag, and never over a
/// change Google made meanwhile to a field this save also sends.
#[tauri::command]
pub async fn update_calendar_event(
    app: AppHandle,
    window: tauri::Window,
    session: String,
    draft: EventPatchDraft,
) -> Result<WriteOutcome> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let state = app.state::<AppState>();
    let Some(open) = state
        .calendar_edit
        .sessions
        .lock()
        .map_err(|_| Error::Other("editor sessions lock poisoned".into()))?
        .get(&session)
        .filter(|open| open.opened.elapsed() < SESSION_TTL)
        .cloned()
    else {
        return Ok(WriteOutcome::Failed {
            // Lapsed after hours open, or PM restarted since it was opened.
            message: "This editor's session has ended. Open the event again.".into(),
        });
    };
    let Some(_flight) = InFlight::claim(&state.calendar_edit, &open.target.row_id) else {
        return Ok(WriteOutcome::Busy);
    };
    let choice = calendar_editing::choice(&*state.conn()?, &open.target.email)?;
    let editing = editing_status(choice, &open.target.token_key);
    let target = &open.target;

    // At most two rounds: a 412 means the event changed between our GET and our PATCH, and gets one
    // fresh look (R2); a second 412 is a conflict.
    for round in 1..=2 {
        let fresh = match fetch(target).await {
            Fetched::Event(fresh) => fresh,
            Fetched::Gone => return Ok(land_gone(&app, target, None)),
            Fetched::Reauth => return Ok(WriteOutcome::Reauth),
            Fetched::Busy => return Ok(WriteOutcome::Busy),
            Fetched::Failed(message) => return Ok(WriteOutcome::Failed { message }),
        };
        if patch::is_gone(&fresh) {
            return Ok(land_gone(&app, target, Some(&fresh)));
        }
        let facts = facts_from_fresh(&fresh, &target.facts, editing);
        let perms = gate::edit_rights(&facts);
        if let Some(reason) = refused_field(&perms, &draft) {
            return Ok(WriteOutcome::ReadOnly { reason });
        }
        let body = match patch::build_patch(&fresh, &draft) {
            Ok(body) => body,
            Err(e) => {
                return Ok(WriteOutcome::Failed {
                    message: e.to_string(),
                })
            }
        };
        if body.is_empty() {
            return Ok(WriteOutcome::NoChange);
        }
        // Fail closed: a single-event save sends nothing beyond the fields it may change.
        if body.keys().any(|k| !patch::PATCHABLE.contains(&k.as_str())) {
            return Ok(WriteOutcome::Failed {
                message: "PM built a change it isn't allowed to send, so nothing was saved.".into(),
            });
        }
        match patch::update_check(&open.base, &fresh, &body) {
            UpdateCheck::Proceed => {}
            UpdateCheck::Gone => return Ok(land_gone(&app, target, Some(&fresh))),
            UpdateCheck::Conflict(fields) => return Ok(WriteOutcome::Conflict { fields }),
        }
        let Some(etag) = fresh.get("etag").and_then(Value::as_str) else {
            return Ok(WriteOutcome::Failed {
                message: "Google sent this event without a version stamp, so PM won't change it."
                    .into(),
            });
        };
        // R7: nobody to email while events with guests stay read-only (C12 adds the choice).
        let request = plan::patch_event(
            &target.remote_calendar,
            &target.remote_event,
            etag,
            Value::Object(body.clone()),
            Notify::None,
            plan::VersionFlags::default(),
        );
        let reply = match google_io::send(&target.token_key, &request).await {
            Ok(reply) => reply,
            Err(Error::Reauth(_)) => return Ok(WriteOutcome::Reauth),
            // No answer: it may or may not have landed. Look before saying either.
            Err(_) => return Ok(after_lost_answer(&app, target, &session, &body).await),
        };
        // A server error on the way may have been applied, whatever the last answer says (a retry
        // of a copy that landed is answered 412): look before reporting anything but success.
        if reply.maybe_applied && reply.verdict != Verdict::Ok {
            match fetch(target).await {
                Fetched::Event(now) if patch::already_applied(&now, &body) => {
                    return Ok(land_saved(&app, target, &session, now, &body));
                }
                Fetched::Event(_) => {}
                Fetched::Gone => return Ok(land_gone(&app, target, None)),
                _ => return Ok(WriteOutcome::Unconfirmed),
            }
        }
        match reply.verdict {
            Verdict::Ok => {
                return Ok(match serde_json::from_str::<Value>(&reply.body) {
                    Ok(saved) => land_saved(&app, target, &session, saved, &body),
                    // Saved, but the reply wasn't JSON: the next sync will show it.
                    Err(_) => saved_pending(&app, target),
                });
            }
            Verdict::Conflict if round == 1 => continue,
            Verdict::Gone => return Ok(land_gone(&app, target, None)),
            verdict => return Ok(outcome_for(verdict, editing)),
        }
    }
    Ok(WriteOutcome::Conflict { fields: Vec::new() })
}

/// Delete an event after an Undo window (plan Q2). The delete is held here in the backend, so it
/// outlives the Calendar tab unmounting, and `cancel_calendar_delete` within `UNDO` keeps the event.
/// When the hold ends the delete goes out only if what the user saw (`seen`, from the row they
/// clicked) is still what Google holds, and `calendar://delete-settled` says how it ended. If PM quits
/// during the hold, nothing is deleted. Other writes to the event answer Busy meanwhile.
#[tauri::command]
pub async fn delete_calendar_event(
    app: AppHandle,
    window: tauri::Window,
    event_id: String,
    seen: SeenSummary,
) -> Result<DeleteStart> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let refused = |result| Ok(DeleteStart::Refused { result });
    let state = app.state::<AppState>();
    let (target, choice) = {
        let conn = state.conn()?;
        match resolve_target(&conn, &event_id)? {
            Ok(target) => {
                let choice = calendar_editing::choice(&conn, &target.email)?;
                (target, choice)
            }
            Err(reason) => return refused(WriteOutcome::ReadOnly { reason }),
        }
    };
    // Said now rather than after the wait; the rest of the gate needs Google's copy, and runs then.
    if editing_status(choice, &target.token_key) != EditingStatus::On {
        return refused(WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::EditingOff,
        });
    }
    let Some(flight) = InFlight::claim(&state.calendar_edit, &target.row_id) else {
        return refused(WriteOutcome::Busy);
    };
    let claim = flight.hand_off(&app);
    let undo_token = new_session_id();
    let undone = state.calendar_edit.held_deletes.hold(
        &undo_token,
        &event_id,
        &seen.summary,
        Instant::now() + UNDO,
    )?;
    let (task_app, token) = (app.clone(), undo_token.clone());
    tauri::async_runtime::spawn(async move {
        let _claim = claim;
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(UNDO) => false,
            _ = undone => true,
        };
        {
            let state = task_app.state::<AppState>();
            // Whoever takes the entry decides: an Undo that got there first wins.
            if cancelled || !state.calendar_edit.held_deletes.take(&token) {
                return;
            }
        }
        let result = send_delete(&task_app, &target, &seen).await;
        let _ = task_app.emit_to(
            "main",
            "calendar://delete-settled",
            DeleteSettled {
                undo_token: token,
                event_id,
                result,
            },
        );
    });
    Ok(DeleteStart::Held {
        undo_token,
        undo_seconds: UNDO.as_secs(),
    })
}

/// Undo a held delete. `true` when it was still waiting and now won't be sent; `false` when it had
/// already gone to Google (or never existed), and `calendar://delete-settled` will say how it ended.
#[tauri::command]
pub fn cancel_calendar_delete(
    app: AppHandle,
    window: tauri::Window,
    undo_token: String,
) -> Result<bool> {
    require_main_window(&window)?;
    Ok(app
        .state::<AppState>()
        .calendar_edit
        .held_deletes
        .cancel(&undo_token))
}

/// How long a delete waits for Undo. One duration everywhere (plan A15).
const UNDO: Duration = Duration::from_secs(8);

/// Deletes waiting out their Undo window, by the token the webview holds. Whoever removes a token's
/// entry decides its fate: `cancel` (the delete is kept) or `take` (it goes to Google). Dropping an
/// entry without sending also wakes its task as undone, which is how quitting abandons them all.
#[derive(Default)]
pub(crate) struct HeldDeletes(Mutex<HashMap<String, Held>>);

/// One held delete: how to undo it, and what a reloaded webview needs to show it again.
struct Held {
    undo: tokio::sync::oneshot::Sender<()>,
    event_id: String,
    summary: String,
    until: Instant,
}

impl HeldDeletes {
    /// Hold the delete of `event_id` under `token` until `until`; the receiver resolves when it is
    /// undone (or abandoned).
    fn hold(
        &self,
        token: &str,
        event_id: &str,
        summary: &str,
        until: Instant,
    ) -> Result<tokio::sync::oneshot::Receiver<()>> {
        let (undo, rx) = tokio::sync::oneshot::channel();
        self.0
            .lock()
            .map_err(|_| Error::Other("held deletes lock poisoned".into()))?
            .insert(
                token.to_string(),
                Held {
                    undo,
                    event_id: event_id.to_string(),
                    summary: summary.to_string(),
                    until,
                },
            );
        Ok(rx)
    }

    /// Undo: `true` when the delete was still waiting.
    fn cancel(&self, token: &str) -> bool {
        let held = self.0.lock().ok().and_then(|mut held| held.remove(token));
        held.map(|h| h.undo.send(())).is_some()
    }

    /// The wait is over: `true` when nothing undid the delete first, so it goes to Google.
    fn take(&self, token: &str) -> bool {
        self.0
            .lock()
            .ok()
            .and_then(|mut held| held.remove(token))
            .is_some()
    }

    /// PM is quitting: every waiting delete is dropped, so none can be sent during the shutdown
    /// work that follows (plan Q2: quitting in the window deletes nothing).
    fn abandon_all(&self) {
        if let Ok(mut held) = self.0.lock() {
            held.clear();
        }
    }

    /// The deletes still waiting, for a webview that reloaded.
    fn list(&self) -> Vec<HeldDeleteInfo> {
        let Ok(held) = self.0.lock() else {
            return Vec::new();
        };
        let now = Instant::now();
        held.iter()
            .map(|(token, h)| HeldDeleteInfo {
                undo_token: token.clone(),
                event_id: h.event_id.clone(),
                summary: h.summary.clone(),
                seconds_left: h.until.saturating_duration_since(now).as_secs(),
            })
            .collect()
    }
}

impl CalendarEditState {
    /// Drop every held delete (see [`HeldDeletes::abandon_all`]).
    pub(crate) fn abandon_held_deletes(&self) {
        self.held_deletes.abandon_all();
    }
}

/// The deletes still waiting for their Undo window to end, so a webview that reloaded can show them
/// again (plan A15).
#[tauri::command]
pub fn list_held_deletes(app: AppHandle, window: tauri::Window) -> Result<Vec<HeldDeleteInfo>> {
    require_main_window(&window)?;
    Ok(app.state::<AppState>().calendar_edit.held_deletes.list())
}

/// Send a held delete: a fresh copy, the gate, the "is it still what you saw" rule, then a DELETE
/// guarded by the etag. Editing is read again here, since it may have been turned off while the
/// delete waited.
async fn send_delete(app: &AppHandle, target: &Target, seen: &SeenSummary) -> WriteOutcome {
    let choice = {
        let state = app.state::<AppState>();
        let choice = state
            .conn()
            .and_then(|conn| calendar_editing::choice(&conn, &target.email));
        match choice {
            Ok(choice) => choice,
            Err(e) => {
                return WriteOutcome::Failed {
                    message: e.to_string(),
                }
            }
        }
    };
    let editing = editing_status(choice, &target.token_key);
    for round in 1..=2 {
        let fresh = match fetch(target).await {
            Fetched::Event(fresh) => fresh,
            Fetched::Gone => return land_gone(app, target, None),
            Fetched::Reauth => return WriteOutcome::Reauth,
            Fetched::Busy => return WriteOutcome::Busy,
            Fetched::Failed(message) => return WriteOutcome::Failed { message },
        };
        if patch::is_gone(&fresh) {
            return land_gone(app, target, Some(&fresh));
        }
        let perms = gate::edit_rights(&facts_from_fresh(&fresh, &target.facts, editing));
        if !perms.delete {
            return WriteOutcome::ReadOnly {
                reason: perms
                    .reasons
                    .first()
                    .copied()
                    .unwrap_or(ReadOnlyReason::CalendarReadOnly),
            };
        }
        match patch::delete_still_matches(seen, &fresh) {
            DeleteCheck::Matches => {}
            DeleteCheck::Gone => return land_gone(app, target, Some(&fresh)),
            DeleteCheck::Changed(fields) => {
                // Show what Google holds now, so the row that comes back is the changed one and a
                // second delete compares against it rather than conflicting again.
                if let Some(row) = calendar::parse_event(&target.calendar_id, &fresh) {
                    let _ = land(app, target, vec![row], Vec::new());
                }
                return WriteOutcome::Conflict { fields };
            }
        }
        let Some(etag) = fresh.get("etag").and_then(Value::as_str) else {
            return WriteOutcome::Failed {
                message: "Google sent this event without a version stamp, so PM won't delete it."
                    .into(),
            };
        };
        let request = plan::delete_event(
            &target.remote_calendar,
            &target.remote_event,
            etag,
            Notify::None,
        );
        let reply = match google_io::send(&target.token_key, &request).await {
            Ok(reply) => reply,
            Err(Error::Reauth(_)) => return WriteOutcome::Reauth,
            // No answer: look. Gone means it landed; still there means it didn't.
            Err(_) => {
                return match still_there(target).await {
                    Some(false) => land_deleted(app, target, &fresh),
                    Some(true) => WriteOutcome::Failed {
                        message: "PM lost touch with Google, and the event is still there. \
                                  Nothing was deleted; try again."
                            .into(),
                    },
                    None => WriteOutcome::Unconfirmed,
                }
            }
        };
        // A server error on the way may have been applied (a retry of a delete that landed is
        // answered 412 or 410): look before reporting anything but success.
        if reply.maybe_applied && !matches!(reply.verdict, Verdict::Ok | Verdict::Gone) {
            match still_there(target).await {
                Some(false) => return land_deleted(app, target, &fresh),
                Some(true) => {}
                None => return WriteOutcome::Unconfirmed,
            }
        }
        match reply.verdict {
            // 2xx, or already gone: either way it's gone from Google now.
            Verdict::Ok | Verdict::Gone => return land_deleted(app, target, &fresh),
            Verdict::Conflict if round == 1 => continue,
            verdict => return outcome_for(verdict, editing),
        }
    }
    WriteOutcome::Conflict { fields: Vec::new() }
}

// --- around the I/O ---

/// A fresh GET's result.
enum Fetched {
    Event(Value),
    Gone,
    Reauth,
    Busy,
    Failed(String),
}

async fn fetch(target: &Target) -> Fetched {
    let request = plan::get_event(&target.remote_calendar, &target.remote_event);
    match google_io::send(&target.token_key, &request).await {
        Ok(reply) => match reply.verdict {
            Verdict::Ok => match serde_json::from_str(&reply.body) {
                Ok(v) => Fetched::Event(v),
                Err(_) => Fetched::Failed("Google sent an event PM couldn't read.".into()),
            },
            Verdict::Gone => Fetched::Gone,
            Verdict::Reauth => Fetched::Reauth,
            Verdict::RateLimited => Fetched::Busy,
            Verdict::QuotaExceeded => Fetched::Failed(QUOTA.into()),
            // Refused a read: the calendar was unshared, or the sign-in no longer covers it.
            Verdict::ReadOnly | Verdict::InsufficientScope | Verdict::NotOrganizer => {
                Fetched::Failed(
                    "Google won't let this account open the event any more. Refresh the calendar \
                     and try again."
                        .into(),
                )
            }
            other => Fetched::Failed(failure_message(&other)),
        },
        Err(Error::Reauth(_)) => Fetched::Reauth,
        Err(Error::Http(_)) => Fetched::Failed(
            "PM couldn't reach Google. Nothing was changed; try again when you're back online."
                .into(),
        ),
        // Not the network: say what it was (a keychain error, say) rather than "you're offline".
        Err(e) => Fetched::Failed(format!(
            "PM couldn't ask Google about this event, so nothing was changed: {e}"
        )),
    }
}

/// The account is over one of Google Calendar's usage limits, which lift on their own, not soon.
const QUOTA: &str = "This Google account has reached one of Google Calendar's usage limits, so \
                     Google isn't taking changes for now. Try again later; the limit usually lifts \
                     within a day.";

/// A save whose answer never arrived: GET the event and see whether it landed.
async fn after_lost_answer(
    app: &AppHandle,
    target: &Target,
    session: &str,
    body: &serde_json::Map<String, Value>,
) -> WriteOutcome {
    match fetch(target).await {
        Fetched::Event(now) if patch::already_applied(&now, body) => {
            land_saved(app, target, session, now, body)
        }
        Fetched::Event(_) => WriteOutcome::Failed {
            message: "PM lost touch with Google, and your change isn't there. Nothing was saved; \
                      try again."
                .into(),
        },
        _ => WriteOutcome::Unconfirmed,
    }
}

/// After a delete whose answer can't be trusted: whether the event is still in Google, or `None`
/// when PM couldn't look.
async fn still_there(target: &Target) -> Option<bool> {
    match fetch(target).await {
        Fetched::Gone => Some(false),
        Fetched::Event(now) => Some(!patch::is_gone(&now)),
        _ => None,
    }
}

/// What a settled verdict means for the user.
fn outcome_for(verdict: Verdict, editing: EditingStatus) -> WriteOutcome {
    match verdict {
        Verdict::Ok => WriteOutcome::Saved {
            warnings: Vec::new(),
        },
        Verdict::Conflict => WriteOutcome::Conflict { fields: Vec::new() },
        Verdict::Gone => WriteOutcome::Gone,
        Verdict::NotOrganizer => WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::NotOrganizer,
        },
        Verdict::ReadOnly => WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::CalendarReadOnly,
        },
        // R10: with the write scope already granted, a scope error can't be fixed by consenting
        // again (that would loop); without it, the account needs editing turned on again.
        Verdict::InsufficientScope => WriteOutcome::ReadOnly {
            reason: if editing == EditingStatus::On {
                ReadOnlyReason::CalendarReadOnly
            } else {
                ReadOnlyReason::EditingOff
            },
        },
        Verdict::RateLimited => WriteOutcome::Busy,
        Verdict::QuotaExceeded => WriteOutcome::Failed {
            message: QUOTA.into(),
        },
        Verdict::Reauth => WriteOutcome::Reauth,
        other => WriteOutcome::Failed {
            message: failure_message(&other),
        },
    }
}

fn failure_message(verdict: &Verdict) -> String {
    match verdict {
        Verdict::BadRequest(m) | Verdict::Other(_, m) if !m.is_empty() => {
            format!("Google didn't accept the change: {m}")
        }
        Verdict::ServerError => "Google had a problem with this. Try again in a moment.".into(),
        _ => "Google didn't accept the change.".into(),
    }
}

fn read_only_message(reason: ReadOnlyReason) -> &'static str {
    match reason {
        ReadOnlyReason::NotGoogle => {
            "This event comes from a calendar PM can only read (a subscription or Outlook)."
        }
        _ => "PM can't change this event.",
    }
}

/// Write Google's reply into the mirror, remember it for the next sync, and tell the briefing and
/// the main window. Lock order: the DB, then the recent writes.
fn land(
    app: &AppHandle,
    target: &Target,
    upserts: Vec<CalendarEvent>,
    deletes: Vec<(String, Option<String>, Option<String>)>,
) -> Result<()> {
    let state = app.state::<AppState>();
    {
        let conn = state.conn()?;
        let delete_ids: Vec<String> = deletes.iter().map(|(id, _, _)| id.clone()).collect();
        calendar::apply_write_effect(&conn, &target.calendar_id, &upserts, &delete_ids)?;
        let mut recent = state
            .calendar_edit
            .recent
            .lock()
            .map_err(|_| Error::Other("recent writes lock poisoned".into()))?;
        let now = Stamp::now();
        for row in upserts {
            recent.record_upsert(row, now);
        }
        for (id, etag, updated) in deletes {
            recent.record_delete(&target.calendar_id, &id, etag, updated, now);
        }
    }
    briefing::nudge(&state);
    let _ = app.emit_to(
        "main",
        "calendar://write-outcome",
        WriteLanded {
            calendar_id: target.calendar_id.clone(),
        },
    );
    Ok(())
}

/// The `calendar://write-outcome` payload: a calendar whose mirror a save just changed.
#[derive(Clone, Serialize)]
struct WriteLanded {
    calendar_id: String,
}

/// Saved at Google, but not mirrored yet: say so; the next sync fills it in.
fn saved_pending(app: &AppHandle, target: &Target) -> WriteOutcome {
    let state = app.state::<AppState>();
    if let Ok(conn) = state.conn() {
        // The row no longer matches Google; make sure the next sync rewrites it.
        let _ = calendar::apply_write_effect(&conn, &target.calendar_id, &[], &[]);
    }
    WriteOutcome::Saved {
        warnings: vec![crate::calendar_write::dto::WriteWarning::MirrorRefreshPending],
    }
}

/// Gone from Google (deleted elsewhere, or cancelled from its series): take it out of the mirror too.
/// `cancelled` is Google's copy of the deleted event when it sent one, so a restore after it shows.
fn land_gone(app: &AppHandle, target: &Target, cancelled: Option<&Value>) -> WriteOutcome {
    let text = |key: &str| {
        cancelled
            .and_then(|c| c.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let _ = land(
        app,
        target,
        Vec::new(),
        vec![(target.row_id.clone(), text("etag"), text("updated"))],
    );
    WriteOutcome::Gone
}

/// Our delete landed: take the row out, remembering the version deleted so a stale fetch can't
/// bring it back.
fn land_deleted(app: &AppHandle, target: &Target, deleted: &Value) -> WriteOutcome {
    let text = |key: &str| deleted.get(key).and_then(Value::as_str).map(str::to_string);
    match land(
        app,
        target,
        Vec::new(),
        vec![(target.row_id.clone(), text("etag"), text("updated"))],
    ) {
        Ok(()) => WriteOutcome::Saved {
            warnings: Vec::new(),
        },
        Err(_) => saved_pending(app, target),
    }
}

/// A save landed: Google's copy now (its reply, or a fresh GET) goes into the mirror, and the session
/// moves onto it for the fields this save `sent` (see [`rebase_sent`]).
fn land_saved(
    app: &AppHandle,
    target: &Target,
    session: &str,
    saved: Value,
    sent: &serde_json::Map<String, Value>,
) -> WriteOutcome {
    if let Ok(mut sessions) = app.state::<AppState>().calendar_edit.sessions.lock() {
        if let Some(open) = sessions.get_mut(session) {
            rebase_sent(&mut open.base, &saved, sent);
        }
    }
    let row = calendar::parse_event(&target.calendar_id, &saved);
    match row.map(|row| land(app, target, vec![row], Vec::new())) {
        Some(Ok(())) => WriteOutcome::Saved {
            warnings: Vec::new(),
        },
        // Saved, but not as a row PM could mirror, or the mirror write failed: the next sync shows it.
        _ => saved_pending(app, target),
    }
}

/// A claim on one mirror row while a write to it is on its way; released on drop, panics included.
struct InFlight<'a> {
    edit: &'a CalendarEditState,
    row_id: String,
}

impl<'a> InFlight<'a> {
    fn claim(edit: &'a CalendarEditState, row_id: &str) -> Option<Self> {
        let mut rows = edit.in_flight.lock().ok()?;
        rows.insert(row_id.to_string()).then(|| InFlight {
            edit,
            row_id: row_id.to_string(),
        })
    }

    /// Keep the claim past this command: a held delete's task holds it through the wait and the
    /// send, so the event answers Busy all that time.
    fn hand_off(self, app: &AppHandle) -> HeldClaim {
        // The claim moves into the returned value, so this one must not release it on drop.
        let mut this = std::mem::ManuallyDrop::new(self);
        HeldClaim {
            app: app.clone(),
            row_id: std::mem::take(&mut this.row_id),
        }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut rows) = self.edit.in_flight.lock() {
            rows.remove(&self.row_id);
        }
    }
}

/// A claim that outlives the command that took it ([`InFlight::hand_off`]); released on drop.
struct HeldClaim {
    app: AppHandle,
    row_id: String,
}

impl Drop for HeldClaim {
    fn drop(&mut self) {
        if let Ok(mut rows) = self.app.state::<AppState>().calendar_edit.in_flight.lock() {
            rows.remove(&self.row_id);
        }
    }
}

/// The only code that sends a calendar write to Google. Private, so nothing outside this module can
/// call it, and the fence script keeps its name out of every model path.
mod google_io {
    use std::time::Duration;

    use crate::calendar_write::classify::{self, Method, Verdict};
    use crate::calendar_write::plan::RequestPlan;
    use crate::error::{Error, Result};
    use crate::google;

    /// Google's answer: what it means, and the body.
    pub(super) struct Reply {
        pub verdict: Verdict,
        pub body: String,
        /// A change was answered with a server error along the way, which Google may still have
        /// applied: unless the verdict is a success, the caller looks before saying what happened.
        pub maybe_applied: bool,
    }

    /// The most PM reads of an answer: an event is a few kilobytes.
    const MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;

    /// Send `plan`, retrying once where that can't apply a change twice (`classify::should_retry`).
    /// A 429 was already waited out (or handed back) by `authorized_send_with` under the interactive
    /// policy, so only Google's 403 rate-limit shape is retried here. `Err` means no answer came back
    /// (offline, a timeout) or the sign-in needs renewing.
    pub(super) async fn send(token_key: &str, plan: &RequestPlan) -> Result<Reply> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(Error::from)?;
        let mut attempt = 1;
        let mut maybe_applied = false;
        loop {
            let resp = google::authorized_send_with(
                &client,
                token_key,
                google::SendPolicy::INTERACTIVE,
                |c, bearer| {
                    let rb = match plan.method {
                        Method::Get => c.get(&plan.url),
                        Method::Patch => c.patch(&plan.url),
                        Method::Delete => c.delete(&plan.url),
                        Method::Insert => c.post(&plan.url),
                    }
                    .bearer_auth(bearer);
                    let rb = match &plan.if_match {
                        Some(etag) => rb.header(reqwest::header::IF_MATCH, etag),
                        None => rb,
                    };
                    match &plan.body {
                        Some(body) => rb.json(body),
                        None => rb,
                    }
                },
            )
            .await?;
            let status = resp.status().as_u16();
            let body = read_capped(resp).await?;
            let verdict = classify::classify(status, &body);
            maybe_applied |= verdict == Verdict::ServerError && plan.method != Method::Get;
            if status != 429 && classify::should_retry(plan.method, &verdict, attempt) {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            return Ok(Reply {
                verdict,
                body,
                maybe_applied,
            });
        }
    }

    /// The answer's body as text, refusing one past [`MAX_REPLY_BYTES`] rather than buffering it.
    async fn read_capped(resp: reqwest::Response) -> Result<String> {
        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if buf.len() + chunk.len() > MAX_REPLY_BYTES {
                return Err(Error::Other("Google's answer was far too large.".into()));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn add_calendar(
        conn: &Connection,
        source: &str,
        provider: &str,
        id: &str,
        remote: Option<&str>,
    ) {
        calendar::upsert_source(conn, source, provider, Some("me@x.com"), "me@x.com").unwrap();
        calendar::upsert_calendar(
            conn,
            &calendar::Calendar {
                id: id.into(),
                source_id: source.into(),
                provider: provider.into(),
                remote_id: remote.map(str::to_string),
                name: "Cal".into(),
                color: None,
                selected: true,
                is_primary: false,
                quiet: false,
                kind: None,
                facts: CalendarFacts {
                    access_role: Some("owner".into()),
                    time_zone: Some("Europe/London".into()),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    }

    fn add_event(conn: &Connection, calendar_id: &str, id: &str) {
        let row = CalendarEvent {
            id: format!("{calendar_id}:{id}"),
            calendar_id: calendar_id.into(),
            summary: "Dentist".into(),
            start: "2026-10-12T09:00:00Z".into(),
            organizer_self: true,
            ..Default::default()
        };
        calendar::replace_events(conn, calendar_id, &[row], true).unwrap();
    }

    #[test]
    fn a_google_row_resolves_to_its_calendar_and_event() {
        let (_d, conn) = store();
        let cal = "gcal:me@x.com:team#1@group.calendar.google.com";
        add_calendar(
            &conn,
            "gcal:me@x.com",
            "google",
            cal,
            Some("team#1@group.calendar.google.com"),
        );
        add_event(&conn, cal, "abc_20261012T090000Z");
        let target = resolve_target(&conn, &format!("{cal}:abc_20261012T090000Z"))
            .unwrap()
            .unwrap();
        assert_eq!(target.remote_calendar, "team#1@group.calendar.google.com");
        assert_eq!(target.remote_event, "abc_20261012T090000Z");
        assert_eq!(target.email, "me@x.com");
        assert_eq!(target.token_key, "google_oauth_token_calendar::me@x.com");
        assert_eq!(target.facts.access_role.as_deref(), Some("owner"));
    }

    /// An id `Url::path_segments_mut` would drop, or turn into a dot segment, is never written
    /// through: the request would land on the event list or the calendar instead.
    #[test]
    fn an_id_that_isnt_one_url_segment_is_never_written_through() {
        for bad in [".", "..", ".\t.", "a\nb", ""] {
            assert!(!is_plain_segment(bad), "{bad:?}");
        }
        for good in [
            "abc_20261012T090000Z",
            "team#1@group.calendar.google.com",
            "a.b",
            "%2e%2e",
        ] {
            assert!(is_plain_segment(good), "{good:?}");
        }
        // What the guard prevents: url drops the segment, so the write would go to the list.
        let mut url = reqwest::Url::parse("https://example.com/calendars/c/events").unwrap();
        url.path_segments_mut().unwrap().extend([".."]);
        assert_eq!(url.path(), "/calendars/c/events");

        let (_d, conn) = store();
        let cal = "gcal:me@x.com:primary";
        add_calendar(&conn, "gcal:me@x.com", "google", cal, Some("primary"));
        add_event(&conn, cal, "..");
        assert_eq!(
            resolve_target(&conn, &format!("{cal}:..")).unwrap(),
            Err(ReadOnlyReason::NotGoogle)
        );
        let odd = "gcal:me@x.com:odd";
        add_calendar(&conn, "gcal:me@x.com", "google", odd, Some("."));
        add_event(&conn, odd, "abc");
        assert_eq!(
            resolve_target(&conn, &format!("{odd}:abc")).unwrap(),
            Err(ReadOnlyReason::NotGoogle)
        );
    }

    #[test]
    fn subscription_and_outlook_rows_are_not_google_oauth() {
        let (_d, conn) = store();
        add_calendar(&conn, "ics:feed1", "other", "ics:feed1", None);
        add_event(&conn, "ics:feed1", "uid1:2026-10-12");
        assert_eq!(
            resolve_target(&conn, "ics:feed1:uid1:2026-10-12").unwrap(),
            Err(ReadOnlyReason::NotGoogle)
        );
        let outlook = "outlook:me@x.com:AAMk";
        add_calendar(
            &conn,
            "outlook:me@x.com",
            "microsoft",
            outlook,
            Some("AAMk"),
        );
        add_event(&conn, outlook, "ev1");
        assert_eq!(
            resolve_target(&conn, &format!("{outlook}:ev1")).unwrap(),
            Err(ReadOnlyReason::NotGoogle)
        );
        // A feed mis-tagged "google" (possible before 3.140.1) still isn't an account.
        add_calendar(&conn, "ics:feed2", "google", "ics:feed2", None);
        add_event(&conn, "ics:feed2", "uid2:2026-10-12");
        assert_eq!(
            resolve_target(&conn, "ics:feed2:uid2:2026-10-12").unwrap(),
            Err(ReadOnlyReason::NotGoogle)
        );
        assert!(resolve_target(&conn, "nope").is_err());
    }

    /// After a title save, a location a colleague changed meanwhile stays a difference from the base,
    /// so the editor's next save of the location is a conflict, not an overwrite.
    #[test]
    fn a_save_moves_the_base_only_for_the_fields_it_sent() {
        let mut base = json!({ "summary": "Dentist", "location": "High St", "etag": "\"1\"" });
        let saved = json!({ "summary": "Dentist (moved)", "location": "Low St", "etag": "\"3\"" });
        let sent = json!({ "summary": "Dentist (moved)" });
        rebase_sent(&mut base, &saved, sent.as_object().unwrap());
        assert_eq!(base["summary"], "Dentist (moved)");
        assert_eq!(base["location"], "High St");
        let next = json!({ "location": "High St, Room 2" });
        assert_eq!(
            patch::update_check(&base, &saved, next.as_object().unwrap()),
            UpdateCheck::Conflict(vec!["location".into()])
        );
        // A field the save cleared, and Google then left out, is gone from the base too.
        let cleared = json!({ "summary": "Dentist (moved)" });
        rebase_sent(
            &mut base,
            &cleared,
            json!({ "location": "" }).as_object().unwrap(),
        );
        assert!(base.get("location").is_none());
    }

    /// Undo and the end of the wait race for one entry, and whichever gets it decides.
    #[test]
    fn a_held_delete_is_either_undone_or_sent_never_both() {
        let held = HeldDeletes::default();
        let until = Instant::now() + UNDO;
        let mut undone = held.hold("a", "cal:a", "Dentist", until).unwrap();
        assert!(held.cancel("a"), "still waiting, so undone");
        assert!(undone.try_recv().is_ok(), "the waiting task hears it");
        assert!(!held.take("a"), "so the wait's end doesn't send it");

        let _rx = held.hold("b", "cal:b", "Gym", until).unwrap();
        assert!(held.take("b"), "the wait ended first: it goes to Google");
        assert!(!held.cancel("b"), "too late to undo");

        assert!(!held.cancel("never-held"));
    }

    /// Quitting drops every waiting delete: each task wakes as undone, and none can be taken.
    #[test]
    fn quitting_abandons_every_held_delete() {
        let held = HeldDeletes::default();
        let until = Instant::now() + UNDO;
        let mut a = held.hold("a", "cal:a", "Dentist", until).unwrap();
        let _b = held.hold("b", "cal:b", "Gym", until).unwrap();
        assert_eq!(held.list().len(), 2);
        let listed = held
            .list()
            .into_iter()
            .find(|h| h.undo_token == "a")
            .unwrap();
        assert_eq!(
            (listed.event_id.as_str(), listed.summary.as_str()),
            ("cal:a", "Dentist")
        );
        assert!(listed.seconds_left <= UNDO.as_secs());
        held.abandon_all();
        assert!(
            a.try_recv().is_err(),
            "the sender is gone: the task's wait ends as undone"
        );
        assert!(!held.take("a") && !held.take("b"));
        assert!(held.list().is_empty());
    }

    #[test]
    fn a_second_write_to_the_same_event_is_busy_until_the_first_ends() {
        let edit = CalendarEditState::default();
        let first = InFlight::claim(&edit, "cal:a").expect("free");
        assert!(InFlight::claim(&edit, "cal:a").is_none());
        assert!(
            InFlight::claim(&edit, "cal:b").is_some(),
            "another event is free"
        );
        drop(first);
        assert!(InFlight::claim(&edit, "cal:a").is_some());
    }

    #[test]
    fn the_fresh_copy_decides_the_gate() {
        let facts = CalendarFacts {
            access_role: Some("writer".into()),
            ..Default::default()
        };
        let mine = json!({
            "organizer": { "self": true }, "eventType": "default",
            "attendees": [{ "email": "me@x.com", "self": true }],
            "description": "<b>agenda</b>"
        });
        let f = facts_from_fresh(&mine, &facts, EditingStatus::On);
        assert!(f.organizer_self && !f.has_guests && f.html_description && !f.recurring);
        let invite = json!({
            "organizer": { "email": "boss@x.com" }, "recurringEventId": "s1",
            "attendees": [{ "email": "me@x.com", "self": true }, { "email": "boss@x.com" }]
        });
        let f = facts_from_fresh(&invite, &facts, EditingStatus::On);
        assert!(!f.organizer_self && f.has_guests && f.recurring);
        assert_eq!(gate::edit_block(&f), Some(ReadOnlyReason::NotOrganizer));
    }

    #[test]
    fn the_editor_sees_times_in_the_events_own_zone() {
        let fresh = json!({
            "summary": "Flight", "location": "LHR",
            "start": { "dateTime": "2026-07-01T08:00:00Z", "timeZone": "Europe/London" },
            "end": { "dateTime": "2026-07-01T16:00:00Z", "timeZone": "America/New_York" },
            "transparency": "transparent", "visibility": "private",
            "attachments": [{ "title": "Boarding pass.pdf", "fileUrl": "https://x" }],
            "htmlLink": "https://calendar.google.com/event?eid=x"
        });
        let e = event_for_edit(&fresh, "Europe/Paris").unwrap();
        assert_eq!(
            e.time,
            TimeDraft::Timed {
                start_date: "2026-07-01".into(),
                start_time: "09:00".into(),
                start_zone: "Europe/London".into(),
                end_date: "2026-07-01".into(),
                end_time: "12:00".into(),
                end_zone: "America/New_York".into(),
            }
        );
        assert_eq!(
            (e.show_as, e.visibility),
            (ShowAs::Free, Visibility::Private)
        );
        assert_eq!(e.attachments, vec!["Boarding pass.pdf"]);
        // Google's exact instants travel alongside, for the editor's pre-save check.
        assert_eq!(
            (e.start_at.as_deref(), e.end_at.as_deref()),
            (Some("2026-07-01T08:00:00Z"), Some("2026-07-01T16:00:00Z"))
        );
        // A node without a zone reads in the fallback (the calendar's zone).
        let bare = json!({
            "start": { "dateTime": "2026-07-01T08:00:00Z" },
            "end": { "dateTime": "2026-07-01T09:00:00Z" }
        });
        match event_for_edit(&bare, "Europe/Paris").unwrap().time {
            TimeDraft::Timed {
                start_time,
                start_zone,
                ..
            } => {
                assert_eq!(
                    (start_time.as_str(), start_zone.as_str()),
                    ("10:00", "Europe/Paris")
                )
            }
            other => panic!("{other:?}"),
        }
        // All-day: the last day covered, not Google's exclusive end, and no instants.
        let day = json!({ "start": { "date": "2026-10-10" }, "end": { "date": "2026-10-13" } });
        let e = event_for_edit(&day, "Europe/London").unwrap();
        assert_eq!(
            e.time,
            TimeDraft::AllDay {
                first_day: "2026-10-10".into(),
                last_day: "2026-10-12".into()
            }
        );
        assert_eq!((e.start_at, e.end_at), (None, None));
    }

    #[test]
    fn a_draft_touching_a_closed_field_is_refused_with_the_reason() {
        let locked = gate::edit_rights(&EditFacts {
            google: true,
            editing: EditingStatus::On,
            access_role: Some("owner"),
            event_type: None,
            organizer_self: true,
            locked: true,
            visibility: None,
            recurring: false,
            has_guests: false,
            html_description: false,
        });
        let title = EventPatchDraft {
            summary: Some("x".into()),
            ..Default::default()
        };
        assert_eq!(refused_field(&locked, &title), Some(ReadOnlyReason::Locked));
        let busy = EventPatchDraft {
            show_as: Some(ShowAs::Free),
            ..Default::default()
        };
        assert_eq!(refused_field(&locked, &busy), None);
    }

    #[test]
    fn verdicts_become_outcomes_the_editor_can_explain() {
        assert_eq!(
            outcome_for(Verdict::InsufficientScope, EditingStatus::On),
            WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::CalendarReadOnly
            }
        );
        assert_eq!(
            outcome_for(Verdict::InsufficientScope, EditingStatus::NeedsConsent),
            WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::EditingOff
            }
        );
        assert_eq!(
            outcome_for(Verdict::RateLimited, EditingStatus::On),
            WriteOutcome::Busy
        );
        // A usage limit isn't "busy for a moment".
        assert_eq!(
            outcome_for(Verdict::QuotaExceeded, EditingStatus::On),
            WriteOutcome::Failed {
                message: QUOTA.into()
            }
        );
        assert_eq!(
            outcome_for(Verdict::Reauth, EditingStatus::On),
            WriteOutcome::Reauth
        );
        assert!(matches!(
            outcome_for(Verdict::BadRequest("Invalid start".into()), EditingStatus::On),
            WriteOutcome::Failed { message } if message.contains("Invalid start")
        ));
    }

    #[test]
    fn the_list_marks_what_can_be_edited() {
        let (_d, conn) = store();
        let cal = "gcal:me@x.com:me@x.com";
        add_calendar(&conn, "gcal:me@x.com", "google", cal, Some("me@x.com"));
        let mine = CalendarEvent {
            id: format!("{cal}:a"),
            calendar_id: cal.into(),
            summary: "Mine".into(),
            start: "2026-10-12T09:00:00Z".into(),
            organizer_self: true,
            ..Default::default()
        };
        let invite = CalendarEvent {
            id: format!("{cal}:b"),
            organizer_self: false,
            ..mine.clone()
        };
        let orphan = CalendarEvent {
            calendar_id: "gone".into(),
            ..mine.clone()
        };
        assert_eq!(
            edit_blocks(&conn, &[mine, invite, orphan]).unwrap(),
            vec![
                None,
                Some(ReadOnlyReason::NotOrganizer),
                Some(ReadOnlyReason::NotGoogle)
            ]
        );
    }
}
