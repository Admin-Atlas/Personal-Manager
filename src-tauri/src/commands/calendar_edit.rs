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
use crate::calendar_recur::lines;
use crate::calendar_recur::spec::{day_phrase, describe_lines};
use crate::calendar_write::classify::Verdict;
use crate::calendar_write::dto::{
    ChoiceRequest, DeleteSettled, DeleteStart, EditLoad, EventForEdit, EventPatchDraft,
    FieldPermissions, HeldDeleteInfo, Notify, ReadOnlyReason, ScopeRow, SeenSummary, SeriesChoices,
    SeriesInfo, SeriesScope, SeriesWarningDto, ShowAs, TimeDraft, Visibility, WriteOutcome,
    WriteWarning,
};
use crate::calendar_write::gate::{self, EditFacts};
use crate::calendar_write::patch::{self, DeleteCheck, UpdateCheck};
use crate::calendar_write::plan::{is_plain_segment, RequestPlan};
use crate::calendar_write::reconcile::{RecentWrites, Stamp};
use crate::calendar_write::series::diff::{self, Expect, GoneRow, Refetch, Removes, SeriesDiff};
use crate::calendar_write::series::plan::{
    plan_series_delete, plan_series_edit, Fresh, NotPlanned, Op, Opened, SeriesInput, SeriesPlan,
    SeriesWarning, Undo,
};
use crate::calendar_write::series::run::{
    Answer, End, Machine, Next, Ran, Rollback, Run, Unfit, Unsent, Why,
};
use crate::calendar_write::series::scope::{
    is_first, scope_options, Change, Row, Scope, ScopeReason,
};
use crate::calendar_write::series::{
    text_only, Exceptions, FreshEvent, Listing, Master, SeriesRefusal,
};
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
    /// one has settled a save (plan A23). A second caller waits, then runs; the change check
    /// ([`super::check_google_calendars`]) never waits, it skips.
    pub(crate) sync_lock: tokio::sync::Mutex<()>,
    /// What the change check knows about each Google calendar (F3). Lock order: the DB, then this;
    /// never held across an await.
    pub(crate) fresh: Mutex<crate::calendar_fresh::Freshness>,
}

/// An open editor: which event, and the copy of it the user saw.
#[derive(Clone)]
struct EditSession {
    target: Target,
    base: Value,
    /// An occurrence's series as Google held it when the editor opened, if PM could read it: what
    /// "all events" and "this and following" check against (G9).
    master: Option<Value>,
    opened: Instant,
}

/// How long an editor may stay open before its session lapses.
const SESSION_TTL: Duration = Duration::from_secs(8 * 3600);

/// Lapsed after hours open, or PM restarted since it was opened.
const SESSION_ENDED: &str = "This editor's session has ended. Open the event again.";

/// Keep a new editor's copies (the event, and an occurrence's series) under a fresh session id, and
/// let lapsed sessions go.
fn keep_session(
    edit: &CalendarEditState,
    target: Target,
    base: Value,
    master: Option<Value>,
) -> Result<String> {
    let session = new_session_id();
    let mut sessions = edit
        .sessions
        .lock()
        .map_err(|_| Error::Other("editor sessions lock poisoned".into()))?;
    sessions.retain(|_, s| s.opened.elapsed() < SESSION_TTL);
    sessions.insert(
        session.clone(),
        EditSession {
            target,
            base,
            master,
            opened: Instant::now(),
        },
    );
    Ok(session)
}

/// The open editor `session` names, unless it has lapsed.
fn open_session(edit: &CalendarEditState, session: &str) -> Result<Option<EditSession>> {
    Ok(edit
        .sessions
        .lock()
        .map_err(|_| Error::Other("editor sessions lock poisoned".into()))?
        .get(session)
        .filter(|open| open.opened.elapsed() < SESSION_TTL)
        .cloned())
}

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
/// for the save. `device_zone` is the webview's IANA zone, or `None` when it can't tell (no UTC
/// fallback, rule R6).
#[tauri::command]
pub async fn get_calendar_event_for_edit(
    app: AppHandle,
    window: tauri::Window,
    event_id: String,
    device_zone: Option<String>,
) -> Result<EditLoad> {
    require_main_window(&window)?;
    require_owner(&app)?;
    // Optional: an event's times carry their own zones, so the editor can open without knowing this
    // computer's (its time controls are then off). Given, it has to be a real zone.
    let device_zone = match device_zone.map(|z| time::parse_device_zone(Some(&z))) {
        None => None,
        Some(Ok(zone)) => Some(zone),
        Some(Err(e)) => {
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
    let token_key = target.token_key.clone();
    open_editor(
        &GoogleWire(&token_key),
        &state.calendar_edit,
        target,
        fresh,
        editing,
        device_zone,
    )
    .await
}

/// The editor for `fresh`, Google's copy of the row's event (not gone): what may change, and a new
/// session holding it and, for an occurrence, its series as it is now. The series is only needed
/// for "all events" and "this and following", so one PM can't fetch or read leaves the editor open,
/// and those choices say they can't go ahead.
async fn open_editor<W: Wire>(
    wire: &W,
    edit: &CalendarEditState,
    target: Target,
    fresh: Value,
    editing: EditingStatus,
    device_zone: Option<chrono_tz::Tz>,
) -> Result<EditLoad> {
    let (fresh, master) = open_series(wire, &target, fresh).await;
    let series = master.as_ref().and_then(|m| series_info(&fresh, m));
    let permissions = gate::edit_rights(&facts_from_fresh(&fresh, &target.facts, editing));
    // Only for a start or end Google sent without a zone: the calendar's zone, else this computer's.
    // Never a guessed UTC (rule R6).
    let fallback = target
        .facts
        .time_zone
        .clone()
        .or_else(|| device_zone.map(|z| z.name().to_string()));
    let Some(event) = event_for_edit(&fresh, fallback.as_deref().unwrap_or("")) else {
        return Ok(EditLoad::Failed {
            message: match fallback {
                Some(_) => "Google sent this event without a start or end PM can read.".into(),
                None => time::TimeError::NoZone.to_string(),
            },
        });
    };
    let seen = seen_of(&target.calendar_id, &fresh);
    let session = keep_session(edit, target, fresh, master)?;
    Ok(EditLoad::Ready {
        session,
        event: Box::new(event),
        permissions,
        seen,
        series,
    })
}

/// The occurrence `fresh` as the editor opens it, and its series to keep. The occurrence is read
/// again after the series, so the editor's copy is always the later one: a change to the series
/// between the two reads shows in the occurrence but not in the kept series, and so meets a save of
/// "all events" as a conflict instead of slipping past it (R2). Without that second read, the series
/// isn't kept.
async fn open_series<W: Wire>(wire: &W, target: &Target, fresh: Value) -> (Value, Option<Value>) {
    let Some(master) = open_master(wire, target, &fresh).await else {
        return (fresh, None);
    };
    match get(wire, &target.remote_calendar, &target.remote_event).await {
        Fetched::Event(again) if !patch::is_gone(&again) => (again, Some(master)),
        _ => (fresh, None),
    }
}

/// The series of the occurrence `fresh`, as Google holds it now, or `None` for a one-off or a series
/// PM couldn't fetch.
async fn open_master<W: Wire>(wire: &W, target: &Target, fresh: &Value) -> Option<Value> {
    let id = fresh
        .get("recurringEventId")
        .and_then(Value::as_str)
        .filter(|id| is_plain_segment(id))?;
    match get(wire, &target.remote_calendar, id).await {
        Fetched::Event(master)
            if !patch::is_gone(&master) && master.get("id").and_then(Value::as_str) == Some(id) =>
        {
            Some(master)
        }
        _ => None,
    }
}

/// Where the occurrence `fresh` sits in its series `master`, or `None` for a series PM can't read.
fn series_info(fresh: &Value, master: &Value) -> Option<SeriesInfo> {
    let read = Master::read(&FreshEvent::from_google(master.clone())).ok()?;
    Some(SeriesInfo {
        first: is_first(&read, &FreshEvent::from_google(fresh.clone())),
        repeats: describe_lines(&read.recurrence, &read.start),
    })
}

/// Google's copy as a delete would compare it ([`patch::delete_still_matches`]): `parse_event`'s
/// start and end, the mirror's clipping of the title and place.
fn seen_of(calendar_id: &str, fresh: &Value) -> Option<SeenSummary> {
    let row = calendar::parse_event(calendar_id, fresh)?;
    Some(SeenSummary {
        summary: calendar::mirrored_summary(&row.summary),
        start: row.start,
        end: row.end,
        all_day: row.all_day,
        location: row.location.as_deref().map(calendar::mirrored_location),
    })
}

/// Save the fields the user changed. Built against a fresh copy, sent with its etag, and never over a
/// change Google made meanwhile to a field this save also sends. `scope` names which occurrences of
/// a repeating event it's for and `choice` the plan the user chose ([`series_choices`]); without a
/// scope it's one event, as before (a repeating one stays read-only until the dialog, C9b-1).
#[tauri::command]
pub async fn update_calendar_event(
    app: AppHandle,
    window: tauri::Window,
    session: String,
    draft: EventPatchDraft,
    scope: Option<SeriesScope>,
    choice: Option<String>,
) -> Result<WriteOutcome> {
    require_main_window(&window)?;
    require_owner(&app)?;
    if scope.is_some() && !cfg!(debug_assertions) {
        return Ok(WriteOutcome::Failed {
            message: SCOPED_NOT_YET.into(),
        });
    }
    let state = app.state::<AppState>();
    let Some(open) = open_session(&state.calendar_edit, &session)? else {
        return Ok(WriteOutcome::Failed {
            message: SESSION_ENDED.into(),
        });
    };
    #[cfg(debug_assertions)]
    {
        let series_wide = matches!(scope, Some(SeriesScope::Following | SeriesScope::All));
        let allowed = dev_allow::check(&*state.conn()?, &open.target.remote_calendar, series_wide)?;
        if let Err(message) = allowed {
            return Ok(WriteOutcome::Failed {
                message: message.into(),
            });
        }
    }
    let editing_choice = calendar_editing::choice(&*state.conn()?, &open.target.email)?;
    let editing = editing_status(editing_choice, &open.target.token_key);
    if let Some(scope) = scope {
        let token_key = open.target.token_key.clone();
        let target = open.target.clone();
        let wire = GoogleWire(&token_key);
        let ctx = SeriesWrite {
            wire: &wire,
            host: &app,
            target: &target,
            editing,
            missing_ok: cfg!(debug_assertions),
        };
        return Ok(save_series(&ctx, &session, open, draft, scope.into(), choice.as_deref()).await);
    }
    let Some(_flight) = InFlight::claim(&state.calendar_edit, &open.target.row_id) else {
        return Ok(WriteOutcome::Busy);
    };
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
            Err(google_io::NoAnswer::Unsent { why, .. }) => return Ok(unsent_outcome(why)),
            // No answer, or a sign-in refused after a server error: it may or may not have landed.
            // Look before saying either (review H1).
            Err(google_io::NoAnswer::Lost { .. }) => {
                return Ok(after_lost_answer(&app, target, &session, &body).await)
            }
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
/// during the hold, nothing is deleted. Other writes to the event answer Busy meanwhile. `scope` and
/// `choice` name the occurrences of a repeating event and the plan chosen, as for a save; a series
/// delete is planned before the hold, and planned again at the send.
#[tauri::command]
pub async fn delete_calendar_event(
    app: AppHandle,
    window: tauri::Window,
    event_id: String,
    seen: SeenSummary,
    scope: Option<SeriesScope>,
    choice: Option<String>,
) -> Result<DeleteStart> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let refused = |result| Ok(DeleteStart::Refused { result });
    if scope.is_some() && !cfg!(debug_assertions) {
        return refused(WriteOutcome::Failed {
            message: SCOPED_NOT_YET.into(),
        });
    }
    let state = app.state::<AppState>();
    let (target, editing_choice) = {
        let conn = state.conn()?;
        match resolve_target(&conn, &event_id)? {
            Ok(target) => {
                let editing_choice = calendar_editing::choice(&conn, &target.email)?;
                (target, editing_choice)
            }
            Err(reason) => return refused(WriteOutcome::ReadOnly { reason }),
        }
    };
    // Said now rather than after the wait; the rest of the gate needs Google's copy, and runs then.
    let editing = editing_status(editing_choice, &target.token_key);
    if editing != EditingStatus::On {
        return refused(WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::EditingOff,
        });
    }
    #[cfg(debug_assertions)]
    {
        let series_wide = matches!(scope, Some(SeriesScope::Following | SeriesScope::All));
        let allowed = dev_allow::check(&*state.conn()?, &target.remote_calendar, series_wide)?;
        if let Err(message) = allowed {
            return refused(WriteOutcome::Failed {
                message: message.into(),
            });
        }
    }
    if let Some(scope) = scope {
        return start_series_delete(
            app.clone(),
            target,
            event_id,
            seen,
            scope.into(),
            choice,
            editing,
        )
        .await;
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
        Vec::new(),
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
        hidden: Vec::new(),
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
    /// The rows it hides (a series delete), for a reloaded webview.
    hidden: Vec<String>,
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
        hidden: Vec<String>,
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
                    hidden,
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

    /// The deletes still waiting, for a webview that reloaded: the oldest first, as they were made,
    /// so the notices come back in that order and Ctrl+Z still undoes the newest.
    fn list(&self) -> Vec<HeldDeleteInfo> {
        let Ok(held) = self.0.lock() else {
            return Vec::new();
        };
        let now = Instant::now();
        let mut waiting: Vec<(&String, &Held)> = held.iter().collect();
        waiting.sort_by_key(|(_, h)| h.until);
        waiting
            .into_iter()
            .map(|(token, h)| HeldDeleteInfo {
                undo_token: token.clone(),
                event_id: h.event_id.clone(),
                summary: h.summary.clone(),
                seconds_left: h.until.saturating_duration_since(now).as_secs(),
                hidden: h.hidden.clone(),
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
            Err(google_io::NoAnswer::Unsent { why, .. }) => return unsent_outcome(why),
            // No answer, or a sign-in refused after a server error: look. Gone means it landed;
            // still there means it didn't (review H1).
            Err(google_io::NoAnswer::Lost { .. }) => {
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

// --- repeating events (C9) ---

/// Whether a series with guests may change: not until C12 brings Send / Don't send, whatever the
/// occurrence shows (C8b review G5).
const GUESTS_ALLOWED: bool = false;

/// Pages read of a series' exceptions (100 a page) and of its occurrences (250 a page) before PM
/// gives up and calls the listing incomplete.
const EXCEPTION_PAGES: usize = 10;
const OCCURRENCE_PAGES: usize = 20;

/// What the cross-check reads of each occurrence in the window ([`Exceptions::cross_checked`]).
const WINDOW_FIELDS: &str = "nextPageToken,items(id,status,recurringEventId,originalStartTime,\
                             start,end,summary,attendees(email,self,optional),attendeesOmitted)";

/// What a change to a repeating occurrence offers: "This event", "This and following" and "All
/// events", each planned now against Google's copies, so the dialog shows what each would do, or why
/// it can't (DS12). Only reads go to Google. `ChoiceRequest::Edit` names an open editor and its
/// draft, `Delete` a mirror row and what the user saw of it.
#[tauri::command]
pub async fn series_choices(
    app: AppHandle,
    window: tauri::Window,
    request: ChoiceRequest,
) -> Result<SeriesChoices> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let state = app.state::<AppState>();
    let (target, open) = match &request {
        ChoiceRequest::Edit { session, .. } => {
            let Some(open) = open_session(&state.calendar_edit, session)? else {
                return Ok(SeriesChoices::Refused {
                    result: WriteOutcome::Failed {
                        message: SESSION_ENDED.into(),
                    },
                });
            };
            (open.target.clone(), Some(open))
        }
        ChoiceRequest::Delete { event_id, .. } => {
            let resolved = resolve_target(&*state.conn()?, event_id)?;
            match resolved {
                Ok(target) => (target, None),
                Err(reason) => {
                    return Ok(SeriesChoices::Refused {
                        result: WriteOutcome::ReadOnly { reason },
                    })
                }
            }
        }
    };
    let (choice, mirror) = {
        let conn = state.conn()?;
        (
            calendar_editing::choice(&conn, &target.email)?,
            MirrorFacts::read(&conn, &target.calendar_id)?,
        )
    };
    let editing = editing_status(choice, &target.token_key);
    Ok(choices(
        &GoogleWire(&target.token_key),
        &target,
        editing,
        &mirror,
        &request,
        open.as_ref().map(EditSession::opened),
    )
    .await)
}

impl EditSession {
    /// What the user opened, as the choices check against it: the occurrence, and its series.
    fn opened(&self) -> (&Value, Option<&Value>) {
        (&self.base, self.master.as_ref())
    }
}

/// The mirror's part in a series' choices.
struct MirrorFacts {
    /// The series this calendar's occurrences belong to: a part split off earlier shows up as
    /// another with the same base id.
    series_ids: Vec<String>,
    /// The stretch PM mirrors (RFC 3339 `timeMin`, `timeMax`): the occurrences there are what a series'
    /// exceptions are checked against.
    window: (String, String),
}

impl MirrorFacts {
    fn read(conn: &Connection, calendar_id: &str) -> Result<MirrorFacts> {
        Ok(MirrorFacts {
            series_ids: calendar::series_ids(conn, calendar_id)?,
            window: calendar::time_window(conn)?,
        })
    }

    /// The window as instants, for telling which rows a re-fetch of it covers.
    fn instants(&self) -> Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|t| t.with_timezone(&chrono::Utc))
        };
        Some((at(&self.window.0)?, at(&self.window.1)?))
    }
}

/// Google's copies a change to a series is planned from, fetched together.
struct Series {
    master_fresh: FreshEvent,
    master: Master,
    /// Its exceptions as listed by iCal UID, in series order and cross-checked.
    exceptions: Exceptions,
    /// All its occurrences, for a rule with a COUNT (A7).
    counted: Option<Listing>,
    /// Another part of it was split off earlier: in Google (`_R`) or by PM (`pmSplitOf`).
    sibling: bool,
    /// For the dev report: whether the listing ran to its end before the cross-check, and the
    /// occurrences it was checked against.
    #[cfg(debug_assertions)]
    listing_complete: bool,
    #[cfg(debug_assertions)]
    window: Listing,
}

/// The series of the occurrence `fresh`, read through `wire`: `Ok(None)` for a one-off.
/// `whole` reads the window's occurrences whole (the dev report's comparisons need every field);
/// otherwise only what the cross-check reads, so a long description can't push a page past the
/// reply cap.
async fn gather<W: Wire>(
    wire: &W,
    target: &Target,
    fresh: &Value,
    mirror: &MirrorFacts,
    whole: bool,
) -> std::result::Result<Option<Series>, WriteOutcome> {
    let Some(master_id) = fresh.get("recurringEventId").and_then(Value::as_str) else {
        // A one-off, or a row that has since become a series' master in Google (a rule added there):
        // saving it unscoped would change every occurrence with no warning (C9 design §0.7).
        return match fresh.get("recurrence") {
            None => Ok(None),
            Some(_) => Err(WriteOutcome::Failed {
                message: "This became a repeating event in Google. Refresh, then open one of its \
                          occurrences."
                    .into(),
            }),
        };
    };
    let unreadable = || WriteOutcome::Failed {
        message: SeriesRefusal::Unreadable.to_string(),
    };
    if !is_plain_segment(master_id) {
        return Err(unreadable());
    }
    let master_json = get(wire, &target.remote_calendar, master_id)
        .await
        .event()?;
    if patch::is_gone(&master_json) {
        return Err(WriteOutcome::Gone);
    }
    let master_fresh = FreshEvent::from_google(master_json);
    let master = Master::read(&master_fresh).map_err(|e| WriteOutcome::Failed {
        message: e.to_string(),
    })?;
    if master.id != master_id {
        return Err(unreadable());
    }
    if let Some(reason) = master.read_only() {
        return Err(WriteOutcome::ReadOnly { reason });
    }
    if master.guests && !GUESTS_ALLOWED {
        return Err(WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::HasGuests,
        });
    }
    let calendar = target.remote_calendar.as_str();
    let listed = match master_fresh.json().get("iCalUID").and_then(Value::as_str) {
        Some(ical) => {
            list_pages(
                wire,
                |page: Option<&str>| plan::list_by_ical(calendar, ical, page),
                EXCEPTION_PAGES,
            )
            .await
        }
        None => Listed {
            items: Vec::new(),
            complete: false,
            gone: false,
        },
    };
    let (time_min, time_max) = (mirror.window.0.as_str(), mirror.window.1.as_str());
    let window = list_pages(
        wire,
        |page: Option<&str>| {
            let mut query = vec![("timeMin", time_min), ("timeMax", time_max)];
            if whole {
                query.push(("maxResults", "100"));
            } else {
                query.push(("maxResults", "250"));
                query.push(("fields", WINDOW_FIELDS));
            }
            query.extend(page.map(|p| ("pageToken", p)));
            plan::instances(calendar, master_id, &query)
        },
        OCCURRENCE_PAGES,
    )
    .await;
    let window = Listing {
        items: window.items,
        complete: window.complete,
    };
    #[cfg(debug_assertions)]
    let listing_complete = listed.complete;
    let exceptions = Exceptions::sorted(listed.items, listed.complete).cross_checked(
        master_fresh.json(),
        &master,
        &window,
    );
    // Google's count of what comes before a split, for a rule that ends after so many (A7): every
    // occurrence, unwindowed, only the fields counting reads.
    let has_count = lines::series_rule(&lines::parse(&master.recurrence))
        .is_ok_and(|(_, facts)| facts.count.is_some());
    let counted = if has_count {
        let all = list_pages(
            wire,
            |page: Option<&str>| {
                let mut query = vec![
                    ("maxResults", "250"),
                    ("fields", "nextPageToken,items(id,status,originalStartTime)"),
                ];
                query.extend(page.map(|p| ("pageToken", p)));
                plan::instances(calendar, master_id, &query)
            },
            OCCURRENCE_PAGES,
        )
        .await;
        Some(Listing {
            items: all.items,
            complete: all.complete,
        })
    } else {
        None
    };
    // Google names the later part of its own split `<id>_R<date>`, so a master with that id is one,
    // whether or not the earlier part is still in the mirror's window.
    let base = |id: &str| id.split("_R").next().unwrap_or(id).to_string();
    let sibling = master.id.contains("_R")
        || mirror
            .series_ids
            .iter()
            .any(|id| *id != master.id && base(id) == base(&master.id))
        || master_fresh
            .json()
            .pointer("/extendedProperties/private/pmSplitOf")
            .is_some();
    Ok(Some(Series {
        master_fresh,
        master,
        exceptions,
        counted,
        sibling,
        #[cfg(debug_assertions)]
        listing_complete,
        #[cfg(debug_assertions)]
        window,
    }))
}

/// [`series_choices`] past its shell: Google through `wire`, the mirror's part in `mirror`, and for
/// an edit `opened`, the occurrence and the series as the editor opened them.
async fn choices<W: Wire>(
    wire: &W,
    target: &Target,
    editing: EditingStatus,
    mirror: &MirrorFacts,
    request: &ChoiceRequest,
    opened: Option<(&Value, Option<&Value>)>,
) -> SeriesChoices {
    let planning = match prepare_change(wire, target, editing, mirror, request, opened).await {
        Prepared::Ready(planning) => planning,
        Prepared::NotRepeating => return SeriesChoices::NotRepeating,
        Prepared::Gone(_) => {
            return SeriesChoices::Refused {
                result: WriteOutcome::Gone,
            }
        }
        Prepared::Refused(result) => return SeriesChoices::Refused { result },
    };
    let row = |scope: Scope| -> ScopeRow {
        // A "this and following" preview gets a new series id of its own, never sent: the save
        // mints one again (one plan, one tail id), and the token leaves it out.
        let tail = plan::client_event_id();
        match plan_scope(&planning, &target.remote_calendar, request, scope, &tail) {
            Planned::NotOffered(why) => ScopeRow::Refused {
                message: why.to_string(),
            },
            Planned::Planned(planned) => {
                scope_row(planned, scope, &planning.bind, &planning.series)
            }
        }
    };
    SeriesChoices::Ready {
        this: row(Scope::This),
        following: row(Scope::Following),
        all: row(Scope::All),
    }
}

/// What a change to one occurrence is planned from, fetched and checked once: the choices and the
/// save plan from the same inputs, so a save refuses alike and its token matches.
struct Planning {
    /// Google's copy of the occurrence now.
    instance: FreshEvent,
    /// What the editor opened (an edit), or Google's copies (a delete has only what the user saw).
    opened_instance: FreshEvent,
    opened_master: Option<FreshEvent>,
    series: Series,
    change: Change,
    /// What the token binds: the session and the draft (an edit), or the row and what the user saw
    /// (a delete).
    bind: String,
}

impl Planning {
    fn input<'a>(&'a self, calendar: &'a str) -> SeriesInput<'a> {
        SeriesInput {
            calendar,
            opened: Opened {
                instance: &self.opened_instance,
                master: self.opened_master.as_ref(),
            },
            fresh: Fresh {
                instance: &self.instance,
                master: &self.series.master_fresh,
            },
            exceptions: &self.series.exceptions,
            instances: self.series.counted.as_ref(),
            sibling: self.series.sibling,
            guests_allowed: GUESTS_ALLOWED,
        }
    }
}

/// What preparing a change came to.
enum Prepared {
    Ready(Box<Planning>),
    NotRepeating,
    /// The occurrence is gone from Google: its cancelled copy, when Google sent one.
    Gone(Option<Value>),
    Refused(WriteOutcome),
}

/// One scope, as its row would offer it.
enum Planned {
    /// Not a choice this change has ([`scope_options`]).
    NotOffered(ScopeReason),
    Planned(std::result::Result<SeriesPlan, NotPlanned>),
}

/// Plan `scope` as its row offers it. `tail_id`: a split's new series id.
fn plan_scope(
    planning: &Planning,
    calendar: &str,
    request: &ChoiceRequest,
    scope: Scope,
    tail_id: &str,
) -> Planned {
    let options = scope_options(
        &planning.change,
        is_first(&planning.series.master, &planning.instance),
        text_only(&planning.series.master).as_ref(),
    );
    let offer = match scope {
        Scope::This => options.this,
        Scope::Following => options.following,
        Scope::All => options.all,
    };
    if let Row::Refused(why) = offer {
        return Planned::NotOffered(why);
    }
    let input = planning.input(calendar);
    Planned::Planned(match request {
        ChoiceRequest::Edit { draft, .. } => {
            plan_series_edit(scope, &input, draft, tail_id, Notify::None)
        }
        ChoiceRequest::Delete { seen, .. } => plan_series_delete(scope, &input, seen, Notify::None),
    })
}

/// Fetch and check what a change is planned from: the occurrence, the gate on it, what the user saw
/// (a delete), and its series.
async fn prepare_change<W: Wire>(
    wire: &W,
    target: &Target,
    editing: EditingStatus,
    mirror: &MirrorFacts,
    request: &ChoiceRequest,
    opened: Option<(&Value, Option<&Value>)>,
) -> Prepared {
    let refused = Prepared::Refused;
    // An edit is checked against what the user opened, never against Google's copies alone.
    let opened = match (request, opened) {
        (ChoiceRequest::Edit { .. }, None) => {
            return refused(WriteOutcome::Failed {
                message: SESSION_ENDED.into(),
            })
        }
        (ChoiceRequest::Edit { .. }, Some(opened)) => Some(opened),
        (ChoiceRequest::Delete { .. }, _) => None,
    };
    let fresh = match get(wire, &target.remote_calendar, &target.remote_event).await {
        Fetched::Event(fresh) if patch::is_gone(&fresh) => return Prepared::Gone(Some(fresh)),
        Fetched::Gone => return Prepared::Gone(None),
        fetched => match fetched.event() {
            Ok(fresh) => fresh,
            Err(result) => return refused(result),
        },
    };
    // The gate on Google's copy, all but its stand-in for this very choice: a repeating event is
    // read-only only until the dialog asks which occurrences (R9).
    let mut facts = facts_from_fresh(&fresh, &target.facts, editing);
    facts.recurring = false;
    let perms = gate::edit_rights(&facts);
    let closed = match request {
        ChoiceRequest::Edit { draft, .. } => refused_field(&perms, draft),
        ChoiceRequest::Delete { .. } => (!perms.delete).then(|| {
            perms
                .reasons
                .first()
                .copied()
                .unwrap_or(ReadOnlyReason::CalendarReadOnly)
        }),
    };
    if let Some(reason) = closed {
        return refused(WriteOutcome::ReadOnly { reason });
    }
    if let ChoiceRequest::Delete { seen, .. } = request {
        match patch::delete_still_matches(seen, &fresh) {
            DeleteCheck::Matches => {}
            DeleteCheck::Gone => return Prepared::Gone(Some(fresh)),
            DeleteCheck::Changed(fields) => return refused(WriteOutcome::Conflict { fields }),
        }
    }
    let series = match gather(wire, target, &fresh, mirror, false).await {
        Ok(Some(series)) => series,
        Ok(None) => return Prepared::NotRepeating,
        Err(result) => return refused(result),
    };
    let instance = FreshEvent::from_google(fresh);
    // An edit checks against what the user opened; the series only if it's this one (C9 design
    // review L13). A delete has only what the user saw, checked above and by its planner.
    let (opened_instance, opened_master) = match opened {
        Some((base, master)) => (
            FreshEvent::from_google(base.clone()),
            master
                .filter(|m| m.get("id").and_then(Value::as_str) == Some(series.master.id.as_str()))
                .map(|m| FreshEvent::from_google(m.clone())),
        ),
        None => (instance.clone(), Some(series.master_fresh.clone())),
    };
    let (change, bind) = match request {
        ChoiceRequest::Edit { session, draft } => {
            match Change::of(draft, &instance, &series.master) {
                Ok(change) => (change, format!("{session}\u{1f}{draft:?}")),
                Err(e) => {
                    return refused(WriteOutcome::Failed {
                        message: e.to_string(),
                    })
                }
            }
        }
        ChoiceRequest::Delete { event_id, seen } => {
            (Change::delete(), format!("{event_id}\u{1f}{seen:?}"))
        }
    };
    Prepared::Ready(Box::new(Planning {
        instance,
        opened_instance,
        opened_master,
        series,
        change,
        bind,
    }))
}

/// A row as the dialog shows it: the plan's warnings and its token, or why there's none. Every
/// refusal is the planners' own, so planning again at send refuses alike.
fn scope_row(
    planned: std::result::Result<SeriesPlan, NotPlanned>,
    asked: Scope,
    bind: &str,
    series: &Series,
) -> ScopeRow {
    let message = |text: &str| ScopeRow::Refused {
        message: text.into(),
    };
    match planned {
        Ok(plan) => {
            let warnings = plan
                .warnings
                .iter()
                .map(|w| warning_dto(w, &plan, series))
                .collect();
            let token = plan.choice_digest(bind);
            if asked == Scope::Following && plan.scope == Scope::All {
                ScopeRow::AsAll { warnings, token }
            } else {
                ScopeRow::Offered { warnings, token }
            }
        }
        Err(NotPlanned::Conflict(fields)) => ScopeRow::Conflict { fields },
        Err(NotPlanned::Refused(why)) => ScopeRow::Refused {
            message: why.to_string(),
        },
        Err(NotPlanned::NoChange) => {
            message("Google already has this, so there's nothing to save.")
        }
        Err(NotPlanned::Gone) => message("This event is no longer in Google."),
        Err(NotPlanned::ReadOnly(reason)) => message(read_only_message(reason)),
    }
}

/// A plan's warning with what the dialog needs to word it. Dates are in the series' zone.
fn warning_dto(warning: &SeriesWarning, plan: &SeriesPlan, series: &Series) -> SeriesWarningDto {
    let date = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
    match *warning {
        SeriesWarning::SplitSibling => SeriesWarningDto::SplitSibling,
        SeriesWarning::ExceptionsMayReset { count, complete } => {
            SeriesWarningDto::ExceptionsMayReset { count, complete }
        }
        SeriesWarning::StartMoved { from, to } => SeriesWarningDto::StartMoved {
            from: date(from),
            to: date(to),
        },
        SeriesWarning::RepeatDayChanges { from, to } => SeriesWarningDto::RepeatDayChanges {
            from: date(from),
            to: date(to),
            before: day_phrase(&series.master.recurrence, &series.master.start),
            after: plan
                .new_rule(&series.master.recurrence)
                .and_then(|(lines, start)| day_phrase(&lines, &start)),
            shift_days: (to - from).num_days(),
        },
        SeriesWarning::LengthChanges => SeriesWarningDto::LengthChanges,
        SeriesWarning::NotCarried {
            attachments,
            conference,
            label,
        } => SeriesWarningDto::NotCarried {
            attachments,
            conference,
            label,
        },
        SeriesWarning::NewIdentity => SeriesWarningDto::NewIdentity,
    }
}

// --- sending a change to a repeating event (C9a-3a) ---

const NOT_REPEATING: &str =
    "This event doesn't repeat, so there are no occurrences to choose from. Save it without choosing.";
const NOT_REPEATING_DELETE: &str = "This event doesn't repeat, so there are no occurrences to \
                                    choose from. Delete it without choosing.";
/// C9a-3a sends no plan that makes a new series; C9a-3b removes this with the split.
const SPLIT_NOT_YET: &str = "PM can't yet change this and the following occurrences on their \
                             own. Choose \u{201c}This event\u{201d} or \u{201c}All events\u{201d}.";
/// Until the dialog that asks which occurrences arrives (C9b-1), release builds send no change to
/// more than one event; dev builds do, for the live checks.
const SCOPED_NOT_YET: &str = "Changing repeating events from PM isn't available yet.";
const ONE_GONE: &str = "That occurrence was deleted in Google, so nothing else was changed.";
const OFFLINE: &str =
    "PM couldn't reach Google. Nothing was changed; try again when you're back online.";
const NOT_SENT: &str = "PM couldn't get this account's Google sign-in ready, so nothing was sent \
                        and nothing changed. Try again in a moment.";
const PUT_BACK: &str = "PM put the series' times and repeat back as they were before this save. \
                        Anything else in this save may have gone through; PM's calendar shows it \
                        after the next refresh. Open the event again to change it.";
const RETIME_UNSURE: &str = "PM lost touch with Google while moving the series, so it may have \
                             moved, or moved only partly. Check it in Google Calendar.";

/// Whether the save names the plan the user chose. A missing choice passes only in dev builds
/// (the devtools probes); in release it's asked again (review scope-M2).
fn choice_matches(choice: Option<&str>, token: &str, missing_ok: bool) -> bool {
    match choice {
        Some(choice) => choice == token,
        None => missing_ok,
    }
}

/// What a write that wasn't proved done means for the user.
fn not_done_outcome(why: Why, editing: EditingStatus) -> WriteOutcome {
    match why {
        Why::Ignored => WriteOutcome::IgnoredByGoogle { split: false },
        Why::Conflict(fields) => WriteOutcome::Conflict { fields },
        Why::Gone => WriteOutcome::Gone,
        Why::Refused(verdict) => outcome_for(verdict, editing),
        Why::Unsent(Unsent::SignIn) => WriteOutcome::Reauth,
        Why::Unsent(Unsent::Offline) => WriteOutcome::Failed {
            message: OFFLINE.into(),
        },
        Why::Unsent(Unsent::Local) => WriteOutcome::Failed {
            message: NOT_SENT.into(),
        },
        Why::PutBack => WriteOutcome::Failed {
            message: PUT_BACK.into(),
        },
        // Cleanup steps only, never in a run that didn't land.
        Why::NotFound => WriteOutcome::Unconfirmed,
    }
}

/// Google's answer as a machine reads it. Only a 2xx body is the event; any other is Google's error.
fn answer_of(sent: Sent) -> Answer {
    match sent {
        Ok(reply) => Answer::Reply {
            body: (reply.verdict == Verdict::Ok)
                .then(|| serde_json::from_str::<Value>(&reply.body).ok())
                .flatten(),
            verdict: reply.verdict,
            date: reply.date,
            maybe_applied: reply.maybe_applied,
        },
        Err(google_io::NoAnswer::Lost { retried, .. }) => Answer::Lost { retried },
        Err(google_io::NoAnswer::Unsent { why, .. }) => Answer::Unsent(why),
    }
}

/// Run `machine` against Google through `wire` to its end. Borrowed, so the plan can be read back
/// for landing.
async fn drive<W: Wire, M: Machine + Send>(wire: &W, machine: &mut M) -> M::Output {
    loop {
        match machine.next() {
            Next::Send(request) => {
                let answer = answer_of(wire.send(&request).await);
                machine.answer(answer);
            }
            Next::Wait(pause) => wire.wait(pause).await,
            Next::Over(out) => return out,
        }
    }
}

/// What one series write works with: Google through `wire`, the mirror and the claims through
/// `host`, the event, the account's editing status, and whether a save may leave out its choice
/// (`missing_ok`, dev builds; scope-M2).
struct SeriesWrite<'a, W, H> {
    wire: &'a W,
    host: &'a H,
    target: &'a Target,
    editing: EditingStatus,
    missing_ok: bool,
}

/// Who a series write lands for: the editor that saved it (its session), or a held delete.
enum Landing<'a> {
    Edit { session: &'a str },
    Delete,
}

/// Plan the claimed scope again, as the dialog's row did, and refuse what a row would.
fn planned_or_refused<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    planning: &Planning,
    request: &ChoiceRequest,
    scope: Scope,
) -> std::result::Result<SeriesPlan, WriteOutcome> {
    let tail = plan::client_event_id();
    match plan_scope(planning, &ctx.target.remote_calendar, request, scope, &tail) {
        Planned::NotOffered(why) => Err(WriteOutcome::Failed {
            message: why.to_string(),
        }),
        Planned::Planned(Ok(plan)) => Ok(plan),
        Planned::Planned(Err(NotPlanned::Conflict(fields))) => {
            Err(WriteOutcome::Conflict { fields })
        }
        Planned::Planned(Err(NotPlanned::Gone)) => Err(land_gone(ctx.host, ctx.target, None)),
        Planned::Planned(Err(NotPlanned::NoChange)) => Err(WriteOutcome::NoChange),
        Planned::Planned(Err(NotPlanned::ReadOnly(reason))) => {
            Err(WriteOutcome::ReadOnly { reason })
        }
        Planned::Planned(Err(NotPlanned::Refused(why))) => Err(WriteOutcome::Failed {
            message: why.to_string(),
        }),
    }
}

/// The series' mirror facts, the change prepared and checked against the claimed series (L13):
/// everything a send plans from.
async fn prepared_for<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    request: &ChoiceRequest,
    opened: Option<(&Value, Option<&Value>)>,
    claimed: Option<&str>,
    scope: Scope,
) -> std::result::Result<(MirrorFacts, Box<Planning>), WriteOutcome> {
    let mirror = ctx
        .host
        .mirror(&ctx.target.calendar_id)
        .map_err(|e| WriteOutcome::Failed {
            message: e.to_string(),
        })?;
    let not_repeating = match request {
        ChoiceRequest::Edit { .. } => NOT_REPEATING,
        ChoiceRequest::Delete { .. } => NOT_REPEATING_DELETE,
    };
    let planning =
        match prepare_change(ctx.wire, ctx.target, ctx.editing, &mirror, request, opened).await {
            Prepared::Ready(planning) => planning,
            Prepared::NotRepeating => {
                return Err(WriteOutcome::Failed {
                    message: not_repeating.into(),
                })
            }
            Prepared::Gone(cancelled) => {
                let gone = land_gone(ctx.host, ctx.target, cancelled.as_ref());
                return Err(if scope == Scope::This {
                    gone
                } else {
                    WriteOutcome::Failed {
                        message: ONE_GONE.into(),
                    }
                });
            }
            Prepared::Refused(result) => return Err(result),
        };
    // The series claimed before anything was read must be the one Google holds now, or two writes
    // could hold different claims for one series.
    if claimed != Some(planning.series.master.id.as_str()) {
        return Err(WriteOutcome::Failed {
            message: SeriesRefusal::NoBase.to_string(),
        });
    }
    Ok((mirror, planning))
}

/// Save a draft for the occurrences `scope` names: claim the series and the row, plan again from
/// fresh copies, check the plan is the one chosen (`choice`), send it step by step, land it.
async fn save_series<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    session: &str,
    open: EditSession,
    draft: EventPatchDraft,
    scope: Scope,
    choice: Option<&str>,
) -> WriteOutcome {
    // The series is known from the occurrence as opened, so the claim comes before any read (L2).
    let Some(master_id) = open
        .base
        .get("recurringEventId")
        .and_then(Value::as_str)
        .filter(|id| is_plain_segment(id))
        .map(str::to_string)
    else {
        return WriteOutcome::Failed {
            message: NOT_REPEATING.into(),
        };
    };
    let keys = [
        series_key(&ctx.target.remote_calendar, &master_id),
        ctx.target.row_id.clone(),
    ];
    let Some(_flight) = InFlight::claim_all(ctx.host.edit(), &keys) else {
        return WriteOutcome::Busy;
    };
    let request = ChoiceRequest::Edit {
        session: session.to_string(),
        draft,
    };
    let (mirror, planning) =
        match prepared_for(ctx, &request, Some(open.opened()), Some(&master_id), scope).await {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
    let plan = match planned_or_refused(ctx, &planning, &request, scope) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    // Before the token: a split is refused whatever was chosen, and nothing but reads has gone out.
    if plan.steps.iter().any(|s| matches!(s.op, Op::Insert { .. })) {
        return WriteOutcome::Failed {
            message: SPLIT_NOT_YET.into(),
        };
    }
    if !choice_matches(choice, &plan.choice_digest(&planning.bind), ctx.missing_ok) {
        return WriteOutcome::NeedsChoice;
    }
    let series_wide = plan.scope != Scope::This;
    let outcome = send_series(ctx, &mirror, &planning, plan, Landing::Edit { session }).await;
    // After a change past this occurrence, the editor's copies no longer stand for Google's.
    let ended = matches!(outcome, WriteOutcome::Saved { .. })
        || matches!(&outcome, WriteOutcome::Failed { message } if message == PUT_BACK);
    if series_wide && ended {
        end_session(ctx.host.edit(), session);
    }
    outcome
}

/// Snapshot, run, land: the edit's and the held delete's send.
async fn send_series<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    mirror: &MirrorFacts,
    planning: &Planning,
    plan: SeriesPlan,
    landing: Landing<'_>,
) -> WriteOutcome {
    // Never a plan that makes a new series, whichever way it came (C9a-3b sends those).
    if plan.steps.iter().any(|s| matches!(s.op, Op::Insert { .. })) {
        return WriteOutcome::Failed {
            message: SPLIT_NOT_YET.into(),
        };
    }
    // The series' rows before the first send, so a sync that lands mid-run can't hide which rows
    // are new (review M4).
    let snapshot = match ctx.host.with_conn(|conn| {
        calendar::series_rows(conn, &ctx.target.calendar_id, &planning.series.master.id)
    }) {
        Ok(rows) => rows,
        Err(e) => {
            return WriteOutcome::Failed {
                message: e.to_string(),
            }
        }
    };
    let mut run = match Run::new(plan) {
        Ok(run) => run,
        Err(Unfit) => {
            return WriteOutcome::Failed {
                message: "PM built a change it isn't allowed to send, so nothing was saved.".into(),
            }
        }
    };
    let ran = drive(ctx.wire, &mut run).await;
    land_series(ctx, mirror, planning, run.plan(), ran, snapshot, landing).await
}

/// End an editor's session: its copies no longer stand for Google's.
fn end_session(edit: &CalendarEditState, session: &str) {
    if let Ok(mut sessions) = edit.sessions.lock() {
        sessions.remove(session);
    }
}

/// `now`'s rows, and the snapshot's that `now` lacks (a sync since may have taken them off).
fn rows_union(snapshot: &[CalendarEvent], now: Vec<CalendarEvent>) -> Vec<CalendarEvent> {
    let mut rows = now;
    let extra: Vec<CalendarEvent> = snapshot
        .iter()
        .filter(|s| !rows.iter().any(|r| r.id == s.id))
        .cloned()
        .collect();
    rows.extend(extra);
    rows
}

/// An HTTP `Date` a second on, as RFC 3339: it is whole seconds, and a floor must not be older than
/// the write it stands for.
fn date_floor(date: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    date.map(|d| {
        (d + chrono::TimeDelta::seconds(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    })
}

fn updated_of(copy: Option<&Value>) -> Option<String> {
    copy.and_then(|c| c.get("updated"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `updated` of the series' copy and of each occurrence listed as changed on its own, which
/// doesn't move its series' (A29): what a removal of every row of the series is floored at.
fn series_moments(planning: &Planning) -> Vec<String> {
    std::iter::once(planning.series.master_fresh.json())
        .chain(planning.series.exceptions.items.iter())
        .filter_map(|v| updated_of(Some(v)))
        .collect()
}

/// The pauses before the second and third re-fetch of a series that doesn't show the write yet.
const REFETCH_AGAIN: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

/// The series re-fetched over the mirror's window, whole (rows are parsed from them), 50 a page,
/// so a long description repeated on each occurrence stays under the reply cap. `absent_ok`: the
/// plan expects the series gone, so a 404 or 410 reads as an empty, whole listing; otherwise it's
/// incomplete (review M7).
async fn refetch<W: Wire>(
    wire: &W,
    calendar: &str,
    series: &str,
    window: &(String, String),
    absent_ok: bool,
) -> Listing {
    let listed = list_pages(
        wire,
        |page: Option<&str>| {
            let mut query = vec![
                ("timeMin", window.0.as_str()),
                ("timeMax", window.1.as_str()),
                ("maxResults", "50"),
            ];
            query.extend(page.map(|p| ("pageToken", p)));
            plan::instances(calendar, series, &query)
        },
        OCCURRENCE_PAGES,
    )
    .await;
    if listed.gone && absent_ok {
        return Listing {
            items: Vec::new(),
            complete: true,
        };
    }
    Listing {
        items: listed.items,
        complete: listed.complete,
    }
}

/// What a run came to, in the mirror and in words.
async fn land_series<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    mirror: &MirrorFacts,
    planning: &Planning,
    plan: &SeriesPlan,
    ran: Ran,
    snapshot: Vec<CalendarEvent>,
    landing: Landing<'_>,
) -> WriteOutcome {
    let (host, target) = (ctx.host, ctx.target);
    let cal = target.calendar_id.as_str();
    let master = planning.series.master.id.as_str();
    let zone = planning.series.master.start.zone();
    let skipped = ran.skipped();
    let ends = match ran {
        Ran::Done { ends } => ends,
        Ran::NotDone {
            why: Why::Gone,
            rollback: Rollback::Nothing,
            ..
        } => {
            if plan.scope == Scope::This {
                return land_gone(host, target, None);
            }
            // The series itself is gone: every row of it goes, no older than its last copies.
            let floor = diff::floor_of(series_moments(planning).iter().map(String::as_str));
            let _ = host.with_conn(|conn| {
                let before = rows_union(&snapshot, calendar::series_rows(conn, cal, master)?);
                let gone = Removes::Series {
                    series: master.to_string(),
                }
                .rows(cal, &before, zone, floor.as_deref());
                record(conn, host.edit(), cal, Vec::new(), &[], gone)
            });
            host.landed(cal);
            return WriteOutcome::Gone;
        }
        Ran::NotDone {
            why,
            rollback: Rollback::Nothing,
            ..
        } => {
            if why == Why::PutBack {
                // The text half may have landed: the next sync rewrites the rows (review M6).
                drop_hash(host, cal);
                host.landed(cal);
            }
            return not_done_outcome(why, ctx.editing);
        }
        // No C9a-3a plan has an undo but the put-back, which reads as PutBack: defensively unknown.
        Ran::NotDone { .. } => {
            drop_hash(host, cal);
            return WriteOutcome::Unconfirmed;
        }
        Ran::Unknown { step, .. } => {
            drop_hash(host, cal);
            let moving = plan
                .steps
                .get(step)
                .is_some_and(|s| matches!(s.undo, Some(Undo::RestoreMaster { .. })));
            return if moving {
                WriteOutcome::Failed {
                    message: RETIME_UNSURE.into(),
                }
            } else {
                WriteOutcome::Unconfirmed
            };
        }
    };
    let (reply, date) = match ends.first() {
        Some(End::Done { reply, date }) => (reply.clone(), *date),
        _ => (None, None),
    };
    // "This event": Google's copy after the write is the row (design §0.5), no re-fetch.
    if plan.scope == Scope::This {
        return match (&landing, &plan.steps[0].op) {
            (Landing::Edit { session }, Op::Patch { body, .. }) => match reply {
                Some(saved) => land_saved(host, target, session, saved, body),
                None => saved_pending(host, target),
            },
            _ => {
                let floor = diff::floor_of(
                    [
                        updated_of(reply.as_ref()),
                        date_floor(date),
                        updated_of(Some(planning.instance.json())),
                    ]
                    .iter()
                    .flatten()
                    .map(String::as_str),
                );
                let landed = host.with_conn(|conn| {
                    let before = rows_union(&snapshot, calendar::series_rows(conn, cal, master)?);
                    let gone = plan.removes.rows(cal, &before, zone, floor.as_deref());
                    record(conn, host.edit(), cal, Vec::new(), &[], gone)
                });
                match landed {
                    Ok(()) => {
                        host.landed(cal);
                        WriteOutcome::Saved {
                            warnings: Vec::new(),
                            left_behind: Vec::new(),
                        }
                    }
                    Err(_) => saved_pending(host, target),
                }
            }
        };
    }
    let left_behind: Vec<String> = plan
        .left_behind(&skipped)
        .iter()
        .map(|p| p.day_in(zone).format("%Y-%m-%d").to_string())
        .collect();
    let edits_master = matches!(plan.steps[0].op, Op::Patch { .. }) && plan.scope == Scope::All;
    let mut expect = plan.expect_after(&skipped);
    let floor = if edits_master {
        let after = updated_of(reply.as_ref());
        if let Some(after) = &after {
            expect.push(Expect::Updated {
                series: master.to_string(),
                after: after.clone(),
            });
        }
        after
    } else if plan.scope == Scope::All {
        // A whole series deleted (a 204, no body): no older than its copies and the answer's own
        // time (A29).
        let mut moments = series_moments(planning);
        moments.extend(updated_of(reply.as_ref()));
        moments.extend(date_floor(date));
        diff::floor_of(moments.iter().map(String::as_str))
    } else {
        updated_of(reply.as_ref())
    };
    // A change to the series' text or times whose answer held no `updated` can't be told from a
    // list that lags behind it.
    let no_updated = edits_master && floor.is_none();
    let absent_ok = expect
        .iter()
        .any(|e| matches!(e, Expect::Absent { series } if series == master));
    // A plan that gives occurrences new ids needs the whole series back, or old and new rows would
    // both show (review M8).
    let moves_ids = edits_master
        && matches!(&plan.steps[0].op,
            Op::Patch { body, .. } if body.contains_key("start") || body.contains_key("recurrence"));
    let removal = |floor: Option<String>| Removal {
        plan,
        skipped: &skipped,
        snapshot: &snapshot,
        zone,
        floor,
    };
    let Some(window) = mirror.instants() else {
        return stale_saved(host, cal, removal(floor), left_behind);
    };
    let mut listing = Listing::default();
    let mut stale = true;
    for pause in [None, Some(REFETCH_AGAIN[0]), Some(REFETCH_AGAIN[1])] {
        if let Some(pause) = pause {
            ctx.wire.wait(pause).await;
        }
        listing = refetch(
            ctx.wire,
            &target.remote_calendar,
            master,
            &mirror.window,
            absent_ok,
        )
        .await;
        let probe = diff::diff_series(
            cal,
            &snapshot,
            &[Refetch {
                series: master,
                listing: &listing,
            }],
            window,
            &expect,
            zone,
            floor.as_deref(),
        );
        stale = no_updated || probe.stale || (moves_ids && !listing.complete);
        if !stale {
            break;
        }
    }
    if stale {
        return stale_saved(host, cal, removal(floor), left_behind);
    }
    let landed = host.with_conn(|conn| {
        let before = rows_union(&snapshot, calendar::series_rows(conn, cal, master)?);
        let diff = diff::diff_series(
            cal,
            &before,
            &[Refetch {
                series: master,
                listing: &listing,
            }],
            window,
            &expect,
            zone,
            floor.as_deref(),
        );
        let gone = plan
            .removes_for(&diff, &skipped)
            .rows(cal, &before, zone, floor.as_deref());
        let created = diff.created_since(&snapshot);
        let mut deletes = diff.deletes;
        for row in gone {
            if !deletes.iter().any(|d| d.row_id == row.row_id) {
                deletes.push(row);
            }
        }
        let upserts = diff.upserts;
        deletes.retain(|d| !upserts.iter().any(|u| u.id == d.row_id));
        record(conn, host.edit(), cal, upserts, &created, deletes)
    });
    match landed {
        Ok(()) => {
            host.landed(cal);
            series_saved(Vec::new(), left_behind)
        }
        Err(_) => {
            drop_hash(host, cal);
            series_saved(vec![WriteWarning::MirrorRefreshPending], left_behind)
        }
    }
}

/// `Saved`, with `LeftOnOldSeries` when occurrences were left behind.
fn series_saved(mut warnings: Vec<WriteWarning>, left_behind: Vec<String>) -> WriteOutcome {
    if !left_behind.is_empty() {
        warnings.push(WriteWarning::LeftOnOldSeries);
    }
    WriteOutcome::Saved {
        warnings,
        left_behind,
    }
}

/// What a run removes from the mirror once it's over: the plan's own removals, but the cancels in
/// `skipped`, from the rows of `snapshot` and now, each no older than `floor`.
struct Removal<'a> {
    plan: &'a SeriesPlan,
    skipped: &'a [usize],
    snapshot: &'a [CalendarEvent],
    zone: chrono_tz::Tz,
    floor: Option<String>,
}

/// Saved at Google, but no re-fetch shows it yet (A14): nothing from the re-fetch is recorded, the
/// plan's own proved removals still land (so a lagging sync can't bring them back), and the next
/// sync rewrites the rest.
fn stale_saved(
    host: &impl Host,
    cal: &str,
    removal: Removal<'_>,
    left_behind: Vec<String>,
) -> WriteOutcome {
    let stale = SeriesDiff {
        upserts: Vec::new(),
        deletes: Vec::new(),
        stale: true,
    };
    let series = match &removal.plan.removes {
        Removes::Series { series } | Removes::From { series, .. } => Some(series.clone()),
        _ => None,
    };
    let _ = host.with_conn(|conn| {
        let now = match &series {
            Some(series) => calendar::series_rows(conn, cal, series)?,
            None => Vec::new(),
        };
        let before = rows_union(removal.snapshot, now);
        let gone = removal.plan.removes_for(&stale, removal.skipped).rows(
            cal,
            &before,
            removal.zone,
            removal.floor.as_deref(),
        );
        record(conn, host.edit(), cal, Vec::new(), &[], gone)
    });
    host.landed(cal);
    series_saved(vec![WriteWarning::MirrorRefreshPending], left_behind)
}

/// What a held series delete keeps for its send: the token it was chosen by, the series claimed,
/// and the rows it hides.
struct HeldPlan {
    token: String,
    master: Option<String>,
    hidden: Vec<String>,
}

/// The first half of a series delete, before its Undo window: claim, then plan on what the user saw
/// (c8:197-198). The claim comes back for the shell to hand off.
async fn plan_held_delete<'h, W: Wire, H: Host>(
    ctx: &SeriesWrite<'h, W, H>,
    event_id: &str,
    seen: &SeenSummary,
    scope: Scope,
    choice: Option<&str>,
) -> std::result::Result<(InFlight<'h>, HeldPlan), WriteOutcome> {
    // The series is known from the mirror row, so the claim comes before any read (L2).
    let master = ctx
        .host
        .with_conn(|conn| calendar::event_by_id(conn, &ctx.target.row_id))
        .map_err(|e| WriteOutcome::Failed {
            message: e.to_string(),
        })?
        .and_then(|row| row.series_id)
        .filter(|id| is_plain_segment(id));
    let mut keys = vec![ctx.target.row_id.clone()];
    keys.extend(
        master
            .as_deref()
            .map(|m| series_key(&ctx.target.remote_calendar, m)),
    );
    let flight = InFlight::claim_all(ctx.host.edit(), &keys).ok_or(WriteOutcome::Busy)?;
    let request = ChoiceRequest::Delete {
        event_id: event_id.to_string(),
        seen: seen.clone(),
    };
    let (_, planning) = prepared_for(ctx, &request, None, master.as_deref(), scope).await?;
    let plan = planned_or_refused(ctx, &planning, &request, scope)?;
    let token = plan.choice_digest(&planning.bind);
    if !choice_matches(choice, &token, ctx.missing_ok) {
        return Err(WriteOutcome::NeedsChoice);
    }
    let zone = planning.series.master.start.zone();
    let hidden = ctx
        .host
        .with_conn(|conn| {
            calendar::series_rows(conn, &ctx.target.calendar_id, &planning.series.master.id)
        })
        .map(|rows| {
            plan.removes
                .rows(&ctx.target.calendar_id, &rows, zone, None)
                .into_iter()
                .map(|g| g.row_id)
                .collect()
        })
        .unwrap_or_else(|_| vec![ctx.target.row_id.clone()]);
    Ok((
        flight,
        HeldPlan {
            token,
            master,
            hidden,
        },
    ))
}

/// The second half, after the Undo window, under the claim the first half handed off: gather again,
/// plan again, check it's still the plan chosen, run, land.
async fn send_held_series_delete<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    event_id: &str,
    seen: &SeenSummary,
    scope: Scope,
    held: &HeldPlan,
) -> WriteOutcome {
    let request = ChoiceRequest::Delete {
        event_id: event_id.to_string(),
        seen: seen.clone(),
    };
    let (mirror, planning) =
        match prepared_for(ctx, &request, None, held.master.as_deref(), scope).await {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
    let plan = match planned_or_refused(ctx, &planning, &request, scope) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    if plan.choice_digest(&planning.bind) != held.token {
        return WriteOutcome::NeedsChoice;
    }
    send_series(ctx, &mirror, &planning, plan, Landing::Delete).await
}

/// A scoped delete: planned now, held for its Undo window with the claim handed off, then sent.
async fn start_series_delete(
    app: AppHandle,
    target: Target,
    event_id: String,
    seen: SeenSummary,
    scope: Scope,
    choice: Option<String>,
    editing: EditingStatus,
) -> Result<DeleteStart> {
    let token_key = target.token_key.clone();
    let wire = GoogleWire(&token_key);
    let ctx = SeriesWrite {
        wire: &wire,
        host: &app,
        target: &target,
        editing,
        missing_ok: cfg!(debug_assertions),
    };
    let (flight, held) =
        match plan_held_delete(&ctx, &event_id, &seen, scope, choice.as_deref()).await {
            Ok(planned) => planned,
            Err(result) => return Ok(DeleteStart::Refused { result }),
        };
    let claim = flight.hand_off(&app);
    let undo_token = new_session_id();
    let undone = app.state::<AppState>().calendar_edit.held_deletes.hold(
        &undo_token,
        &event_id,
        &seen.summary,
        held.hidden.clone(),
        Instant::now() + UNDO,
    )?;
    let hidden = held.hidden.clone();
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
        // Editing may have been turned off during the wait.
        let editing = match editing_now(&task_app, &target) {
            Ok(editing) => editing,
            Err(result) => {
                let _ = task_app.emit_to(
                    "main",
                    "calendar://delete-settled",
                    DeleteSettled {
                        undo_token: token,
                        event_id,
                        result,
                    },
                );
                return;
            }
        };
        let wire = GoogleWire(&target.token_key);
        let ctx = SeriesWrite {
            wire: &wire,
            host: &task_app,
            target: &target,
            editing,
            missing_ok: cfg!(debug_assertions),
        };
        let result = send_held_series_delete(&ctx, &event_id, &seen, scope, &held).await;
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
        hidden,
    })
}

/// The account's editing status now, for a send after a wait.
fn editing_now(
    app: &AppHandle,
    target: &Target,
) -> std::result::Result<EditingStatus, WriteOutcome> {
    let choice = app
        .state::<AppState>()
        .conn()
        .and_then(|conn| calendar_editing::choice(&conn, &target.email))
        .map_err(|e| WriteOutcome::Failed {
            message: e.to_string(),
        })?;
    let editing = editing_status(choice, &target.token_key);
    if editing != EditingStatus::On {
        return Err(WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::EditingOff,
        });
    }
    Ok(editing)
}

/// Dev builds only: which calendars a write may change (`PM_CALENDAR_WRITE_ALLOW`, comma-separated
/// calendar names; C9 design scope-M5, L9). Unset, every write passes but one to more than one
/// occurrence, which fails closed; set, only the named calendars, resolved to Google's ids once per
/// run, a name that matches none or two refusing every write.
#[cfg(debug_assertions)]
mod dev_allow {
    use std::collections::HashSet;
    use std::sync::OnceLock;

    use rusqlite::Connection;

    use crate::calendar;

    pub(super) const REFUSED: &str =
        "This dev build may only change the calendars named in PM_CALENDAR_WRITE_ALLOW.";
    pub(super) const UNSET: &str =
        "This dev build changes more than one occurrence of a repeating \
                                    event only when PM_CALENDAR_WRITE_ALLOW names the calendar it \
                                    may change.";
    pub(super) const UNRESOLVED: &str = "PM_CALENDAR_WRITE_ALLOW names a calendar PM doesn't have, or \
                                         a name two calendars share, so this dev build won't change \
                                         any calendar.";
    const VAR: &str = "PM_CALENDAR_WRITE_ALLOW";

    /// The names resolved to Google calendar ids, once per run; `None`: a name didn't resolve.
    static ALLOWED: OnceLock<Option<HashSet<String>>> = OnceLock::new();

    fn var() -> Option<String> {
        std::env::var(VAR).ok().filter(|v| !v.trim().is_empty())
    }

    /// Comma-separated names to the Google ids of the Google calendars named so: `None` when a name
    /// matches none, or more than one.
    pub(super) fn resolve(
        names: &str,
        calendars: &[calendar::Calendar],
    ) -> Option<HashSet<String>> {
        let mut ids = HashSet::new();
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let matched: HashSet<String> = calendars
                .iter()
                .filter(|c| c.provider == "google" && c.name == name)
                .map(|c| c.remote_id.clone().unwrap_or_else(|| c.id.clone()))
                .collect();
            if matched.len() != 1 {
                return None;
            }
            ids.extend(matched);
        }
        (!ids.is_empty()).then_some(ids)
    }

    /// The command shells' rule, for a write to the calendar `remote`.
    pub(super) fn decide(
        var: Option<&str>,
        allowed: Option<&Option<HashSet<String>>>,
        remote: &str,
        series_wide: bool,
    ) -> std::result::Result<(), &'static str> {
        match (var, allowed) {
            (None, _) if series_wide => Err(UNSET),
            (None, _) => Ok(()),
            (Some(_), Some(Some(ids))) if ids.contains(remote) => Ok(()),
            (Some(_), Some(Some(_))) => Err(REFUSED),
            (Some(_), _) => Err(UNRESOLVED),
        }
    }

    /// Read the variable, resolve its names once, and decide.
    pub(super) fn check(
        conn: &Connection,
        remote: &str,
        series_wide: bool,
    ) -> crate::error::Result<std::result::Result<(), &'static str>> {
        let var = var();
        if let (Some(names), None) = (&var, ALLOWED.get()) {
            let calendars = calendar::list_calendars(conn)?;
            let _ = ALLOWED.set(resolve(names, &calendars));
        }
        Ok(decide(var.as_deref(), ALLOWED.get(), remote, series_wide))
    }

    /// The sender's backstop for every write: set, only a request under a resolved calendar.
    pub(super) fn permits(
        var: Option<&str>,
        allowed: Option<&Option<HashSet<String>>>,
        url: &str,
    ) -> bool {
        match (var, allowed) {
            (None, _) => true,
            (Some(_), Some(Some(ids))) => ids
                .iter()
                .any(|id| url.starts_with(&crate::calendar_write::plan::calendar_prefix(id))),
            (Some(_), _) => false,
        }
    }

    pub(super) fn sends_to(url: &str) -> bool {
        permits(var().as_deref(), ALLOWED.get(), url)
    }
}

/// What [`dev_series_report`] says about a series (dev builds only): rules, dates, counts and field
/// names, never a title, an id or an address.
#[cfg(debug_assertions)]
#[derive(Debug, Serialize)]
pub struct SeriesReport {
    /// Its `recurrence` lines, and how they read.
    rule: Vec<String>,
    repeats: String,
    master_updated: Option<String>,
    /// As listed by iCal UID, the master left out.
    exceptions: Vec<ReportedException>,
    /// The listing ran to its end; and still holds once checked against the occurrences.
    listing_complete: bool,
    cross_checked: bool,
    /// The occurrences in the mirror's window, cancelled ones included.
    instances: usize,
    instances_complete: bool,
    instances_cancelled: usize,
    modified_in_instances: usize,
    /// How many share the master's `updated` (C9 design L-R14: does it follow the master?).
    instances_updated_like_master: usize,
    /// Fields the master has that an occurrence lacks (`same_field`'s assumption, INFERRED).
    fields_missing_vs_master: Vec<&'static str>,
    /// The master's etag in the listing is the one a GET returns (A14's equal-etag rule).
    list_etag_matches_get: Option<bool>,
    /// Occurrences in the unwindowed listing a COUNT rule is counted from.
    counted: Option<usize>,
    split_off: bool,
    google_split: bool,
    sibling: bool,
    conference: bool,
}

/// One listed exception: where it sits (in the series' zone), its status, whether it's this series'
/// own (a listing by iCal UID holds other parts split off it too), and what changed.
#[cfg(debug_assertions)]
#[derive(Debug, Serialize)]
struct ReportedException {
    original: String,
    status: String,
    own_series: bool,
    kind: String,
}

/// Dev builds only: what Google returns for one series, for C9a's live probes (R0 and P5 in the C9
/// design): its rule, its exceptions as listed by iCal UID, its occurrences, and the assumptions the
/// planners make about them. Only reads go to Google. From the devtools console:
/// `await window.__TAURI_INTERNALS__.invoke("dev_series_report", { eventId })`, with a mirror row's
/// id.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_series_report(
    app: AppHandle,
    window: tauri::Window,
    event_id: String,
) -> Result<SeriesReport> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let state = app.state::<AppState>();
    let (target, mirror) = {
        let conn = state.conn()?;
        let target = resolve_target(&conn, &event_id)?
            .map_err(|_| Error::Other("PM can't read that event from Google.".into()))?;
        let mirror = MirrorFacts::read(&conn, &target.calendar_id)?;
        (target, mirror)
    };
    let wire = GoogleWire(&target.token_key);
    let failed = |outcome: WriteOutcome| Error::Other(format!("Google: {outcome:?}"));
    let fresh = get(&wire, &target.remote_calendar, &target.remote_event)
        .await
        .event()
        .map_err(failed)?;
    let series = gather(&wire, &target, &fresh, &mirror, true)
        .await
        .map_err(failed)?
        .ok_or_else(|| Error::Other("That event doesn't repeat.".into()))?;
    Ok(series_report(&series))
}

#[cfg(debug_assertions)]
fn series_report(series: &Series) -> SeriesReport {
    use crate::calendar_recur::Point;
    use crate::calendar_write::series::{classify_instance, original_of, InstanceKind};
    let (master, master_json) = (&series.master, series.master_fresh.json());
    let zone = master.start.zone();
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let own = |item: &Value| text(item, "recurringEventId").as_deref() == Some(master.id.as_str());
    let kind = |item: &Value| match classify_instance(master_json, master, item) {
        InstanceKind::Plain => "plain".to_string(),
        InstanceKind::Cancelled => "cancelled".to_string(),
        InstanceKind::Modified { carry, other } => {
            format!(
                "modified: carry={} other={}",
                carry.join(","),
                other.join(",")
            )
        }
    };
    let exceptions = series
        .exceptions
        .items
        .iter()
        .filter_map(|item| {
            let original = match original_of(item)? {
                Point::At(t) => t.with_timezone(&zone).format("%Y-%m-%d %H:%M").to_string(),
                Point::Day(d) => d.format("%Y-%m-%d").to_string(),
            };
            Some(ReportedException {
                original,
                status: text(item, "status").unwrap_or_else(|| "confirmed".into()),
                own_series: own(item),
                kind: kind(item),
            })
        })
        .collect();
    let occurrences: Vec<&Value> = series.window.items.iter().filter(|i| own(i)).collect();
    let cancelled = |i: &&&Value| text(i, "status").as_deref() == Some("cancelled");
    let master_updated = text(master_json, "updated");
    SeriesReport {
        rule: master.recurrence.clone(),
        repeats: describe_lines(&master.recurrence, &master.start),
        exceptions,
        listing_complete: series.listing_complete,
        cross_checked: series.exceptions.complete,
        instances: occurrences.len(),
        instances_complete: series.window.complete,
        instances_cancelled: occurrences.iter().filter(cancelled).count(),
        modified_in_instances: occurrences
            .iter()
            .filter(|i| {
                matches!(
                    classify_instance(master_json, master, i),
                    InstanceKind::Modified { .. }
                )
            })
            .count(),
        instances_updated_like_master: occurrences
            .iter()
            .filter(|i| master_updated.is_some() && text(i, "updated") == master_updated)
            .count(),
        fields_missing_vs_master: crate::calendar_write::series::fields_missing_vs_master(
            master_json,
            &series.window.items,
        ),
        list_etag_matches_get: series
            .exceptions
            .items
            .iter()
            .find(|i| text(i, "id").as_deref() == Some(master.id.as_str()))
            .map(|listed| text(listed, "etag") == text(master_json, "etag")),
        counted: series.counted.as_ref().map(|c| c.items.len()),
        master_updated,
        split_off: master_json
            .pointer("/extendedProperties/private/pmSplitOf")
            .is_some(),
        google_split: master.id.contains("_R"),
        sibling: series.sibling,
        conference: master_json.get("conferenceData").is_some()
            || master_json.get("hangoutLink").is_some(),
    }
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

impl Fetched {
    /// The event, or what not having it means for a write.
    fn event(self) -> std::result::Result<Value, WriteOutcome> {
        match self {
            Fetched::Event(event) => Ok(event),
            Fetched::Gone => Err(WriteOutcome::Gone),
            Fetched::Reauth => Err(WriteOutcome::Reauth),
            Fetched::Busy => Err(WriteOutcome::Busy),
            Fetched::Failed(message) => Err(WriteOutcome::Failed { message }),
        }
    }
}

/// What a request to Google came to: an answer, or why there's none ([`google_io::NoAnswer`]).
type Sent = std::result::Result<google_io::Reply, google_io::NoAnswer>;

/// Where requests to Google go: [`GoogleWire`] in the app, a script in the tests.
trait Wire: Sync {
    fn send(&self, plan: &RequestPlan) -> impl std::future::Future<Output = Sent> + Send;
    /// A pause a machine or a re-fetch asks for (a script records it and goes on at once).
    fn wait(&self, pause: Duration) -> impl std::future::Future<Output = ()> + Send;
}

/// Google itself, under the account's sign-in: the keychain key of its calendar token.
struct GoogleWire<'a>(&'a str);

impl Wire for GoogleWire<'_> {
    fn send(&self, plan: &RequestPlan) -> impl std::future::Future<Output = Sent> + Send {
        google_io::send(self.0, plan)
    }

    fn wait(&self, pause: Duration) -> impl std::future::Future<Output = ()> + Send {
        tokio::time::sleep(pause)
    }
}

async fn fetch(target: &Target) -> Fetched {
    get(
        &GoogleWire(&target.token_key),
        &target.remote_calendar,
        &target.remote_event,
    )
    .await
}

/// A fresh copy of one event.
async fn get<W: Wire>(wire: &W, calendar: &str, event: &str) -> Fetched {
    let request = plan::get_event(calendar, event);
    match wire
        .send(&request)
        .await
        .map_err(google_io::NoAnswer::into_error)
    {
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

/// A listing read page by page: `complete` only when its last page said there were no more.
struct Listed {
    items: Vec<Value>,
    complete: bool,
    /// Its first page was answered 404 or 410: the series itself isn't there.
    gone: bool,
}

/// Every page of a listing, `page` building each request from the previous page's token, up to
/// `max_pages`. A page that fails, or that PM can't read, ends it short, as does the cap: a listing
/// is never taken for whole on a guess.
async fn list_pages<W: Wire>(
    wire: &W,
    page: impl Fn(Option<&str>) -> RequestPlan,
    max_pages: usize,
) -> Listed {
    let mut items = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..max_pages {
        let reply = match wire.send(&page(token.as_deref())).await {
            Ok(reply) if reply.verdict == Verdict::Ok => reply,
            Ok(reply) if reply.verdict == Verdict::Gone && token.is_none() => {
                return Listed {
                    items,
                    complete: false,
                    gone: true,
                }
            }
            _ => break,
        };
        let Ok(body) = serde_json::from_str::<Value>(&reply.body) else {
            break;
        };
        let Some(page_items) = body.get("items").and_then(Value::as_array) else {
            break;
        };
        items.extend(page_items.iter().cloned());
        match body.get("nextPageToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => {
                return Listed {
                    items,
                    complete: true,
                    gone: false,
                }
            }
        }
    }
    Listed {
        items,
        complete: false,
        gone: false,
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

/// What a request that never reached Google means for the user: nothing changed.
fn unsent_outcome(why: Unsent) -> WriteOutcome {
    not_done_outcome(Why::Unsent(why), EditingStatus::On)
}

/// What a settled verdict means for the user.
fn outcome_for(verdict: Verdict, editing: EditingStatus) -> WriteOutcome {
    match verdict {
        Verdict::Ok => WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
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

/// Where a write reads and lands the mirror: the app, or a temp DB in the tests.
trait Host: Sync {
    /// `f` on the open store, holding the (non-reentrant) DB lock for `f` only (L11).
    fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R>;
    fn edit(&self) -> &CalendarEditState;
    /// The mirror changed: nudge the briefing and tell the main window. Called with no lock held.
    fn landed(&self, calendar_id: &str);
    /// The mirror's part in a series write. Its window follows the clock, so the tests fix it here.
    fn mirror(&self, calendar_id: &str) -> Result<MirrorFacts> {
        self.with_conn(|conn| MirrorFacts::read(conn, calendar_id))
    }
}

impl Host for AppHandle {
    fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let state = self.state::<AppState>();
        let conn = state.conn()?;
        f(&conn)
    }

    fn edit(&self) -> &CalendarEditState {
        &self.state::<AppState>().inner().calendar_edit
    }

    fn landed(&self, calendar_id: &str) {
        briefing::nudge(&self.state::<AppState>());
        let _ = self.emit_to(
            "main",
            "calendar://write-outcome",
            WriteLanded {
                calendar_id: calendar_id.to_string(),
            },
        );
    }
}

/// Write Google's copy into the mirror, remember it for the next sync, and tell the briefing and the
/// main window.
fn land(
    host: &impl Host,
    target: &Target,
    upserts: Vec<CalendarEvent>,
    deletes: Vec<GoneRow>,
) -> Result<()> {
    land_on(host, &target.calendar_id, upserts, &[], deletes)
}

fn land_on(
    host: &impl Host,
    calendar_id: &str,
    upserts: Vec<CalendarEvent>,
    created: &[String],
    deletes: Vec<GoneRow>,
) -> Result<()> {
    host.with_conn(|conn| record(conn, host.edit(), calendar_id, upserts, created, deletes))?;
    host.landed(calendar_id);
    Ok(())
}

/// One mirror write, then the same rows remembered for the next sync, inside the DB lock (lock
/// order: the DB, then the recent writes): `created` ones as new (a list that lags can't take them
/// off the screen), the other upserts as changed, and each gone row with the version removed.
fn record(
    conn: &Connection,
    edit: &CalendarEditState,
    calendar_id: &str,
    upserts: Vec<CalendarEvent>,
    created: &[String],
    deletes: Vec<GoneRow>,
) -> Result<()> {
    let delete_ids: Vec<String> = deletes.iter().map(|d| d.row_id.clone()).collect();
    calendar::apply_write_effect(conn, calendar_id, &upserts, &delete_ids)?;
    let mut recent = edit
        .recent
        .lock()
        .map_err(|_| Error::Other("recent writes lock poisoned".into()))?;
    let now = Stamp::now();
    for row in upserts {
        if created.contains(&row.id) {
            recent.record_create(row, now);
        } else {
            recent.record_upsert(row, now);
        }
    }
    for gone in deletes {
        recent.record_delete(calendar_id, &gone.row_id, gone.etag, gone.updated, now);
    }
    Ok(())
}

/// The `calendar://write-outcome` payload: a calendar whose mirror a save just changed.
#[derive(Clone, Serialize)]
struct WriteLanded {
    calendar_id: String,
}

/// Saved at Google, but not mirrored yet: say so; the next sync fills it in.
fn saved_pending(host: &impl Host, target: &Target) -> WriteOutcome {
    drop_hash(host, &target.calendar_id);
    WriteOutcome::Saved {
        warnings: vec![WriteWarning::MirrorRefreshPending],
        left_behind: Vec::new(),
    }
}

/// The calendar's rows no longer match Google: make sure the next sync rewrites them.
fn drop_hash(host: &impl Host, calendar_id: &str) {
    let _ = host.with_conn(|conn| calendar::apply_write_effect(conn, calendar_id, &[], &[]));
}

/// The row a write to `target` removes, at the version `copy` (Google's last copy of it) holds.
fn gone_row(target: &Target, copy: Option<&Value>) -> GoneRow {
    let text = |key: &str| {
        copy.and_then(|c| c.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    GoneRow {
        row_id: target.row_id.clone(),
        etag: text("etag"),
        updated: text("updated"),
    }
}

/// Gone from Google (deleted elsewhere, or cancelled from its series): take it out of the mirror too.
/// `cancelled` is Google's copy of the deleted event when it sent one, so a restore after it shows.
fn land_gone(host: &impl Host, target: &Target, cancelled: Option<&Value>) -> WriteOutcome {
    let _ = land(host, target, Vec::new(), vec![gone_row(target, cancelled)]);
    WriteOutcome::Gone
}

/// Our delete landed: take the row out, remembering the version deleted so a stale fetch can't
/// bring it back.
fn land_deleted(host: &impl Host, target: &Target, deleted: &Value) -> WriteOutcome {
    match land(
        host,
        target,
        Vec::new(),
        vec![gone_row(target, Some(deleted))],
    ) {
        Ok(()) => WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
        },
        Err(_) => saved_pending(host, target),
    }
}

/// A save landed: Google's copy now (its reply, or a fresh GET) goes into the mirror, and the session
/// moves onto it for the fields this save `sent` (see [`rebase_sent`]).
fn land_saved(
    host: &impl Host,
    target: &Target,
    session: &str,
    saved: Value,
    sent: &serde_json::Map<String, Value>,
) -> WriteOutcome {
    if let Ok(mut sessions) = host.edit().sessions.lock() {
        if let Some(open) = sessions.get_mut(session) {
            rebase_sent(&mut open.base, &saved, sent);
        }
    }
    let row = calendar::parse_event(&target.calendar_id, &saved);
    match row.map(|row| land(host, target, vec![row], Vec::new())) {
        Some(Ok(())) => WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
        },
        // Saved, but not as a row PM could mirror, or the mirror write failed: the next sync shows it.
        _ => saved_pending(host, target),
    }
}

/// A claim on mirror rows, and on a series, while a write to them is on its way; released on drop,
/// panics included.
struct InFlight<'a> {
    edit: &'a CalendarEditState,
    keys: Vec<String>,
}

impl<'a> InFlight<'a> {
    fn claim(edit: &'a CalendarEditState, row_id: &str) -> Option<Self> {
        Self::claim_all(edit, &[row_id.to_string()])
    }

    /// Every key at once, or none: a second write to the row or the series is Busy.
    fn claim_all(edit: &'a CalendarEditState, keys: &[String]) -> Option<Self> {
        let mut held = edit.in_flight.lock().ok()?;
        if keys.iter().any(|k| held.contains(k)) {
            return None;
        }
        held.extend(keys.iter().cloned());
        Some(InFlight {
            edit,
            keys: keys.to_vec(),
        })
    }

    /// Keep the claim past this command: a held delete's task holds it through the wait and the
    /// send, so the event answers Busy all that time.
    fn hand_off(self, app: &AppHandle) -> HeldClaim {
        // The claim moves into the returned value, so this one must not release it on drop.
        let mut this = std::mem::ManuallyDrop::new(self);
        HeldClaim {
            app: app.clone(),
            keys: std::mem::take(&mut this.keys),
        }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut held) = self.edit.in_flight.lock() {
            for key in &self.keys {
                held.remove(key);
            }
        }
    }
}

/// A claim that outlives the command that took it ([`InFlight::hand_off`]); released on drop.
struct HeldClaim {
    app: AppHandle,
    keys: Vec<String>,
}

impl Drop for HeldClaim {
    fn drop(&mut self) {
        if let Ok(mut held) = self.app.state::<AppState>().calendar_edit.in_flight.lock() {
            for key in &self.keys {
                held.remove(key);
            }
        }
    }
}

/// A series' key in the claims, by Google's own calendar and series ids (one Google calendar can be
/// mirrored under two accounts). Never a row id: those start with PM's calendar id.
fn series_key(remote_calendar: &str, master_id: &str) -> String {
    format!("series\u{1f}{remote_calendar}\u{1f}{master_id}")
}

/// The only code that sends a calendar write to Google. Private, so nothing outside this module can
/// call it, and the fence script keeps its name out of every model path.
mod google_io {
    use std::time::Duration;

    use chrono::{DateTime, Utc};

    use crate::calendar_write::classify::{self, Method, Verdict};
    use crate::calendar_write::plan::RequestPlan;
    use crate::calendar_write::series::run::Unsent;
    use crate::error::{Error, Result};
    use crate::google;

    /// Google's answer: what it means, and the body.
    pub(super) struct Reply {
        pub verdict: Verdict,
        pub body: String,
        /// A change was answered with a server error along the way, which Google may still have
        /// applied: unless the verdict is a success, the caller looks before saying what happened.
        pub maybe_applied: bool,
        /// When Google says it answered (its HTTP `Date`), for a removal's floor (A29).
        pub date: Option<DateTime<Utc>>,
    }

    /// No answer PM could read.
    pub(super) enum NoAnswer {
        /// Nothing of it can have been applied: every attempt failed before Google could apply it.
        Unsent { error: Error, why: Unsent },
        /// A copy may have landed. `retried`: a second copy went out after a server error.
        Lost { error: Error, retried: bool },
    }

    impl NoAnswer {
        pub(super) fn into_error(self) -> Error {
            match self {
                NoAnswer::Unsent { error, .. } | NoAnswer::Lost { error, .. } => error,
            }
        }
    }

    /// Whether an error from `authorized_send_with` leaves this attempt unapplied, and why: every
    /// error but a transport one comes from getting or renewing the token, before the request was
    /// built or after Google answered it 401 or 429 (its doc says so); a transport error proves
    /// nothing went only when the connection was never made, or the request never built.
    pub(super) fn unsent(error: &Error) -> Option<Unsent> {
        match error {
            Error::Reauth(_) => Some(Unsent::SignIn),
            Error::Http(e) if e.is_connect() => Some(Unsent::Offline),
            Error::Http(e) if e.is_builder() => Some(Unsent::Local),
            Error::Http(_) => None,
            _ => Some(Unsent::Local),
        }
    }

    /// A failed attempt, `maybe_applied` saying whether an earlier attempt of the same request met a
    /// server error.
    pub(super) fn no_answer(error: Error, maybe_applied: bool) -> NoAnswer {
        match (unsent(&error), maybe_applied) {
            (Some(why), false) => NoAnswer::Unsent { error, why },
            // A sign-in refused after a copy met a server error: that copy may have landed, and this
            // one never went (review H1).
            (Some(_), true) => NoAnswer::Lost {
                error,
                retried: false,
            },
            (None, earlier) => NoAnswer::Lost {
                error,
                retried: earlier,
            },
        }
    }

    /// The most PM reads of an answer: an event is a few kilobytes.
    const MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;

    /// Send `plan`, retrying once where that can't apply a change twice (`classify::should_retry`).
    /// A 429 was already waited out (or handed back) by `authorized_send_with` under the interactive
    /// policy, so only Google's 403 rate-limit shape is retried here. `Err` says whether a copy may
    /// have landed ([`NoAnswer`]).
    pub(super) async fn send(
        token_key: &str,
        plan: &RequestPlan,
    ) -> std::result::Result<Reply, NoAnswer> {
        // Dev builds: never a write to a calendar PM_CALENDAR_WRITE_ALLOW doesn't name, whichever
        // path sent it (review L9).
        #[cfg(debug_assertions)]
        if plan.method != Method::Get && !super::dev_allow::sends_to(&plan.url) {
            return Err(NoAnswer::Unsent {
                error: Error::Other(super::dev_allow::REFUSED.into()),
                why: Unsent::Local,
            });
        }
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(client) => client,
            Err(e) => {
                return Err(NoAnswer::Unsent {
                    error: Error::from(e),
                    why: Unsent::Local,
                })
            }
        };
        let mut attempt = 1;
        let mut maybe_applied = false;
        loop {
            let sent = google::authorized_send_with(
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
            .await;
            let resp = match sent {
                Ok(resp) => resp,
                Err(error) => return Err(no_answer(error, maybe_applied)),
            };
            let status = resp.status().as_u16();
            let date = google::server_time_of(resp.headers());
            // Google answered, but PM can't read what: a 2xx may sit unread.
            let body = match read_capped(resp).await {
                Ok(body) => body,
                Err(error) => {
                    return Err(NoAnswer::Lost {
                        error,
                        retried: maybe_applied,
                    })
                }
            };
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
                date,
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

    /// A delete started from the editor compares against what the editor showed (Google's fresh
    /// copy), in the very terms the delete check uses.
    #[test]
    fn the_editor_hands_a_delete_what_it_showed() {
        let fresh = json!({
            "id": "abc", "summary": "Dentist", "location": "High St",
            "start": { "dateTime": "2026-10-12T09:00:00+01:00" },
            "end": { "dateTime": "2026-10-12T10:00:00+01:00" }
        });
        let seen = seen_of("cal", &fresh).unwrap();
        assert_eq!(seen.summary, "Dentist");
        assert_eq!(seen.location.as_deref(), Some("High St"));
        assert!(!seen.all_day);
        assert_eq!(
            patch::delete_still_matches(&seen, &fresh),
            DeleteCheck::Matches
        );
        assert!(seen_of("cal", &json!({ "id": "abc" })).is_none());
    }

    /// Undo and the end of the wait race for one entry, and whichever gets it decides.
    #[test]
    fn a_held_delete_is_either_undone_or_sent_never_both() {
        let held = HeldDeletes::default();
        let until = Instant::now() + UNDO;
        let mut undone = held
            .hold("a", "cal:a", "Dentist", Vec::new(), until)
            .unwrap();
        assert!(held.cancel("a"), "still waiting, so undone");
        assert!(undone.try_recv().is_ok(), "the waiting task hears it");
        assert!(!held.take("a"), "so the wait's end doesn't send it");

        let _rx = held.hold("b", "cal:b", "Gym", Vec::new(), until).unwrap();
        assert!(held.take("b"), "the wait ended first: it goes to Google");
        assert!(!held.cancel("b"), "too late to undo");

        assert!(!held.cancel("never-held"));
    }

    /// A reloaded webview gets the waiting deletes oldest first, so their notices come back in the
    /// order they were made and Ctrl+Z (the newest notice) undoes the newest delete.
    #[test]
    fn waiting_deletes_are_listed_oldest_first() {
        let held = HeldDeletes::default();
        let now = Instant::now();
        let tokens = ["n", "c", "x", "a", "q"];
        // Held newest first, so the list can't come back in the order it was made by luck.
        let _waiting: Vec<_> = tokens
            .iter()
            .enumerate()
            .rev()
            .map(|(i, token)| {
                held.hold(
                    token,
                    "cal:e",
                    "Event",
                    Vec::new(),
                    now + Duration::from_millis(i as u64 * 10),
                )
                .unwrap()
            })
            .collect();
        let listed: Vec<String> = held.list().into_iter().map(|h| h.undo_token).collect();
        assert_eq!(listed, tokens);
    }

    /// Quitting drops every waiting delete: each task wakes as undone, and none can be taken.
    #[test]
    fn quitting_abandons_every_held_delete() {
        let held = HeldDeletes::default();
        let until = Instant::now() + UNDO;
        let mut a = held
            .hold("a", "cal:a", "Dentist", Vec::new(), until)
            .unwrap();
        let _b = held.hold("b", "cal:b", "Gym", Vec::new(), until).unwrap();
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

    // --- repeating events (C9a-2): Google scripted ---

    use crate::calendar_write::classify::{self, Method};
    use crate::calendar_write::series::fixtures::{
        at, cancelled, fresh as fresh_event, master, monday_19, moved, plain,
        seen_of as seen_by_user, title, weekly, with, CALENDAR,
    };
    use crate::calendar_write::series::scope::ScopeReason;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One scripted request: its method, the end of its path, something its query must hold, the
    /// etag a write must go out on, the answer, and what else happens as it's answered (a sync
    /// landing mid-run, say).
    struct Step {
        method: Method,
        path: String,
        query: &'static str,
        if_match: Option<String>,
        answer: Scripted,
        then: Option<Box<dyn Fn() + Send + Sync>>,
    }

    enum Scripted {
        Json(Value),
        Status(u16),
        /// Any answer: its status, body and `Date`, and whether an attempt met a server error.
        Reply {
            status: u16,
            body: Option<Value>,
            date: Option<chrono::DateTime<chrono::Utc>>,
            maybe_applied: bool,
        },
        Lost {
            retried: bool,
        },
        Unsent(Unsent),
    }

    impl Step {
        fn new(method: Method, path: &str, query: &'static str, answer: Scripted) -> Step {
            Step {
                method,
                path: path.into(),
                query,
                if_match: None,
                answer,
                then: None,
            }
        }

        fn get(path: &str, query: &'static str, body: Value) -> Step {
            Step::new(Method::Get, path, query, Scripted::Json(body))
        }

        fn status(path: &str, query: &'static str, status: u16) -> Step {
            Step::new(Method::Get, path, query, Scripted::Status(status))
        }

        /// A write to `path` on `etag`, emailing nobody.
        fn write(method: Method, path: &str, etag: &str, answer: Scripted) -> Step {
            Step {
                if_match: Some(etag.into()),
                ..Step::new(method, path, "sendUpdates=none", answer)
            }
        }

        fn then(self, then: impl Fn() + Send + Sync + 'static) -> Step {
            Step {
                then: Some(Box::new(then)),
                ..self
            }
        }
    }

    /// Google, scripted: each request must be the next step's, and gets its answer. Read by an
    /// index, never a queue's pop (CodeQL).
    struct Script {
        steps: Vec<Step>,
        next: AtomicUsize,
        sent: Mutex<Vec<RequestPlan>>,
        waits: Mutex<Vec<Duration>>,
    }

    impl Script {
        fn new(steps: Vec<Step>) -> Script {
            Script {
                steps,
                next: AtomicUsize::new(0),
                sent: Mutex::new(Vec::new()),
                waits: Mutex::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<RequestPlan> {
            self.sent.lock().unwrap().clone()
        }

        fn waits(&self) -> Vec<Duration> {
            self.waits.lock().unwrap().clone()
        }

        /// Every step was asked for.
        fn asked_all(&self) {
            assert_eq!(self.next.load(Ordering::SeqCst), self.steps.len());
        }

        /// Every step was asked for, and nothing but reads went out.
        fn finished(&self) {
            self.asked_all();
            assert!(self.sent().iter().all(|p| p.method == Method::Get));
        }
    }

    impl Wire for Script {
        fn send(&self, plan: &RequestPlan) -> impl std::future::Future<Output = Sent> + Send {
            let i = self.next.fetch_add(1, Ordering::SeqCst);
            self.sent.lock().unwrap().push(plan.clone());
            let step = self
                .steps
                .get(i)
                .unwrap_or_else(|| panic!("request {i} wasn't scripted: {}", plan.url));
            let (path, query) = plan.url.split_once('?').unwrap_or((&plan.url, ""));
            // On Google's id for the calendar, never the mirror's; every part of the query asked for.
            let on = format!("/calendar/v3/calendars/{CALENDAR}{}", step.path);
            assert!(
                plan.method == step.method
                    && path.ends_with(&on)
                    && step.query.split(' ').all(|part| query.contains(part))
                    && step
                        .if_match
                        .as_ref()
                        .is_none_or(|etag| plan.if_match.as_ref() == Some(etag)),
                "request {i} was {:?} {} on {:?}, not {:?} {}?{} on {:?}",
                plan.method,
                plan.url,
                plan.if_match,
                step.method,
                step.path,
                step.query,
                step.if_match
            );
            if let Some(then) = &step.then {
                then();
            }
            let reply = |status: u16, body: String, date, maybe_applied| {
                Ok(google_io::Reply {
                    verdict: classify::classify(status, &body),
                    body,
                    maybe_applied,
                    date,
                })
            };
            std::future::ready(match &step.answer {
                Scripted::Json(v) => reply(200, v.to_string(), None, false),
                Scripted::Status(s) => reply(*s, String::new(), None, false),
                Scripted::Reply {
                    status,
                    body,
                    date,
                    maybe_applied,
                } => reply(
                    *status,
                    body.as_ref().map(Value::to_string).unwrap_or_default(),
                    *date,
                    *maybe_applied,
                ),
                Scripted::Lost { retried } => Err(google_io::NoAnswer::Lost {
                    error: Error::Other("no answer".into()),
                    retried: *retried,
                }),
                Scripted::Unsent(why) => Err(google_io::NoAnswer::Unsent {
                    error: Error::Other("not sent".into()),
                    why: *why,
                }),
            })
        }

        fn wait(&self, pause: Duration) -> impl std::future::Future<Output = ()> + Send {
            self.waits.lock().unwrap().push(pause);
            std::future::ready(())
        }
    }

    fn page(items: Vec<Value>, next: Option<&str>) -> Value {
        match next {
            Some(token) => json!({ "items": items, "nextPageToken": token }),
            None => json!({ "items": items }),
        }
    }

    fn target(event: &Value) -> Target {
        let event = event["id"].as_str().unwrap();
        Target {
            row_id: format!("cal:{event}"),
            calendar_id: "cal".into(),
            remote_calendar: CALENDAR.into(),
            remote_event: event.into(),
            email: "me@example.com".into(),
            token_key: "key".into(),
            facts: CalendarFacts {
                access_role: Some("owner".into()),
                time_zone: Some("Europe/London".into()),
                ..Default::default()
            },
        }
    }

    fn mirror(series: &[&str]) -> MirrorFacts {
        MirrorFacts {
            series_ids: series.iter().map(|s| s.to_string()).collect(),
            window: ("2026-09-01T00:00:00Z".into(), "2027-11-01T00:00:00Z".into()),
        }
    }

    /// The reads one dialog makes of a series with no COUNT, in order: the occurrence, then
    /// [`series_reads`].
    fn reads(
        instance: &Value,
        series: &Value,
        listed: Vec<Value>,
        window: Vec<Value>,
    ) -> Vec<Step> {
        let mut steps = vec![instance_read(instance)];
        steps.extend(series_reads(series, listed, window));
        steps
    }

    fn instance_read(instance: &Value) -> Step {
        Step::get(
            &format!("/events/{}", instance["id"].as_str().unwrap()),
            "",
            instance.clone(),
        )
    }

    /// The series, its listing by iCal UID, and its occurrences in the mirror's window.
    fn series_reads(series: &Value, listed: Vec<Value>, window: Vec<Value>) -> Vec<Step> {
        vec![
            Step::get("/events/abc", "", series.clone()),
            Step::get("/events", "iCalUID=abc%40google.com", page(listed, None)),
            window_read(window),
        ]
    }

    /// The cross-check's read of the occurrences in the mirror's window: only the fields it asks for,
    /// and each occurrence answered as Google would answer that mask.
    fn window_read(window: Vec<Value>) -> Step {
        Step::get(
            "/events/abc/instances",
            "showDeleted=true timeMin=2026-09-01T00%3A00%3A00Z timeMax=2027-11-01T00%3A00%3A00Z \
             maxResults=250 fields=nextPageToken%2Citems%28id%2Cstatus%2CrecurringEventId%2C\
             originalStartTime%2Cstart%2Cend%2Csummary%2Cattendees%28email%2Cself%2Coptional%29%2C\
             attendeesOmitted%29",
            page(window.iter().map(masked).collect(), None),
        )
    }

    /// `event` as Google answers [`WINDOW_FIELDS`]: only what its `items(...)` selects, read from the
    /// constant itself, so a field dropped from the mask is dropped from every fixture too.
    fn masked(event: &Value) -> Value {
        let items = WINDOW_FIELDS
            .split_once("items(")
            .and_then(|(_, rest)| rest.strip_suffix(')'))
            .expect("WINDOW_FIELDS selects items(...)");
        keep(event, items)
    }

    /// `value` with only the fields `selector` names (`a,b(c,d)`): an object's own, and each of an
    /// array's.
    fn keep(value: &Value, selector: &str) -> Value {
        if let Some(list) = value.as_array() {
            return Value::from(list.iter().map(|v| keep(v, selector)).collect::<Vec<_>>());
        }
        let mut parts = Vec::new();
        let (mut depth, mut part) = (0, String::new());
        for c in selector.chars().chain([',']) {
            match c {
                ',' if depth == 0 => parts.push(std::mem::take(&mut part)),
                '(' | ')' => {
                    depth += if c == '(' { 1 } else { -1 };
                    part.push(c);
                }
                _ => part.push(c),
            }
        }
        let mut kept = serde_json::Map::new();
        for part in parts {
            let (name, inner) = match part.split_once('(') {
                Some((name, rest)) => {
                    (name.to_string(), rest.strip_suffix(')').map(str::to_string))
                }
                None => (part, None),
            };
            if let Some(v) = value.get(&name) {
                let v = match &inner {
                    Some(inner) => keep(v, inner),
                    None => v.clone(),
                };
                kept.insert(name, v);
            }
        }
        Value::Object(kept)
    }

    async fn listing(script: &Script, cap: usize) -> Listed {
        list_pages(
            script,
            |page: Option<&str>| plan::list_by_ical(CALENDAR, "abc@google.com", page),
            cap,
        )
        .await
    }

    fn edit(draft: EventPatchDraft) -> ChoiceRequest {
        ChoiceRequest::Edit {
            session: "s1".into(),
            draft,
        }
    }

    /// The choices for `request` on `instance`, the editor opened on `opened` (an edit) and the
    /// mirror holding `series`.
    async fn ask(
        script: &Script,
        instance: &Value,
        request: ChoiceRequest,
        opened: Option<(&Value, Option<&Value>)>,
        series: &[&str],
    ) -> (ScopeRow, ScopeRow, ScopeRow) {
        let target = target(instance);
        let got = choices(
            script,
            &target,
            EditingStatus::On,
            &mirror(series),
            &request,
            opened,
        )
        .await;
        script.finished();
        match got {
            SeriesChoices::Ready {
                this,
                following,
                all,
            } => (this, following, all),
            other => panic!("{other:?}"),
        }
    }

    /// The choices when the gate or the series refuses before any row: `steps` must all be asked.
    async fn refused(
        steps: Vec<Step>,
        instance: &Value,
        editing: EditingStatus,
        role: &str,
        request: ChoiceRequest,
    ) -> SeriesChoices {
        let script = Script::new(steps);
        let mut on = target(instance);
        on.facts.access_role = Some(role.into());
        let got = choices(
            &script,
            &on,
            editing,
            &mirror(&["abc"]),
            &request,
            Some((instance, None)),
        )
        .await;
        script.finished();
        got
    }

    fn warnings(row: &ScopeRow) -> &[SeriesWarningDto] {
        match row {
            ScopeRow::Offered { warnings, .. } | ScopeRow::AsAll { warnings, .. } => warnings,
            other => panic!("not offered: {other:?}"),
        }
    }

    fn token(row: &ScopeRow) -> &str {
        match row {
            ScopeRow::Offered { token, .. } | ScopeRow::AsAll { token, .. } => token,
            other => panic!("not offered: {other:?}"),
        }
    }

    fn incomplete() -> ScopeRow {
        ScopeRow::Refused {
            message: SeriesRefusal::Incomplete.to_string(),
        }
    }

    /// Opening an occurrence fetches its series to keep with the session; a one-off, a series that's
    /// gone (404, or a copy marked cancelled), or one under another id, keeps nothing.
    #[tokio::test]
    async fn opening_an_occurrence_fetches_its_series() {
        let (series, instance) = (weekly(), monday_19());
        let script = Script::new(vec![Step::get("/events/abc", "", series.clone())]);
        assert_eq!(
            open_master(&script, &target(&instance), &instance).await,
            Some(series.clone())
        );
        script.finished();
        assert_eq!(
            series_info(&instance, &series),
            Some(SeriesInfo {
                first: false,
                repeats: "Weekly on Monday".into()
            })
        );
        let first = plain("2026-10-05T09:00:00+01:00");
        assert!(series_info(&first, &series).unwrap().first);
        let one_off = json!({ "id": "x" });
        let silent = Script::new(Vec::new());
        assert_eq!(
            open_master(&silent, &target(&one_off), &one_off).await,
            None
        );
        silent.finished();
        for answer in [
            Step::status("/events/abc", "", 404),
            Step::get(
                "/events/abc",
                "",
                with(series.clone(), "status", json!("cancelled")),
            ),
            Step::get("/events/abc", "", with(series.clone(), "id", json!("xyz"))),
        ] {
            let script = Script::new(vec![answer]);
            assert_eq!(
                open_master(&script, &target(&instance), &instance).await,
                None
            );
            script.finished();
        }
    }

    /// DS12: every row a change offers is planned, with its warnings and token; a row the change
    /// can't take says why in words.
    #[tokio::test]
    async fn choices_plan_every_offered_row_and_word_refusals() {
        let (series, instance) = (weekly(), monday_19());
        let window = vec![
            plain("2026-10-12T09:00:00+01:00"),
            instance.clone(),
            plain("2026-10-26T09:00:00Z"),
        ];
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            window.clone(),
        ));
        let opened = Some((&instance, Some(&series)));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        script.finished();
        assert!(matches!(&this, ScopeRow::Offered { warnings, .. } if warnings.is_empty()));
        assert_eq!(warnings(&following), [SeriesWarningDto::NewIdentity]);
        assert!(matches!(&all, ScopeRow::Offered { warnings, .. } if warnings.is_empty()));
        for row in [&this, &following, &all] {
            assert_eq!(token(row).len(), 16);
        }
        assert_ne!(token(&this), token(&all));

        // Who can see it is the series' (Google), so never "this event" alone.
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            window.clone(),
        ));
        let private = EventPatchDraft {
            visibility: Some(Visibility::Private),
            ..Default::default()
        };
        let (this, following, all) = ask(&script, &instance, edit(private), opened, &["abc"]).await;
        assert_eq!(
            this,
            ScopeRow::Refused {
                message: ScopeReason::VisibilityIsSeriesWide.to_string()
            }
        );
        assert!(matches!(following, ScopeRow::Offered { .. }));
        assert!(matches!(all, ScopeRow::Offered { .. }));

        // Renamed in Google since the editor opened, the series and so each occurrence: every row
        // sends a title, and says so.
        let renamed = |event: &Value| with(event.clone(), "summary", json!("Standup v2"));
        let script = Script::new(reads(
            &renamed(&instance),
            &renamed(&series),
            vec![renamed(&series)],
            window.iter().map(renamed).collect(),
        ));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        let conflict = ScopeRow::Conflict {
            fields: vec!["summary".into()],
        };
        assert_eq!(
            (this, following, all),
            (conflict.clone(), conflict.clone(), conflict.clone())
        );
        // Only the occurrence renamed on its own: "this event" meets the change, and the series'
        // rows, which don't send this occurrence's title, still go ahead.
        let own = renamed(&instance);
        let script = Script::new(reads(
            &own,
            &series,
            vec![series.clone(), own.clone()],
            vec![own.clone()],
        ));
        let (this, _, all) = ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert_eq!(this, conflict);
        assert!(matches!(all, ScopeRow::Offered { .. }));

        // Nothing to change.
        let script = Script::new(reads(&instance, &series, vec![series.clone()], window));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Standup")), opened, &["abc"]).await;
        let nothing = ScopeRow::Refused {
            message: "Google already has this, so there's nothing to save.".into(),
        };
        assert_eq!((&this, &following, &all), (&nothing, &nothing, &nothing));
    }

    /// From the first occurrence, "this and following" is every occurrence: offered as "All
    /// events", the same plan with the same token.
    #[tokio::test]
    async fn following_on_the_first_occurrence_is_as_all() {
        let (series, first) = (weekly(), plain("2026-10-05T09:00:00+01:00"));
        let script = Script::new(reads(
            &first,
            &series,
            vec![series.clone()],
            vec![first.clone()],
        ));
        let (_, following, all) = ask(
            &script,
            &first,
            edit(title("Retro")),
            Some((&first, Some(&series))),
            &["abc"],
        )
        .await;
        assert!(matches!(following, ScopeRow::AsAll { .. }));
        assert_eq!(token(&following), token(&all));
    }

    /// A29: a listing by iCal UID that didn't run to its end refuses what reaches past the occurrence
    /// picked, and the occurrences' own listing never stands in for it. Until C12 that includes a
    /// change of text to all of them: only a whole listing shows none has guests of its own.
    #[tokio::test]
    async fn an_exceptions_page_that_fails_refuses_incomplete_never_reads_instances() {
        let (series, instance) = (weekly(), monday_19());
        let failing = |instance: &Value| {
            vec![
                instance_read(instance),
                Step::get("/events/abc", "", series.clone()),
                Step::get(
                    "/events",
                    "iCalUID=abc%40google.com",
                    page(vec![series.clone()], Some("p2")),
                ),
                Step::status("/events", "pageToken=p2", 500),
                window_read(vec![instance.clone()]),
            ]
        };
        let script = Script::new(failing(&instance));
        let opened = Some((&instance, Some(&series)));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert!(script.sent()[3].url.contains("pageToken=p2"));
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!((following, all), (incomplete(), incomplete()));
        let script = Script::new(failing(&instance));
        let delete = ChoiceRequest::Delete {
            event_id: "cal:abc_20261019T080000Z".into(),
            seen: seen_by_user(&fresh_event(instance.clone())),
        };
        let (this, following, all) = ask(&script, &instance, delete, None, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!((following, all), (incomplete(), incomplete()));
    }

    /// C9 design §0.8, review M3/L4: an occurrence moved or cancelled on its own that the listing by
    /// iCal UID misses refuses a split, and an "all events" move, which would otherwise lose or bring
    /// it back. (Until C12 the guests rule refuses them first, a change of text to every occurrence
    /// too; plan.rs tests each guard on its own with guests allowed.)
    #[tokio::test]
    async fn a_moved_occurrence_the_listing_misses_refuses_incomplete() {
        let series = weekly();
        let instance = plain("2026-10-26T09:00:00Z");
        let moved_19 = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-20T09:00:00+01:00",
            "2026-10-20T10:00:00+01:00",
        );
        let opened = Some((&instance, Some(&series)));
        for missed in [moved_19.clone(), cancelled("2026-10-19T09:00:00+01:00")] {
            let window = vec![missed.clone(), instance.clone()];
            let script = Script::new(reads(&instance, &series, vec![series.clone()], window));
            let (this, following, all) =
                ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
            assert!(matches!(this, ScopeRow::Offered { .. }));
            assert_eq!(
                (&following, &all),
                (&incomplete(), &incomplete()),
                "{missed}"
            );
            let window = vec![missed.clone(), instance.clone()];
            let script = Script::new(reads(&instance, &series, vec![series.clone()], window));
            let (_, _, all) = ask(
                &script,
                &instance,
                edit(at("10:00", "2026-10-26")),
                opened,
                &["abc"],
            )
            .await;
            assert_eq!(all, incomplete(), "{missed}");
        }
        // Listed, nothing is missing.
        let window = vec![moved_19.clone(), instance.clone()];
        let listed = vec![series.clone(), moved_19];
        let script = Script::new(reads(&instance, &series, listed, window));
        let (_, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert!(matches!(following, ScopeRow::Offered { .. }));
        assert!(matches!(all, ScopeRow::Offered { .. }));
    }

    /// A7: a rule that ends after so many is cut by Google's own count of what came before, read
    /// from every occurrence (unwindowed, only the fields counting needs). Without it, no split.
    #[tokio::test]
    async fn a_count_series_takes_googles_k_from_unwindowed_instances() {
        let series = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"]);
        let fifth = plain("2026-11-02T09:00:00Z");
        let all_ten: Vec<Value> = [
            "2026-10-05T09:00:00+01:00",
            "2026-10-12T09:00:00+01:00",
            "2026-10-19T09:00:00+01:00",
            "2026-10-26T09:00:00Z",
            "2026-11-02T09:00:00Z",
            "2026-11-09T09:00:00Z",
            "2026-11-16T09:00:00Z",
            "2026-11-23T09:00:00Z",
            "2026-11-30T09:00:00Z",
            "2026-12-07T09:00:00Z",
        ]
        .into_iter()
        .map(plain)
        .collect();
        let counted = |answer: Scripted| {
            let mut steps = reads(&fifth, &series, vec![series.clone()], all_ten.clone());
            steps.push(Step::new(
                Method::Get,
                "/events/abc/instances",
                "fields=nextPageToken%2Citems%28id%2Cstatus%2CoriginalStartTime%29",
                answer,
            ));
            steps
        };
        let opened = Some((&fifth, Some(&series)));
        let script = Script::new(counted(Scripted::Json(page(all_ten.clone(), None))));
        let (_, following, _) = ask(&script, &fifth, edit(title("Retro")), opened, &["abc"]).await;
        script.finished();
        assert!(matches!(following, ScopeRow::Offered { .. }));
        let count = &script.sent()[4];
        assert!(!count.url.contains("timeMin"), "{}", count.url);
        let script = Script::new(counted(Scripted::Status(500)));
        let (_, following, all) =
            ask(&script, &fifth, edit(title("Retro")), opened, &["abc"]).await;
        assert_eq!(
            following,
            ScopeRow::Refused {
                message: crate::calendar_recur::RecurError::CannotCount.to_string()
            }
        );
        assert!(matches!(all, ScopeRow::Offered { .. }));
    }

    /// C8b review G5: a series with guests stays read-only until C12, even from an occurrence whose
    /// guests were removed on its own.
    #[tokio::test]
    async fn a_series_with_a_guest_is_read_only_until_c12() {
        let series = with(
            weekly(),
            "attendees",
            json!([{ "email": "a@example.com", "responseStatus": "accepted" }]),
        );
        let instance = monday_19();
        let script = Script::new(vec![
            instance_read(&instance),
            Step::get("/events/abc", "", series.clone()),
        ]);
        let got = choices(
            &script,
            &target(&instance),
            EditingStatus::On,
            &mirror(&["abc"]),
            &edit(title("Retro")),
            Some((&instance, Some(&series))),
        )
        .await;
        script.finished();
        assert_eq!(
            got,
            SeriesChoices::Refused {
                result: WriteOutcome::ReadOnly {
                    reason: ReadOnlyReason::HasGuests
                }
            }
        );
    }

    /// C9 design review L7: an occurrence with a plain description of its own, in a series whose
    /// description is formatted, may change its own; the series' rows would overwrite the formatted
    /// one as plain text.
    #[tokio::test]
    async fn a_formatted_series_description_is_never_saved_as_plain_text() {
        let series = with(weekly(), "description", json!("<b>Agenda</b>"));
        let instance = monday_19();
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), instance.clone()],
            vec![instance.clone()],
        ));
        let notes = EventPatchDraft {
            description: Some("New notes".into()),
            ..Default::default()
        };
        let (this, following, all) = ask(
            &script,
            &instance,
            edit(notes),
            Some((&instance, Some(&series))),
            &["abc"],
        )
        .await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        for row in [following, all] {
            assert!(
                matches!(&row, ScopeRow::Refused { message } if message.contains("formatting")),
                "{row:?}"
            );
        }
        // A title is no reason to hold the series back.
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), instance.clone()],
            vec![instance.clone()],
        ));
        let (_, _, all) = ask(
            &script,
            &instance,
            edit(title("Retro")),
            Some((&instance, Some(&series))),
            &["abc"],
        )
        .await;
        assert!(matches!(all, ScopeRow::Offered { .. }));
    }

    /// DS16: a part split off this series earlier (Google's own `_R`) is said where the choice
    /// reaches past this occurrence.
    #[tokio::test]
    async fn a_google_split_sibling_warns() {
        let (series, instance) = (weekly(), monday_19());
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            vec![instance.clone()],
        ));
        let delete = ChoiceRequest::Delete {
            event_id: "cal:abc_20261019T080000Z".into(),
            seen: seen_by_user(&fresh_event(instance.clone())),
        };
        let (this, following, all) = ask(
            &script,
            &instance,
            delete,
            None,
            &["abc", "abc_R20261102T090000"],
        )
        .await;
        assert!(warnings(&this).is_empty());
        assert_eq!(warnings(&following), [SeriesWarningDto::SplitSibling]);
        assert_eq!(warnings(&all), [SeriesWarningDto::SplitSibling]);
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            vec![instance.clone()],
        ));
        let (_, following, _) = ask(
            &script,
            &instance,
            edit(title("Retro")),
            Some((&instance, Some(&series))),
            &["abc", "abc_R20261102T090000"],
        )
        .await;
        assert!(warnings(&following).contains(&SeriesWarningDto::SplitSibling));
    }

    /// Only reads go out, and what comes back names no etag, event id or new series' id.
    #[tokio::test]
    async fn choices_send_only_gets_and_carry_no_etag_or_tail_id() {
        let (series, instance) = (weekly(), monday_19());
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            vec![instance.clone()],
        ));
        let got = choices(
            &script,
            &target(&instance),
            EditingStatus::On,
            &mirror(&["abc"]),
            &edit(at("10:00", "2026-10-20")),
            Some((&instance, Some(&series))),
        )
        .await;
        script.finished();
        let text = serde_json::to_string(&got).unwrap();
        for leak in [
            "m1",
            instance["etag"].as_str().unwrap().trim_matches('"'),
            "abc_",
            r#""abc""#,
            "example.com",
        ] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        let longest_hex = text
            .split(|c: char| !c.is_ascii_hexdigit())
            .map(str::len)
            .max();
        assert_eq!(longest_hex, Some(16), "{text}");
        // The move to Tuesday says what the days were and what they become.
        assert!(text.contains(
            r#""warning":"repeat_day_changes","from":"2026-10-19","to":"2026-10-20","before":"every Monday","after":"every Tuesday","shift_days":1"#
        ), "{text}");
    }

    /// A delete is offered on what the user saw; a change since is a conflict before any row, and a
    /// one-off has no choice to make. A row that has since become a series' master is refused.
    #[tokio::test]
    async fn a_delete_asks_on_what_the_user_saw_and_a_one_off_has_no_choice() {
        let (series, instance) = (weekly(), monday_19());
        let seen = seen_by_user(&fresh_event(instance.clone()));
        let renamed = with(instance.clone(), "summary", json!("Standup moved"));
        let script = Script::new(vec![Step::get(
            "/events/abc_20261019T080000Z",
            "",
            renamed.clone(),
        )]);
        let delete = ChoiceRequest::Delete {
            event_id: "cal:abc_20261019T080000Z".into(),
            seen: seen.clone(),
        };
        let got = choices(
            &script,
            &target(&instance),
            EditingStatus::On,
            &mirror(&["abc"]),
            &delete,
            None,
        )
        .await;
        assert_eq!(
            got,
            SeriesChoices::Refused {
                result: WriteOutcome::Conflict {
                    fields: vec!["summary".into()]
                }
            }
        );
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            vec![instance.clone()],
        ));
        let (this, following, all) = ask(&script, &instance, delete, None, &["abc"]).await;
        for row in [this, following, all] {
            assert!(matches!(row, ScopeRow::Offered { .. }), "{row:?}");
        }
        let one_off = json!({
            "id": "solo", "etag": "\"s1\"", "summary": "Dentist",
            "organizer": { "self": true },
            "start": { "dateTime": "2026-10-12T09:00:00+01:00" },
            "end": { "dateTime": "2026-10-12T10:00:00+01:00" }
        });
        let ask_one = |event: Value| async move {
            let script = Script::new(vec![Step::get("/events/solo", "", event.clone())]);
            choices(
                &script,
                &target(&event),
                EditingStatus::On,
                &mirror(&[]),
                &edit(title("Retro")),
                Some((&event, None)),
            )
            .await
        };
        assert_eq!(ask_one(one_off.clone()).await, SeriesChoices::NotRepeating);
        let now_a_series = with(one_off, "recurrence", json!(["RRULE:FREQ=DAILY"]));
        assert!(matches!(
            ask_one(now_a_series).await,
            SeriesChoices::Refused { result: WriteOutcome::Failed { message } }
                if message.starts_with("This became a repeating event in Google")
        ));
    }

    /// A listing is read page by page; a page that fails or that PM can't read, or the cap, leaves
    /// it incomplete with what came before.
    #[tokio::test]
    async fn a_listing_is_whole_only_when_its_last_page_says_so() {
        let item = |n: u32| json!({ "id": format!("e{n}") });
        let script = Script::new(vec![
            Step::get("/events", "maxResults=100", page(vec![item(1)], Some("p2"))),
            Step::get("/events", "pageToken=p2", page(vec![item(2)], None)),
        ]);
        let whole = listing(&script, 10).await;
        assert!(whole.complete);
        assert_eq!(whole.items, vec![item(1), item(2)]);
        for last in [
            Step::status("/events", "pageToken=p2", 500),
            Step::get(
                "/events",
                "pageToken=p2",
                json!({ "kind": "calendar#events" }),
            ),
        ] {
            let script = Script::new(vec![
                Step::get("/events", "", page(vec![item(1)], Some("p2"))),
                last,
            ]);
            let short = listing(&script, 10).await;
            assert!(!short.complete);
            assert_eq!(short.items, vec![item(1)]);
        }
        let endless = Script::new(vec![
            Step::get("/events", "", page(vec![item(1)], Some("p2"))),
            Step::get("/events", "", page(vec![item(2)], Some("p3"))),
        ]);
        let capped = listing(&endless, 2).await;
        endless.finished();
        assert!(!capped.complete);
        assert_eq!(capped.items.len(), 2);
    }

    /// The mirror names each series a calendar's occurrences belong to, once.
    #[test]
    fn the_mirror_lists_a_calendars_series() {
        let (_d, conn) = store();
        let cal = "gcal:me@x.com:primary";
        add_calendar(&conn, "gcal:me@x.com", "google", cal, Some("primary"));
        let row = |id: &str, series: Option<&str>| CalendarEvent {
            id: format!("{cal}:{id}"),
            calendar_id: cal.into(),
            summary: "Gym".into(),
            start: "2026-10-12T09:00:00Z".into(),
            series_id: series.map(str::to_string),
            ..Default::default()
        };
        let rows = [
            row("abc_1", Some("abc")),
            row("abc_2", Some("abc")),
            row("abc_R3_1", Some("abc_R3")),
            row("solo", None),
        ];
        calendar::replace_events(&conn, cal, &rows, true).unwrap();
        let facts = MirrorFacts::read(&conn, cal).unwrap();
        assert_eq!(facts.series_ids, vec!["abc", "abc_R3"]);
        assert!(facts.window.0 < facts.window.1);
        assert!(MirrorFacts::read(&conn, "other")
            .unwrap()
            .series_ids
            .is_empty());
    }

    /// C9a's probe R0 reads this report: its counts and field names, never a title, an id or an
    /// address.
    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn dev_series_report_returns_no_titles_ids_or_addresses() {
        let series = weekly();
        let instance = plain("2026-10-26T09:00:00Z");
        let retitled = with(
            plain("2026-10-12T09:00:00+01:00"),
            "summary",
            json!("Standup (room 4)"),
        );
        let moved_19 = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-20T09:00:00+01:00",
            "2026-10-20T10:00:00+01:00",
        );
        let gone_2 = cancelled("2026-11-02T09:00:00Z");
        let listed = vec![
            series.clone(),
            gone_2.clone(),
            moved_19.clone(),
            retitled.clone(),
        ];
        let window = vec![
            with(retitled, "updated", json!("2026-10-06T08:00:00.000Z")),
            moved_19,
            instance.clone(),
            gone_2,
        ];
        let report = report_of(&series, listed, window).await;
        let kinds: Vec<(&str, &str)> = report
            .exceptions
            .iter()
            .map(|e| (e.original.as_str(), e.kind.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("2026-10-12 09:00", "modified: carry=summary other="),
                ("2026-10-19 09:00", "modified: carry=time other="),
                ("2026-11-02 09:00", "cancelled"),
            ]
        );
        assert!(report.listing_complete && report.cross_checked);
        assert_eq!(
            (
                report.instances,
                report.instances_cancelled,
                report.modified_in_instances,
                // The cancelled one has no `updated`, and the retitled one its own.
                report.instances_updated_like_master
            ),
            (4, 1, 2, 2)
        );
        assert_eq!(report.list_etag_matches_get, Some(true));
        assert!(report.fields_missing_vs_master.is_empty());
        let text = serde_json::to_string(&report).unwrap();
        for leak in ["Standup", "Daily notes", "abc", "example.com", "m1"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }

        // Each assumption broken: an occurrence lacking a field its series has, the listing's copy
        // of the master at another etag, and a listing that ran to its end yet lacks a change.
        let placed = with(weekly(), "location", json!("Room 1"));
        let older = with(placed.clone(), "etag", json!("\"m0\""));
        let retitled = with(
            plain("2026-10-12T09:00:00+01:00"),
            "summary",
            json!("Standup (room 4)"),
        );
        let report = report_of(&placed, vec![older], vec![retitled, instance]).await;
        assert!(report.listing_complete && !report.cross_checked);
        assert_eq!(report.list_etag_matches_get, Some(false));
        assert_eq!(report.fields_missing_vs_master, ["location"]);
    }

    /// The dev report of `series`, listed by iCal UID as `listed` with `window` its occurrences, read
    /// whole.
    #[cfg(debug_assertions)]
    async fn report_of(series: &Value, listed: Vec<Value>, window: Vec<Value>) -> SeriesReport {
        let instance = plain("2026-10-26T09:00:00Z");
        let script = Script::new(vec![
            Step::get("/events/abc", "", series.clone()),
            Step::get("/events", "iCalUID=abc%40google.com", page(listed, None)),
            Step::get(
                "/events/abc/instances",
                "timeMin=2026-09-01 maxResults=100",
                page(window, None),
            ),
        ]);
        let gathered = gather(
            &script,
            &target(&instance),
            &instance,
            &mirror(&["abc"]),
            true,
        )
        .await
        .unwrap()
        .unwrap();
        script.finished();
        assert!(!script.sent()[2].url.contains("fields="));
        series_report(&gathered)
    }

    /// G9, R2 end to end: the editor opens on the occurrence read again after its series, the
    /// session keeps both, and the choices check against them, so a rename made in Google after the
    /// editor opened is a conflict for every row that sends the title.
    #[tokio::test]
    async fn the_editor_keeps_its_series_and_the_choices_check_against_it() {
        let (series, instance) = (weekly(), monday_19());
        // Moved to another room between the first read and the second: the editor shows the later.
        let later = with(instance.clone(), "location", json!("Room 4"));
        let edit_state = CalendarEditState::default();
        let script = Script::new(vec![
            Step::get("/events/abc", "", series.clone()),
            instance_read(&later),
        ]);
        let load = open_editor(
            &script,
            &edit_state,
            target(&instance),
            instance.clone(),
            EditingStatus::On,
            None,
        )
        .await
        .unwrap();
        script.finished();
        let EditLoad::Ready {
            session,
            series: info,
            event,
            ..
        } = load
        else {
            // Not `{load:?}`: it holds the session id, which CodeQL won't let a message print.
            panic!("the occurrence didn't open for editing");
        };
        assert_eq!(info.map(|i| i.repeats), Some("Weekly on Monday".into()));
        assert_eq!(event.location, "Room 4");
        let open = open_session(&edit_state, &session).unwrap().unwrap();
        assert_eq!(open.target, target(&instance));
        assert_eq!(open.opened(), (&later, Some(&series)));
        assert!(open_session(&edit_state, "not-a-session")
            .unwrap()
            .is_none());
        let renamed = |event: &Value| with(event.clone(), "summary", json!("Standup v2"));
        let script = Script::new(reads(
            &renamed(&instance),
            &renamed(&series),
            vec![renamed(&series)],
            vec![renamed(&instance)],
        ));
        let (this, following, all) = ask(
            &script,
            &instance,
            ChoiceRequest::Edit {
                session,
                draft: title("Retro"),
            },
            Some(open.opened()),
            &["abc"],
        )
        .await;
        let conflict = ScopeRow::Conflict {
            fields: vec!["summary".into()],
        };
        assert_eq!(
            (this, following, all),
            (conflict.clone(), conflict.clone(), conflict)
        );
    }

    /// The editor's copy is read after the series; a rename between the two reads is in it, and
    /// not in the series kept. Without that second read (a failure, or the occurrence now gone), no
    /// series is kept.
    #[tokio::test]
    async fn opening_reads_the_occurrence_again_after_its_series() {
        let (series, instance) = (weekly(), monday_19());
        let later = with(instance.clone(), "summary", json!("Standup v2"));
        let script = Script::new(vec![
            Step::get("/events/abc", "", series.clone()),
            instance_read(&later),
        ]);
        let (base, master) = open_series(&script, &target(&instance), instance.clone()).await;
        script.finished();
        assert_eq!((base, master), (later, Some(series.clone())));
        for second in [
            Step::status("/events/abc_20261019T080000Z", "", 503),
            instance_read(&with(instance.clone(), "status", json!("cancelled"))),
        ] {
            let script = Script::new(vec![Step::get("/events/abc", "", series.clone()), second]);
            let (base, master) = open_series(&script, &target(&instance), instance.clone()).await;
            script.finished();
            assert_eq!((base, master), (instance.clone(), None));
        }
    }

    /// C9 design review L13: the series the editor opened counts only if it's the one Google holds
    /// for this occurrence; otherwise the rows that need it say so.
    #[tokio::test]
    async fn the_editors_series_counts_only_if_its_the_one_google_holds() {
        let (series, instance) = (weekly(), monday_19());
        let elsewhere = with(series.clone(), "id", json!("xyz"));
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            vec![instance.clone()],
        ));
        let (this, following, all) = ask(
            &script,
            &instance,
            edit(title("Retro")),
            Some((&instance, Some(&elsewhere))),
            &["abc"],
        )
        .await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        let no_base = ScopeRow::Refused {
            message: SeriesRefusal::NoBase.to_string(),
        };
        assert_eq!((following, all), (no_base.clone(), no_base));
    }

    /// An edit without the copies the user opened is never planned against Google's alone: nothing
    /// is read, and the editor is asked to open again.
    #[tokio::test]
    async fn an_edit_without_its_opened_copies_is_refused() {
        let instance = monday_19();
        let script = Script::new(Vec::new());
        let got = choices(
            &script,
            &target(&instance),
            EditingStatus::On,
            &mirror(&["abc"]),
            &edit(title("Retro")),
            None,
        )
        .await;
        script.finished();
        assert_eq!(
            got,
            SeriesChoices::Refused {
                result: WriteOutcome::Failed {
                    message: SESSION_ENDED.into()
                }
            }
        );
    }

    /// The gate runs on Google's copy of the occurrence before any row, all of it but the stand-in
    /// for this very choice.
    #[tokio::test]
    async fn the_gate_refuses_before_any_row() {
        let instance = monday_19();
        let read_only = |reason| SeriesChoices::Refused {
            result: WriteOutcome::ReadOnly { reason },
        };
        let only = |event: &Value| vec![instance_read(event)];
        for editing in [
            EditingStatus::Off,
            EditingStatus::Paused,
            EditingStatus::NeedsConsent,
        ] {
            assert_eq!(
                refused(
                    only(&instance),
                    &instance,
                    editing,
                    "owner",
                    edit(title("Retro"))
                )
                .await,
                read_only(ReadOnlyReason::EditingOff)
            );
        }
        let delete = ChoiceRequest::Delete {
            event_id: "cal:abc_20261019T080000Z".into(),
            seen: seen_by_user(&fresh_event(instance.clone())),
        };
        assert_eq!(
            refused(
                only(&instance),
                &instance,
                EditingStatus::On,
                "reader",
                delete
            )
            .await,
            read_only(ReadOnlyReason::CalendarReadOnly)
        );
        let formatted = with(instance.clone(), "description", json!("<b>Notes</b>"));
        let notes = EventPatchDraft {
            description: Some("Notes".into()),
            ..Default::default()
        };
        assert_eq!(
            refused(
                only(&formatted),
                &formatted,
                EditingStatus::On,
                "owner",
                edit(notes)
            )
            .await,
            read_only(ReadOnlyReason::HtmlDescription)
        );
        let invitation = with(
            instance.clone(),
            "organizer",
            json!({ "email": "boss@example.com" }),
        );
        assert_eq!(
            refused(
                only(&invitation),
                &invitation,
                EditingStatus::On,
                "owner",
                edit(title("Retro"))
            )
            .await,
            read_only(ReadOnlyReason::NotOrganizer)
        );
    }

    /// Until C12, a row that would reach an occurrence with guests of its own is refused: listed by
    /// iCal UID (the planners) or only in Google's occurrences (here), and only from where it
    /// reaches. "This event" on another occurrence is still offered.
    #[tokio::test]
    async fn guests_on_another_occurrence_hold_back_the_rows_that_reach_it() {
        let (series, instance) = (weekly(), monday_19());
        let invited = |at: &str| {
            with(
                plain(at),
                "attendees",
                json!([{ "email": "a@example.com", "responseStatus": "needsAction" }]),
            )
        };
        let opened = Some((&instance, Some(&series)));
        let held = |day: &str| ScopeRow::Refused {
            message: format!(
                "The occurrence on {day} has guests of its own, and PM can't change events with \
                 guests yet. Choose \"This event\" to change only this one."
            ),
        };
        let later = invited("2026-10-26T09:00:00Z");
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), later.clone()],
            vec![instance.clone(), later.clone()],
        ));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!(
            (&following, &all),
            (&held("26-10-2026"), &held("26-10-2026"))
        );
        let delete = || ChoiceRequest::Delete {
            event_id: "cal:abc_20261019T080000Z".into(),
            seen: seen_by_user(&fresh_event(instance.clone())),
        };
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), later.clone()],
            vec![instance.clone(), later.clone()],
        ));
        let (this, following, all) = ask(&script, &instance, delete(), None, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!((following, all), (held("26-10-2026"), held("26-10-2026")));
        // Before the occurrence picked: "this and following" doesn't reach it.
        let earlier = invited("2026-10-12T09:00:00+01:00");
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), earlier.clone()],
            vec![earlier.clone(), instance.clone()],
        ));
        let (_, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert!(matches!(following, ScopeRow::Offered { .. }));
        assert_eq!(all, held("12-10-2026"));
        // Missing from the listing, seen only in Google's occurrences: the cross-check counts it
        // against the listing, which then can't show that nothing reached has guests.
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone()],
            vec![instance.clone(), later],
        ));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!((following, all), (incomplete(), incomplete()));
    }

    /// A change to every occurrence says how many were changed on their own and may keep or lose
    /// that; the dialog carries the planner's own `complete` (its "some" case is planned in
    /// plan.rs, for C12, when a short listing no longer refuses).
    #[tokio::test]
    async fn a_change_to_every_occurrence_says_how_many_may_reset() {
        let (series, instance) = (weekly(), monday_19());
        let opened = Some((&instance, Some(&series)));
        let retitled = with(
            plain("2026-10-26T09:00:00Z"),
            "summary",
            json!("Standup (room 4)"),
        );
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), retitled.clone()],
            vec![instance.clone(), retitled],
        ));
        let (_, _, all) = ask(&script, &instance, edit(title("Retro")), opened, &["abc"]).await;
        assert_eq!(
            warnings(&all),
            [SeriesWarningDto::ExceptionsMayReset {
                count: 1,
                complete: true
            }]
        );
    }

    /// DS16: a part split off earlier is said however PM knows it: Google's `_R` in the series' own
    /// id (its earlier part may have left the mirror's window), or PM's own note on it.
    #[tokio::test]
    async fn a_split_part_says_so_however_pm_knows() {
        let delete = |instance: &Value| ChoiceRequest::Delete {
            event_id: format!("cal:{}", instance["id"].as_str().unwrap()),
            seen: seen_by_user(&fresh_event(instance.clone())),
        };
        let part = "abc_R20261019T080000";
        let series = with(weekly(), "id", json!(part));
        let instance = with(
            plain("2026-10-26T09:00:00Z"),
            "recurringEventId",
            json!(part),
        );
        let script = Script::new(vec![
            instance_read(&instance),
            Step::get(&format!("/events/{part}"), "", series.clone()),
            Step::get(
                "/events",
                "iCalUID=abc%40google.com",
                page(vec![series.clone()], None),
            ),
            Step::get(
                &format!("/events/{part}/instances"),
                "timeMin=2026-09-01",
                page(vec![instance.clone()], None),
            ),
        ]);
        let (this, following, all) =
            ask(&script, &instance, delete(&instance), None, &[part]).await;
        assert!(warnings(&this).is_empty());
        assert_eq!(warnings(&following), [SeriesWarningDto::SplitSibling]);
        assert_eq!(warnings(&all), [SeriesWarningDto::SplitSibling]);
        let tail = with(
            weekly(),
            "extendedProperties",
            json!({ "private": { "pmSplitOf": "older" } }),
        );
        let instance = monday_19();
        let script = Script::new(reads(
            &instance,
            &tail,
            vec![tail.clone()],
            vec![instance.clone()],
        ));
        let (_, following, all) = ask(&script, &instance, delete(&instance), None, &["abc"]).await;
        assert_eq!(warnings(&following), [SeriesWarningDto::SplitSibling]);
        assert_eq!(warnings(&all), [SeriesWarningDto::SplitSibling]);
    }

    // --- sending a change to a repeating event (C9a-3a): Google scripted, a temp mirror ---

    use crate::calendar_write::series::fixtures::Case;
    use std::sync::Arc;

    /// When PM's writes in these tests reach Google: the `updated` of every copy after one.
    const SAVED_AT: &str = "2026-10-17T12:00:00.000Z";

    /// The app as a series write sees it: a temp mirror holding calendar "cal" (Google's
    /// [`CALENDAR`], as [`target`] names it), the editing state, and every calendar it was told
    /// changed. Its DB lock panics when taken twice, so a nested lock (L11) fails rather than hangs.
    struct TestHost {
        db: Mutex<Connection>,
        edit: CalendarEditState,
        told: Mutex<Vec<String>>,
        _dir: tempfile::TempDir,
    }

    impl TestHost {
        /// A host whose mirror holds `events`, as a sync that read them whole writes them.
        fn holding(events: &[Value]) -> Arc<TestHost> {
            let (dir, conn) = store();
            add_calendar(
                &conn,
                "gcal:me@example.com",
                "google",
                "cal",
                Some(CALENDAR),
            );
            calendar::replace_events(&conn, "cal", &rows_of(events), true).unwrap();
            Arc::new(TestHost {
                db: Mutex::new(conn),
                edit: CalendarEditState::default(),
                told: Mutex::new(Vec::new()),
                _dir: dir,
            })
        }

        /// The series' rows, in time order.
        fn rows(&self) -> Vec<CalendarEvent> {
            self.with_conn(|conn| calendar::series_rows(conn, "cal", "abc"))
                .unwrap()
        }

        fn ids(&self) -> Vec<String> {
            ids_of(self.rows())
        }

        fn row(&self, event: &str) -> CalendarEvent {
            self.with_conn(|conn| calendar::event_by_id(conn, &format!("cal:{event}")))
                .unwrap()
                .unwrap()
        }

        fn told(&self) -> Vec<String> {
            self.told.lock().unwrap().clone()
        }

        fn nothing_recorded(&self) -> bool {
            self.edit.recent.lock().unwrap().is_empty()
        }

        /// The calendar's sync hash is gone, so the next sync rewrites its rows.
        fn hash_dropped(&self) -> bool {
            self.with_conn(|conn| crate::db::get_setting(conn, "calendar_events_hash:cal"))
                .unwrap()
                .is_none()
        }

        /// What a whole sync that began at `started` and listed `listed` shows, once the recent
        /// writes are laid over it.
        fn synced(&self, listed: &[Value], started: Instant) -> Vec<CalendarEvent> {
            self.edit
                .recent
                .lock()
                .unwrap()
                .merge("cal", rows_of(listed), true, started, Stamp::now())
                .rows
        }
    }

    impl Host for TestHost {
        fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
            match self.db.try_lock() {
                Ok(conn) => f(&conn),
                Err(std::sync::TryLockError::WouldBlock) => panic!("DB lock taken twice"),
                Err(std::sync::TryLockError::Poisoned(_)) => panic!("DB lock poisoned"),
            }
        }

        fn edit(&self) -> &CalendarEditState {
            &self.edit
        }

        fn landed(&self, calendar_id: &str) {
            self.told.lock().unwrap().push(calendar_id.to_string());
        }

        fn mirror(&self, calendar_id: &str) -> Result<MirrorFacts> {
            let series_ids = self.with_conn(|conn| calendar::series_ids(conn, calendar_id))?;
            Ok(MirrorFacts {
                series_ids,
                window: mirror(&[]).window,
            })
        }
    }

    fn rows_of(events: &[Value]) -> Vec<CalendarEvent> {
        events
            .iter()
            .filter_map(|e| calendar::parse_event("cal", e))
            .collect()
    }

    fn ids_of(rows: Vec<CalendarEvent>) -> Vec<String> {
        rows.into_iter().map(|r| r.id).collect()
    }

    fn sorted(mut ids: Vec<String>) -> Vec<String> {
        ids.sort();
        ids
    }

    /// The series' occurrences in October, as the mirror and Google first hold them.
    fn october() -> Vec<Value> {
        vec![
            plain("2026-10-12T09:00:00+01:00"),
            monday_19(),
            plain("2026-10-26T09:00:00Z"),
        ]
    }

    /// `event` as Google holds it after a write: `fields` set, at `etag`, updated at [`SAVED_AT`].
    fn written(event: &Value, fields: &[(&str, Value)], etag: &str) -> Value {
        let mut v = event.clone();
        for (key, value) in fields {
            v[*key] = value.clone();
        }
        v["etag"] = json!(etag);
        v["updated"] = json!(SAVED_AT);
        v
    }

    fn saved() -> WriteOutcome {
        WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
        }
    }

    fn pending() -> WriteOutcome {
        WriteOutcome::Saved {
            warnings: vec![WriteWarning::MirrorRefreshPending],
            left_behind: Vec::new(),
        }
    }

    /// A series write to `target` against `host` through `script`: editing on, and a save may leave
    /// out its choice (a dev build).
    fn writer<'a>(
        script: &'a Script,
        host: &'a TestHost,
        target: &'a Target,
    ) -> SeriesWrite<'a, Script, TestHost> {
        SeriesWrite {
            wire: script,
            host,
            target,
            editing: EditingStatus::On,
            missing_ok: true,
        }
    }

    /// An editor opened on `instance` of `series`, kept as the editor's load keeps it.
    fn open_in(host: &TestHost, instance: &Value, series: &Value) -> (String, EditSession) {
        let session = keep_session(
            &host.edit,
            target(instance),
            instance.clone(),
            Some(series.clone()),
        )
        .unwrap();
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        (session, open)
    }

    /// Save `draft` for `scope` from an editor opened on `instance` of `series`, Google answering
    /// as `script` says: the outcome, and the editor's session.
    async fn save(
        host: &TestHost,
        script: &Script,
        instance: &Value,
        series: &Value,
        draft: EventPatchDraft,
        scope: Scope,
    ) -> (WriteOutcome, String) {
        let (session, open) = open_in(host, instance, series);
        let target = target(instance);
        let outcome = save_series(
            &writer(script, host, &target),
            &session,
            open,
            draft,
            scope,
            None,
        )
        .await;
        (outcome, session)
    }

    /// A delete of `instance` for `scope`: planned before its Undo window, sent after it.
    async fn held_delete(
        host: &TestHost,
        script: &Script,
        instance: &Value,
        scope: Scope,
    ) -> WriteOutcome {
        let target = target(instance);
        let ctx = writer(script, host, &target);
        let seen = seen_by_user(&fresh_event(instance.clone()));
        let (_claim, held) = match plan_held_delete(&ctx, &target.row_id, &seen, scope, None).await
        {
            Ok(planned) => planned,
            Err(outcome) => return outcome,
        };
        send_held_series_delete(&ctx, &target.row_id, &seen, scope, &held).await
    }

    /// The reads of the October series a send makes, from Monday 19-10.
    fn october_reads() -> Vec<Step> {
        reads(&monday_19(), &weekly(), vec![weekly()], october())
    }

    /// The series re-fetched over the mirror's window after a write: whole, 50 a page.
    fn refetch_read(answer: Scripted) -> Step {
        Step::new(
            Method::Get,
            "/events/abc/instances",
            "timeMin=2026-09-01T00%3A00%3A00Z timeMax=2027-11-01T00%3A00%3A00Z maxResults=50",
            answer,
        )
    }

    /// The PATCH of the series itself, on the etag it was read at.
    fn series_patch(answer: Scripted) -> Step {
        Step::write(Method::Patch, "/events/abc", "\"m1\"", answer)
    }

    fn retitled(event: &Value, etag: &str) -> Value {
        written(event, &[("summary", json!("Retro"))], etag)
    }

    fn retitled_october() -> Vec<Value> {
        october()
            .iter()
            .enumerate()
            .map(|(i, e)| retitled(e, &format!("\"r{i}\"")))
            .collect()
    }

    /// "All events" an hour later, from Monday 19-10 at 10:00: Google's series after it, and its
    /// occurrences, each under a new id.
    fn an_hour_later() -> (Value, Vec<Value>) {
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::All, at("10:00", "2026-10-19"))
            .unwrap();
        let Op::Patch { body, .. } = &plan.steps[0].op else {
            panic!("not a patch");
        };
        let fields: Vec<(&str, Value)> =
            body.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let series = written(&weekly(), &fields, "\"m2\"");
        let occurrences = [
            "2026-10-12T10:00:00+01:00",
            "2026-10-19T10:00:00+01:00",
            "2026-10-26T10:00:00Z",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, start)| written(&plain(start), &[], &format!("\"n{i}\"")))
        .collect();
        (series, occurrences)
    }

    /// Save "all events an hour later", Google answering the PATCH with the moved series, then
    /// `refetches`.
    async fn retime(host: &TestHost, refetches: Vec<Step>) -> (WriteOutcome, Script) {
        let (moved_series, _) = an_hour_later();
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(moved_series)));
        steps.extend(refetches);
        let script = Script::new(steps);
        let draft = at("10:00", "2026-10-19");
        let (outcome, _) = save(host, &script, &monday_19(), &weekly(), draft, Scope::All).await;
        script.asked_all();
        (outcome, script)
    }

    /// An occurrence after Monday 19-10 renamed on its own, so stored: a cut leaves it in the old
    /// series unless it's cancelled too.
    fn off_site() -> Value {
        with(plain("2026-11-02T09:00:00Z"), "summary", json!("Off-site"))
    }

    /// The October series with [`off_site`], as the mirror holds it and a delete from Monday 19-10
    /// reads it.
    fn with_off_site() -> (Vec<Value>, Vec<Step>) {
        let mut events = october();
        events.push(off_site());
        let window = vec![monday_19(), plain("2026-10-26T09:00:00Z"), off_site()];
        let steps = reads(&monday_19(), &weekly(), vec![weekly(), off_site()], window);
        (events, steps)
    }

    /// Google's series after the cut a "this and following" delete from Monday 19-10 sends, the
    /// series holding `exceptions`.
    fn cut_series(exceptions: Vec<Value>) -> Value {
        let plan = Case::new(weekly(), monday_19())
            .exceptions(exceptions)
            .delete(Scope::Following)
            .unwrap();
        let Op::Patch { body, .. } = &plan.steps[0].op else {
            panic!("not a cut");
        };
        written(
            &weekly(),
            &[("recurrence", body["recurrence"].clone())],
            "\"m2\"",
        )
    }

    /// "All events" moved to Tuesday from Monday 19-10, and Google's series holding half of it: the
    /// new start, the old rule.
    fn half_way_to_tuesday() -> (EventPatchDraft, Value) {
        let to_tuesday = at("09:00", "2026-10-20");
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::All, to_tuesday.clone())
            .unwrap();
        let Op::Patch { body, .. } = &plan.steps[0].op else {
            panic!("not a patch");
        };
        let half = written(
            &weekly(),
            &[
                ("start", body["start"].clone()),
                ("end", body["end"].clone()),
            ],
            "\"m2\"",
        );
        (to_tuesday, half)
    }

    /// §5: "this event" lands Google's reply in its row, no re-fetch, and moves the editor's copy
    /// on for the fields it sent; the editor stays open, the series being as it was.
    #[tokio::test]
    async fn this_event_save_lands_googles_reply_in_the_row() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let etag = instance["etag"].as_str().unwrap();
        let mut steps = october_reads();
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            etag,
            Scripted::Json(retitled(&instance, "\"i2\"")),
        ));
        let script = Script::new(steps);
        let (outcome, session) = save(
            &host,
            &script,
            &instance,
            &series,
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert_eq!(script.sent()[4].body, Some(json!({ "summary": "Retro" })));
        let row = host.row("abc_20261019T080000Z");
        assert_eq!(
            (row.summary.as_str(), row.etag.as_deref()),
            ("Retro", Some("\"i2\""))
        );
        assert_eq!(host.told(), ["cal"]);
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        assert_eq!(open.base["summary"], "Retro");
        assert_eq!(open.base["etag"], instance["etag"]);
    }

    /// §5: deleting "this event" cancels it, never a DELETE; its row goes, and a sync that lags
    /// can't bring it back.
    #[tokio::test]
    async fn this_delete_cancels_never_deletes() {
        let host = TestHost::holding(&october());
        let instance = monday_19();
        let mut steps = october_reads();
        steps.extend(october_reads());
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            instance["etag"].as_str().unwrap(),
            Scripted::Json(written(
                &instance,
                &[("status", json!("cancelled"))],
                "\"i2\"",
            )),
        ));
        let script = Script::new(steps);
        assert_eq!(
            held_delete(&host, &script, &instance, Scope::This).await,
            saved()
        );
        script.asked_all();
        let sent = script.sent();
        assert!(sent.iter().all(|r| r.method != Method::Delete));
        assert_eq!(sent[8].body.as_ref().unwrap()["status"], "cancelled");
        assert_eq!(
            host.ids(),
            ["cal:abc_20261012T080000Z", "cal:abc_20261026T090000Z"]
        );
        let shown = ids_of(host.synced(&october(), Instant::now()));
        assert!(!shown.contains(&"cal:abc_20261019T080000Z".to_string()));
    }

    /// §5: "all events" PATCHes the series, re-fetches its occurrences (the mirror's window, 50 a
    /// page) and records every row, so a sync that began before the save can't put the old title
    /// back. The editor's copies no longer stand for Google's, so its session ends.
    #[tokio::test]
    async fn all_events_retitle_refetches_and_records_every_row() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(retitled(&series, "\"m2\""))));
        steps.push(refetch_read(Scripted::Json(page(retitled_october(), None))));
        let script = Script::new(steps);
        let before = Instant::now();
        let (outcome, session) = save(
            &host,
            &script,
            &instance,
            &series,
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert!(script.waits().is_empty());
        assert_eq!(script.sent()[4].body, Some(json!({ "summary": "Retro" })));
        let rows = host.rows();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.summary == "Retro"));
        assert_eq!(host.told(), ["cal"]);
        assert!(open_session(&host.edit, &session).unwrap().is_none());
        assert!(host
            .synced(&october(), before)
            .iter()
            .all(|r| r.summary == "Retro"));
    }

    /// §5, §0.6: an "all events" move gives every occurrence a new id. The new rows are recorded as
    /// created, so a whole sync that lags behind the move keeps them and hides the old ones, whether
    /// it began before the save or after.
    #[tokio::test]
    async fn a_retime_records_new_ids_as_created_so_a_lagging_complete_sync_keeps_them() {
        let host = TestHost::holding(&october());
        let (_, moved) = an_hour_later();
        let before = Instant::now();
        let refetch = refetch_read(Scripted::Json(page(moved.clone(), None)));
        let (outcome, script) = retime(&host, vec![refetch]).await;
        assert_eq!(outcome, saved());
        assert!(script.waits().is_empty());
        let new_ids = ids_of(rows_of(&moved));
        assert_eq!(host.ids(), new_ids);
        for started in [before, Instant::now()] {
            assert_eq!(sorted(ids_of(host.synced(&october(), started))), new_ids);
        }
    }

    /// §5 (A29): an old row is removed at the write's own `updated`: listed no later than that
    /// under another etag, it's still the version removed; later, it was changed in Google since,
    /// and comes back.
    #[tokio::test]
    async fn old_ids_are_hidden_at_the_writes_updated() {
        let host = TestHost::holding(&october());
        let (_, moved) = an_hour_later();
        let (outcome, _) =
            retime(&host, vec![refetch_read(Scripted::Json(page(moved, None)))]).await;
        assert_eq!(outcome, saved());
        let old = "cal:abc_20261019T080000Z".to_string();
        let at_the_write = written(&monday_19(), &[], "\"o2\"");
        assert!(!ids_of(host.synced(&[at_the_write], Instant::now())).contains(&old));
        let later = with(
            written(&monday_19(), &[], "\"o3\""),
            "updated",
            json!("2026-10-17T12:00:01.000Z"),
        );
        assert!(ids_of(host.synced(&[later], Instant::now())).contains(&old));
    }

    /// Review M4: a sync that lands while the save is on its way writes the new ids first; they
    /// are still recorded as created (new since the rows read before the first send), so a sync that
    /// lags behind keeps them.
    #[tokio::test]
    async fn a_sync_between_the_save_and_its_landing_still_leaves_new_ids_created() {
        let host = TestHost::holding(&october());
        let (moved_series, moved) = an_hour_later();
        let (syncing, synced_rows) = (Arc::clone(&host), rows_of(&moved));
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(moved_series)).then(move || {
            syncing
                .with_conn(|conn| calendar::replace_events(conn, "cal", &synced_rows, true))
                .unwrap();
        }));
        steps.push(refetch_read(Scripted::Json(page(moved.clone(), None))));
        let script = Script::new(steps);
        let draft = at("10:00", "2026-10-19");
        let (outcome, _) = save(&host, &script, &monday_19(), &weekly(), draft, Scope::All).await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert_eq!(
            sorted(ids_of(host.synced(&october(), Instant::now()))),
            ids_of(rows_of(&moved))
        );
    }

    /// §5, A14: a re-fetch that doesn't show the write yet is tried again after 1 s and 3 s; still
    /// not, nothing from it is recorded, the rows wait for the next sync, and the save says so.
    #[tokio::test]
    async fn a_stale_refetch_retries_1s_3s_then_records_nothing_and_is_pending() {
        let host = TestHost::holding(&october());
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(retitled(&weekly(), "\"m2\""))));
        for _ in 0..3 {
            steps.push(refetch_read(Scripted::Json(page(october(), None))));
        }
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, pending());
        assert_eq!(script.waits(), REFETCH_AGAIN);
        assert!(host.nothing_recorded() && host.hash_dropped());
        assert!(host.rows().iter().all(|r| r.summary == "Standup"));
        assert_eq!(host.told(), ["cal"]);
    }

    /// Review M8: after a move that gives occurrences new ids, only a whole re-fetch lands, or old
    /// and new rows would both show.
    #[tokio::test]
    async fn an_incomplete_refetch_after_a_retime_is_pending_never_half_landed() {
        let host = TestHost::holding(&october());
        let (_, moved) = an_hour_later();
        let mut refetches = Vec::new();
        for _ in 0..3 {
            refetches.push(refetch_read(Scripted::Json(page(
                moved.clone(),
                Some("p2"),
            ))));
            refetches.push(Step::status("/events/abc/instances", "pageToken=p2", 500));
        }
        let (outcome, script) = retime(&host, refetches).await;
        assert_eq!(outcome, pending());
        assert_eq!(script.waits(), REFETCH_AGAIN);
        assert!(host.nothing_recorded());
        assert_eq!(host.ids(), ids_of(rows_of(&october())));
    }

    /// §5, review M7, A29: after a whole series' delete its occurrences' listing answers 410, which
    /// reads as nothing left; every row goes, no older than a second after Google's answer.
    #[tokio::test]
    async fn instances_410_after_a_whole_series_delete_reads_as_absent() {
        let host = TestHost::holding(&october());
        let date = chrono::DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut steps = october_reads();
        steps.extend(october_reads());
        steps.push(Step::write(
            Method::Delete,
            "/events/abc",
            "\"m1\"",
            Scripted::Reply {
                status: 204,
                body: None,
                date: Some(date),
                maybe_applied: false,
            },
        ));
        steps.push(refetch_read(Scripted::Status(410)));
        let script = Script::new(steps);
        assert_eq!(
            held_delete(&host, &script, &monday_19(), Scope::All).await,
            saved()
        );
        script.asked_all();
        assert!(script.waits().is_empty());
        assert!(host.ids().is_empty());
        // Listed within the second of Google's answer, under another etag, it's still the version
        // deleted (a `Date` is whole seconds); two seconds on, it was restored since.
        let at_answer = with(
            with(monday_19(), "etag", json!("\"x8\"")),
            "updated",
            json!("2026-10-17T12:00:00.500Z"),
        );
        let restored = with(
            with(plain("2026-10-26T09:00:00Z"), "etag", json!("\"x9\"")),
            "updated",
            json!("2026-10-17T12:00:02Z"),
        );
        let shown = ids_of(host.synced(&[at_answer, restored], Instant::now()));
        assert_eq!(shown, ["cal:abc_20261026T090000Z"]);
    }

    /// Review M7: after an edit, a 404 for the series' occurrences isn't proof of anything: the
    /// rows stay for the next sync.
    #[tokio::test]
    async fn a_refetch_404_after_an_all_events_edit_is_pending_not_absent() {
        let host = TestHost::holding(&october());
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(retitled(&weekly(), "\"m2\""))));
        for _ in 0..3 {
            steps.push(refetch_read(Scripted::Status(404)));
        }
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, pending());
        assert_eq!(host.ids(), ids_of(rows_of(&october())));
    }

    /// Review M7 where it matters: a cut's re-fetch answered 404 isn't "nothing left", which would
    /// take the rows before the cut off too. Pending, and only the cut's own removals land.
    #[tokio::test]
    async fn a_cut_delete_refetch_404_is_pending_never_absent() {
        let host = TestHost::holding(&october());
        let mut steps = october_reads();
        steps.extend(october_reads());
        steps.push(series_patch(Scripted::Json(cut_series(Vec::new()))));
        for _ in 0..3 {
            steps.push(refetch_read(Scripted::Status(404)));
        }
        let script = Script::new(steps);
        let outcome = held_delete(&host, &script, &monday_19(), Scope::Following).await;
        script.asked_all();
        assert_eq!(outcome, pending());
        assert_eq!(script.waits(), REFETCH_AGAIN);
        assert_eq!(host.ids(), ["cal:abc_20261012T080000Z"]);
        let shown = ids_of(host.synced(&october(), Instant::now()));
        assert_eq!(shown, ["cal:abc_20261012T080000Z"]);
    }

    /// Critique 3: a "this event" cancel proved without a reply (a lost answer, then a 404) is
    /// floored at Google's copy, which may be newer than the mirror's; one answered 404 directly, at
    /// a second after its `Date`.
    #[tokio::test]
    async fn a_this_delete_proved_without_a_reply_is_floored_at_googles_copy() {
        // Changed in Google since the last sync, outside what the user saw.
        let newer = written(
            &monday_19(),
            &[("description", json!("New notes"))],
            "\"i9\"",
        );
        let reads_of = || {
            reads(
                &newer,
                &weekly(),
                vec![weekly(), newer.clone()],
                vec![newer.clone()],
            )
        };
        let id = "/events/abc_20261019T080000Z";
        let date = chrono::DateTime::parse_from_rfc3339("2026-10-17T13:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let cases = [
            (
                vec![
                    Step::write(
                        Method::Patch,
                        id,
                        "\"i9\"",
                        Scripted::Lost { retried: false },
                    ),
                    Step::status(id, "", 404),
                ],
                SAVED_AT,
                "2026-10-17T12:00:01.000Z",
            ),
            (
                vec![Step::write(
                    Method::Patch,
                    id,
                    "\"i9\"",
                    Scripted::Reply {
                        status: 404,
                        body: None,
                        date: Some(date),
                        maybe_applied: false,
                    },
                )],
                "2026-10-17T13:00:00.500Z",
                "2026-10-17T13:00:02.000Z",
            ),
        ];
        for (cancel, hidden_at, back_at) in cases {
            let host = TestHost::holding(&october());
            let before = Instant::now();
            let mut steps = reads_of();
            steps.extend(reads_of());
            steps.extend(cancel);
            let script = Script::new(steps);
            assert_eq!(
                held_delete(&host, &script, &newer, Scope::This).await,
                saved()
            );
            script.asked_all();
            let row = "cal:abc_20261019T080000Z".to_string();
            assert!(!host.ids().contains(&row));
            let listed = |updated: &str, etag: &str| {
                with(
                    with(newer.clone(), "updated", json!(updated)),
                    "etag",
                    json!(etag),
                )
            };
            let hidden = ids_of(host.synced(&[listed(hidden_at, "\"j1\"")], before));
            assert!(!hidden.contains(&row), "{hidden_at}");
            let back = ids_of(host.synced(&[listed(back_at, "\"j2\"")], Instant::now()));
            assert!(back.contains(&row), "{back_at}");
        }
    }

    /// Review findings 5 and A29: a series gone when "all events" reaches it takes every row off,
    /// each no older than the occurrences changed on their own, which don't move the series'
    /// `updated`, so a sync already running can't bring one back.
    #[tokio::test]
    async fn a_gone_series_is_floored_at_its_changed_occurrences() {
        let host = TestHost::holding(&october());
        let demo = written(
            &plain("2026-10-26T09:00:00Z"),
            &[("summary", json!("Demo"))],
            "\"d2\"",
        );
        let mut steps = reads(
            &monday_19(),
            &weekly(),
            vec![weekly(), demo.clone()],
            vec![monday_19(), demo.clone()],
        );
        steps.push(series_patch(Scripted::Status(404)));
        let script = Script::new(steps);
        let before = Instant::now();
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, WriteOutcome::Gone);
        assert!(host.ids().is_empty());
        assert!(host.synced(&[demo], before).is_empty());
    }

    /// Critique 7: a move whose answers prove nothing may have landed in part (the new start
    /// without the rule), so it says to check Google, never only to refresh: lost twice, or its
    /// put-back lost.
    #[tokio::test]
    async fn an_unknown_retime_says_it_may_have_moved_partly() {
        let (to_tuesday, half) = half_way_to_tuesday();
        let look = || Step::get("/events/abc", "", weekly());
        let cases = [
            (
                at("10:00", "2026-10-19"),
                vec![
                    series_patch(Scripted::Lost { retried: false }),
                    look(),
                    series_patch(Scripted::Lost { retried: false }),
                    look(),
                ],
            ),
            (
                to_tuesday,
                vec![
                    series_patch(Scripted::Reply {
                        status: 500,
                        body: None,
                        date: None,
                        maybe_applied: true,
                    }),
                    Step::get("/events/abc", "", half),
                    Step::write(
                        Method::Patch,
                        "/events/abc",
                        "\"m2\"",
                        Scripted::Lost { retried: false },
                    ),
                ],
            ),
        ];
        for (draft, answers) in cases {
            let host = TestHost::holding(&october());
            let mut steps = october_reads();
            steps.extend(answers);
            let script = Script::new(steps);
            let (outcome, _) =
                save(&host, &script, &monday_19(), &weekly(), draft, Scope::All).await;
            script.asked_all();
            assert_eq!(
                outcome,
                WriteOutcome::Failed {
                    message: RETIME_UNSURE.into()
                }
            );
            assert!(host.nothing_recorded() && host.hash_dropped());
        }
    }

    /// §2.3: "this and following" without a new series cuts the series, then cancels the stored
    /// occurrences after the cut. One changed in Google since is left, said, and its row stays.
    #[tokio::test]
    async fn a_following_delete_cuts_then_cancels_and_reports_what_it_left() {
        let (events, reads_now) = with_off_site();
        let host = TestHost::holding(&events);
        let cut = cut_series(vec![off_site()]);
        let moved_on = written(
            &off_site(),
            &[("summary", json!("Off-site (room 2)"))],
            "\"q2\"",
        );
        let (_, again) = with_off_site();
        let mut steps = reads_now;
        steps.extend(again);
        steps.extend([
            series_patch(Scripted::Json(cut)),
            Step::write(
                Method::Patch,
                "/events/abc_20261102T090000Z",
                off_site()["etag"].as_str().unwrap(),
                Scripted::Status(412),
            ),
            Step::get("/events/abc_20261102T090000Z", "", moved_on.clone()),
            refetch_read(Scripted::Json(page(
                vec![october()[0].clone(), moved_on],
                None,
            ))),
        ]);
        let script = Script::new(steps);
        let outcome = held_delete(&host, &script, &monday_19(), Scope::Following).await;
        script.asked_all();
        assert_eq!(
            outcome,
            WriteOutcome::Saved {
                warnings: vec![WriteWarning::LeftOnOldSeries],
                left_behind: vec!["2026-11-02".into()],
            }
        );
        assert_eq!(
            host.ids(),
            ["cal:abc_20261012T080000Z", "cal:abc_20261102T090000Z"]
        );
        assert_eq!(
            host.row("abc_20261102T090000Z").summary,
            "Off-site (room 2)"
        );
    }

    /// Review M6: a move Google took half of (the new start, the old rule) is put back on Google's
    /// etag, emailing nobody, and said so; no re-fetch, the next sync rewrites the rows, and the
    /// editor is asked to open again.
    #[tokio::test]
    async fn a_half_landed_retime_is_put_back_and_says_so() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let (to_tuesday, half) = half_way_to_tuesday();
        let mut steps = october_reads();
        steps.extend([
            series_patch(Scripted::Reply {
                status: 500,
                body: None,
                date: None,
                maybe_applied: true,
            }),
            Step::get("/events/abc", "", half),
            Step::write(
                Method::Patch,
                "/events/abc",
                "\"m2\"",
                Scripted::Json(written(&series, &[], "\"m3\"")),
            ),
        ]);
        let script = Script::new(steps);
        let (outcome, session) =
            save(&host, &script, &instance, &series, to_tuesday, Scope::All).await;
        script.asked_all();
        assert_eq!(
            outcome,
            WriteOutcome::Failed {
                message: PUT_BACK.into()
            }
        );
        let restore = script.sent().last().unwrap().clone();
        assert_eq!(restore.body.unwrap()["recurrence"], series["recurrence"]);
        assert!(host.nothing_recorded() && host.hash_dropped());
        assert_eq!(host.told(), ["cal"]);
        assert!(open_session(&host.edit, &session).unwrap().is_none());
    }

    /// Google answered 200 and kept what was there: said so, and the mirror is as it was.
    #[tokio::test]
    async fn google_ignoring_a_change_says_so_and_changes_nothing() {
        let host = TestHost::holding(&october());
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(written(
            &weekly(),
            &[],
            "\"m2\"",
        ))));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, WriteOutcome::IgnoredByGoogle { split: false });
        assert!(host.rows().iter().all(|r| r.summary == "Standup"));
        assert!(host.nothing_recorded() && !host.hash_dropped() && host.told().is_empty());
    }

    /// No answer, Google still showing what was there, and no answer to the once-more either: it
    /// may yet land, so nothing is recorded and the next sync rewrites the rows.
    #[tokio::test]
    async fn an_answer_lost_twice_is_unconfirmed_and_drops_the_hash() {
        let host = TestHost::holding(&october());
        let look = || Step::get("/events/abc", "", weekly());
        let mut steps = october_reads();
        steps.extend([
            series_patch(Scripted::Lost { retried: false }),
            look(),
            series_patch(Scripted::Lost { retried: false }),
            look(),
        ]);
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, WriteOutcome::Unconfirmed);
        assert!(host.nothing_recorded() && host.hash_dropped());
    }

    /// A copy that never reached Google proves the write not done: the outcome says why, and nothing
    /// changes, the editor still open.
    #[tokio::test]
    async fn an_unsent_write_says_why_and_changes_nothing() {
        for (why, expected) in [
            (Unsent::SignIn, WriteOutcome::Reauth),
            (
                Unsent::Offline,
                WriteOutcome::Failed {
                    message: OFFLINE.into(),
                },
            ),
            (
                Unsent::Local,
                WriteOutcome::Failed {
                    message: NOT_SENT.into(),
                },
            ),
        ] {
            let host = TestHost::holding(&october());
            let mut steps = october_reads();
            steps.push(series_patch(Scripted::Unsent(why)));
            let script = Script::new(steps);
            let (outcome, session) = save(
                &host,
                &script,
                &monday_19(),
                &weekly(),
                title("Retro"),
                Scope::All,
            )
            .await;
            script.asked_all();
            assert_eq!(outcome, expected, "{why:?}");
            assert!(host.rows().iter().all(|r| r.summary == "Standup"));
            assert!(host.nothing_recorded() && !host.hash_dropped() && host.told().is_empty());
            assert!(open_session(&host.edit, &session).unwrap().is_some());
        }
    }

    /// Design critique 1: until C12 a guest who appeared in Google before the resend a 412 calls for
    /// is a conflict, whatever the step watches; nothing is sent again.
    #[tokio::test]
    async fn a_guest_added_before_a_resend_is_a_conflict() {
        let host = TestHost::holding(&october());
        let invited = written(
            &weekly(),
            &[("attendees", json!([{ "email": "a@example.com" }]))],
            "\"m2\"",
        );
        let mut steps = october_reads();
        steps.extend([
            series_patch(Scripted::Status(412)),
            Step::get("/events/abc", "", invited),
        ]);
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(
            outcome,
            WriteOutcome::Conflict {
                fields: vec!["attendees".into()]
            }
        );
        let writes = script
            .sent()
            .iter()
            .filter(|r| r.method == Method::Patch)
            .count();
        assert_eq!(writes, 1);
    }

    /// Design critique: the cut that ends a series is the riskiest step here. Ignored, unknown or
    /// never sent, no cancel follows it and no row changes.
    #[tokio::test]
    async fn an_ignored_unknown_or_unsent_cut_sends_no_cancels_and_changes_no_rows() {
        let look = || Step::get("/events/abc", "", weekly());
        let cases = [
            (
                vec![series_patch(Scripted::Json(written(
                    &weekly(),
                    &[],
                    "\"m2\"",
                )))],
                WriteOutcome::IgnoredByGoogle { split: false },
            ),
            (
                vec![
                    series_patch(Scripted::Lost { retried: false }),
                    look(),
                    series_patch(Scripted::Lost { retried: false }),
                    look(),
                ],
                WriteOutcome::Unconfirmed,
            ),
            (
                vec![series_patch(Scripted::Unsent(Unsent::Offline))],
                WriteOutcome::Failed {
                    message: OFFLINE.into(),
                },
            ),
        ];
        for (cut, expected) in cases {
            let (events, mut steps) = with_off_site();
            let host = TestHost::holding(&events);
            steps.extend(with_off_site().1);
            steps.extend(cut);
            let script = Script::new(steps);
            let outcome = held_delete(&host, &script, &monday_19(), Scope::Following).await;
            script.asked_all();
            assert_eq!(outcome, expected);
            assert!(!script
                .sent()
                .iter()
                .any(|r| r.method != Method::Get && r.url.contains("abc_20261102")));
            assert_eq!(host.ids(), ids_of(rows_of(&events)));
            assert!(host.nothing_recorded());
        }
    }

    /// §5, L2: a second write to a series, from any of its occurrences, or to the row, is Busy
    /// before anything is read; one to another series isn't held up.
    #[tokio::test]
    async fn two_writes_to_one_series_are_busy() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let target = target(&instance);
        let seen = seen_by_user(&fresh_event(instance.clone()));
        for held in [series_key(CALENDAR, "abc"), target.row_id.clone()] {
            let _other = InFlight::claim_all(&host.edit, std::slice::from_ref(&held)).unwrap();
            let script = Script::new(Vec::new());
            let (outcome, _) = save(
                &host,
                &script,
                &instance,
                &series,
                title("Retro"),
                Scope::All,
            )
            .await;
            assert_eq!(outcome, WriteOutcome::Busy, "{held}");
            let planned = plan_held_delete(
                &writer(&script, &host, &target),
                &target.row_id,
                &seen,
                Scope::All,
                None,
            )
            .await;
            assert_eq!(planned.err(), Some(WriteOutcome::Busy), "{held}");
            script.asked_all();
        }
        let _other = InFlight::claim_all(&host.edit, &[series_key(CALENDAR, "xyz")]).unwrap();
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(retitled(&series, "\"m2\""))));
        steps.push(refetch_read(Scripted::Json(page(retitled_october(), None))));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &series,
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert_ne!(
            series_key(CALENDAR, "abc"),
            series_key("other@example.com", "abc")
        );
    }

    /// §5, DS12: at the send the plan is made again; one that differs from the row chosen (one more
    /// occurrence renamed on its own, which "all events" would now reset) is asked again, with
    /// nothing but reads sent.
    #[tokio::test]
    async fn a_choice_that_changed_asks_again() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let (session, open) = open_in(&host, &instance, &series);
        let request = ChoiceRequest::Edit {
            session: session.clone(),
            draft: title("Retro"),
        };
        let script = Script::new(october_reads());
        let (_, _, all) = ask(&script, &instance, request, Some(open.opened()), &["abc"]).await;
        let demo = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Demo"));
        let script = Script::new(reads(
            &instance,
            &series,
            vec![series.clone(), demo.clone()],
            vec![instance.clone(), demo],
        ));
        let target = target(&instance);
        let outcome = save_series(
            &writer(&script, &host, &target),
            &session,
            open,
            title("Retro"),
            Scope::All,
            Some(token(&all)),
        )
        .await;
        script.finished();
        assert_eq!(outcome, WriteOutcome::NeedsChoice);
    }

    /// Review scope-M2: a release build sends only the plan the user chose, by its token; a save
    /// or a held delete without one is asked again, with nothing but reads sent.
    #[tokio::test]
    async fn a_save_names_its_choice_in_release_builds() {
        assert!(!choice_matches(None, "t", false));
        assert!(choice_matches(None, "t", true));
        assert!(!choice_matches(Some("u"), "t", true));
        assert!(choice_matches(Some("t"), "t", false));
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let target = target(&instance);
        let script = Script::new(october_reads());
        let ctx = SeriesWrite {
            missing_ok: false,
            ..writer(&script, &host, &target)
        };
        let (session, open) = open_in(&host, &instance, &series);
        assert_eq!(
            save_series(&ctx, &session, open, title("Retro"), Scope::All, None).await,
            WriteOutcome::NeedsChoice
        );
        script.finished();
        let script = Script::new(october_reads());
        let ctx = SeriesWrite {
            missing_ok: false,
            ..writer(&script, &host, &target)
        };
        let seen = seen_by_user(&fresh_event(instance.clone()));
        let planned = plan_held_delete(&ctx, &target.row_id, &seen, Scope::All, None).await;
        assert_eq!(planned.err(), Some(WriteOutcome::NeedsChoice));
        script.finished();
    }

    /// Review L13: the series claimed (the one the editor opened) must be the one Google now holds
    /// the occurrence under, or two writes could hold different claims for one series.
    #[tokio::test]
    async fn a_series_that_isnt_the_one_claimed_is_refused() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let elsewhere = with(instance.clone(), "recurringEventId", json!("xyz"));
        let other = with(
            with(weekly(), "id", json!("xyz")),
            "iCalUID",
            json!("xyz@google.com"),
        );
        let reads_now = || {
            Script::new(vec![
                instance_read(&elsewhere),
                Step::get("/events/xyz", "", other.clone()),
                Step::get(
                    "/events",
                    "iCalUID=xyz%40google.com",
                    page(vec![other.clone()], None),
                ),
                Step::get(
                    "/events/xyz/instances",
                    "timeMin=2026-09-01",
                    page(vec![elsewhere.clone()], None),
                ),
            ])
        };
        let no_base = WriteOutcome::Failed {
            message: SeriesRefusal::NoBase.to_string(),
        };
        // An edit claimed the series the editor opened. "This event" needs no opened series, so
        // only the claim's own check stands between it and a PATCH.
        for scope in [Scope::This, Scope::All] {
            let script = reads_now();
            let (outcome, _) =
                save(&host, &script, &instance, &series, title("Retro"), scope).await;
            script.finished();
            assert_eq!(outcome, no_base, "{scope:?}");
        }
        // A delete claimed the series its mirror row names.
        let script = reads_now();
        let target = target(&instance);
        let seen = seen_by_user(&fresh_event(elsewhere.clone()));
        let planned = plan_held_delete(
            &writer(&script, &host, &target),
            &target.row_id,
            &seen,
            Scope::All,
            None,
        )
        .await;
        script.finished();
        assert_eq!(planned.err(), Some(no_base));
    }

    /// The decision for C9a-3a: "this and following" past the first occurrence makes a new series,
    /// which C9a-3b sends; refused before anything is written.
    #[tokio::test]
    async fn a_following_edit_after_the_first_is_refused_before_anything_is_sent() {
        let host = TestHost::holding(&october());
        let script = Script::new(october_reads());
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.finished();
        assert_eq!(
            outcome,
            WriteOutcome::Failed {
                message: SPLIT_NOT_YET.into()
            }
        );
    }

    /// The shared planning: a row's token is the send's own digest, for "this event", "all events",
    /// and "this and following" from the first occurrence, which saves as all events. Accepted
    /// with the choice required, as in release.
    #[tokio::test]
    async fn choices_and_the_save_plan_alike() {
        let first = plain("2026-10-05T09:00:00+01:00");
        let mut events = vec![first.clone()];
        events.extend(october());
        let host = TestHost::holding(&events);
        let series = weekly();
        let target = target(&first);
        let reads_now = || reads(&first, &series, vec![series.clone()], events.clone());
        for scope in [Scope::This, Scope::Following, Scope::All] {
            let (session, open) = open_in(&host, &first, &series);
            let request = ChoiceRequest::Edit {
                session: session.clone(),
                draft: title("Retro"),
            };
            let script = Script::new(reads_now());
            let (this, following, all) =
                ask(&script, &first, request, Some(open.opened()), &["abc"]).await;
            assert!(matches!(following, ScopeRow::AsAll { .. }));
            let row = match scope {
                Scope::This => this,
                Scope::Following => following,
                Scope::All => all,
            };
            let mut steps = reads_now();
            if scope == Scope::This {
                steps.push(Step::write(
                    Method::Patch,
                    "/events/abc_20261005T080000Z",
                    first["etag"].as_str().unwrap(),
                    Scripted::Json(retitled(&first, "\"f2\"")),
                ));
            } else {
                let renamed: Vec<Value> = events.iter().map(|e| retitled(e, "\"r\"")).collect();
                steps.push(series_patch(Scripted::Json(retitled(&series, "\"m2\""))));
                steps.push(refetch_read(Scripted::Json(page(renamed, None))));
            }
            let script = Script::new(steps);
            let ctx = SeriesWrite {
                missing_ok: false,
                ..writer(&script, &host, &target)
            };
            let outcome = save_series(
                &ctx,
                &session,
                open,
                title("Retro"),
                scope,
                Some(token(&row)),
            )
            .await;
            script.asked_all();
            assert_eq!(outcome, saved(), "{scope:?}");
        }
    }

    /// §2.3: a scoped call on an event that doesn't repeat is refused: an edit before anything is
    /// read (the editor opened a one-off), a delete once Google's copy shows one.
    #[tokio::test]
    async fn a_scoped_call_on_a_one_off_is_refused() {
        let one_off = json!({
            "id": "solo", "etag": "\"s1\"", "summary": "Dentist",
            "organizer": { "self": true },
            "start": { "dateTime": "2026-10-12T09:00:00+01:00" },
            "end": { "dateTime": "2026-10-12T10:00:00+01:00" }
        });
        let host = TestHost::holding(std::slice::from_ref(&one_off));
        let target = target(&one_off);
        let script = Script::new(Vec::new());
        let session = keep_session(&host.edit, target.clone(), one_off.clone(), None).unwrap();
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        let ctx = writer(&script, &host, &target);
        assert_eq!(
            save_series(&ctx, &session, open, title("Retro"), Scope::All, None).await,
            WriteOutcome::Failed {
                message: NOT_REPEATING.into()
            }
        );
        script.asked_all();
        let script = Script::new(vec![Step::get("/events/solo", "", one_off.clone())]);
        let ctx = writer(&script, &host, &target);
        let seen = seen_by_user(&fresh_event(one_off));
        let planned = plan_held_delete(&ctx, &target.row_id, &seen, Scope::All, None).await;
        assert_eq!(
            planned.err(),
            Some(WriteOutcome::Failed {
                message: NOT_REPEATING_DELETE.into()
            })
        );
        script.finished();
    }

    /// §5: a series delete is planned before its Undo window (reads only), hides every row it
    /// removes, and holds the series and the row until its claim is let go.
    #[tokio::test]
    async fn a_scoped_delete_is_planned_before_its_hold_and_hides_every_row() {
        let host = TestHost::holding(&october());
        let instance = monday_19();
        let target = target(&instance);
        let script = Script::new(october_reads());
        let ctx = writer(&script, &host, &target);
        let seen = seen_by_user(&fresh_event(instance.clone()));
        let (claim, held) = plan_held_delete(&ctx, &target.row_id, &seen, Scope::All, None)
            .await
            .unwrap();
        script.finished();
        assert_eq!(held.hidden, ids_of(rows_of(&october())));
        assert_eq!(held.master.as_deref(), Some("abc"));
        let series = series_key(CALENDAR, "abc");
        assert!(InFlight::claim_all(&host.edit, std::slice::from_ref(&series)).is_none());
        assert!(InFlight::claim(&host.edit, &target.row_id).is_none());
        drop(claim);
        assert!(InFlight::claim_all(&host.edit, &[series, target.row_id.clone()]).is_some());
    }

    /// A reloaded webview gets each held series delete's hidden rows back; Undo sends nothing.
    #[test]
    fn a_held_series_delete_lists_its_hidden_rows_and_undo_sends_nothing() {
        let held = HeldDeletes::default();
        let hidden = vec!["cal:a".to_string(), "cal:b".to_string()];
        let _rx = held
            .hold(
                "t",
                "cal:a",
                "Standup",
                hidden.clone(),
                Instant::now() + UNDO,
            )
            .unwrap();
        assert_eq!(held.list()[0].hidden, hidden);
        assert!(held.cancel("t"));
        assert!(!held.take("t"), "undone, so never sent");
    }

    /// §5: at the end of its Undo window a series delete is planned again; a plan that differs from
    /// the one held (an occurrence after it now stored, so one more cancel) is asked again.
    #[tokio::test]
    async fn a_held_delete_asks_again_when_its_plan_changed() {
        let (events, changed) = with_off_site();
        let host = TestHost::holding(&events);
        let instance = monday_19();
        let window = vec![instance.clone(), plain("2026-10-26T09:00:00Z")];
        let mut steps = reads(&instance, &weekly(), vec![weekly()], window);
        steps.extend(changed);
        let script = Script::new(steps);
        assert_eq!(
            held_delete(&host, &script, &instance, Scope::Following).await,
            WriteOutcome::NeedsChoice
        );
        script.finished();
    }

    /// Review findings 4 and 5: a series delete whose occurrence was deleted in Google during the
    /// hold deletes nothing else, and says so; a series gone when the save reaches it takes every
    /// row of it off.
    #[tokio::test]
    async fn a_series_write_meets_what_google_deleted() {
        let host = TestHost::holding(&october());
        let instance = monday_19();
        let mut steps = october_reads();
        steps.push(instance_read(&with(
            instance.clone(),
            "status",
            json!("cancelled"),
        )));
        let script = Script::new(steps);
        assert_eq!(
            held_delete(&host, &script, &instance, Scope::All).await,
            WriteOutcome::Failed {
                message: ONE_GONE.into()
            }
        );
        script.finished();
        assert_eq!(
            host.ids(),
            ["cal:abc_20261012T080000Z", "cal:abc_20261026T090000Z"]
        );

        let host = TestHost::holding(&october());
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Status(404)));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, WriteOutcome::Gone);
        assert!(host.ids().is_empty());
        assert_eq!(host.told(), ["cal"]);
    }

    // --- the sender's folds (review H1, L6) ---

    /// Review H1: a sign-in refused after a copy met a server error leaves that copy in doubt.
    #[test]
    fn a_sign_in_refused_after_a_server_error_is_lost_not_refused() {
        let refused = || Error::Reauth("sign in again".into());
        assert!(matches!(
            google_io::no_answer(refused(), true),
            google_io::NoAnswer::Lost { retried: false, .. }
        ));
        assert!(matches!(
            google_io::no_answer(refused(), false),
            google_io::NoAnswer::Unsent {
                why: Unsent::SignIn,
                ..
            }
        ));
    }

    /// What can't have reached Google is unsent, and says why: the sign-in, a connection never
    /// made, or anything on this computer (the keychain, a request that couldn't be built).
    #[tokio::test]
    async fn an_error_before_anything_reached_google_is_unsent() {
        let unbuilt = reqwest::Client::new().get("not a url").build().unwrap_err();
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            reqwest::Client::new()
                .get(format!("http://127.0.0.1:{port}/"))
                .send()
                .await
                .unwrap_err()
        };
        for (error, why) in [
            (Error::Reauth("sign in again".into()), Unsent::SignIn),
            (Error::Other("keychain".into()), Unsent::Local),
            (Error::from(unbuilt), Unsent::Local),
            (Error::from(closed), Unsent::Offline),
        ] {
            assert_eq!(google_io::unsent(&error), Some(why), "{error}");
        }
    }

    /// A request that reached a server and got no answer may have landed: lost, saying whether a
    /// second copy went after a server error.
    #[tokio::test]
    async fn an_error_after_a_server_error_says_whether_it_was_resent() {
        // Connections are queued but never answered.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", silent.local_addr().unwrap());
        for earlier in [false, true] {
            let timed_out = reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap()
                .get(&url)
                .send()
                .await
                .unwrap_err();
            assert!(matches!(
                google_io::no_answer(Error::from(timed_out), earlier),
                google_io::NoAnswer::Lost { retried, .. } if retried == earlier
            ));
        }
    }

    /// Only a 2xx body is the event a machine reads; any other is Google's error.
    #[test]
    fn only_a_2xx_body_reaches_the_machine() {
        let reply = |status: u16, body: &str| {
            answer_of(Ok(google_io::Reply {
                verdict: classify::classify(status, body),
                body: body.into(),
                maybe_applied: false,
                date: None,
            }))
        };
        let refused = r#"{"error":{"code":412,"errors":[{"reason":"conditionNotMet"}]}}"#;
        assert!(matches!(
            reply(412, refused),
            Answer::Reply {
                verdict: Verdict::Conflict,
                body: None,
                ..
            }
        ));
        assert!(matches!(
            reply(200, r#"{"id":"a"}"#),
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(_),
                ..
            }
        ));
        assert!(matches!(
            reply(204, ""),
            Answer::Reply {
                verdict: Verdict::Ok,
                body: None,
                ..
            }
        ));
        assert_eq!(
            answer_of(Err(google_io::NoAnswer::Lost {
                error: Error::Other("timeout".into()),
                retried: true
            })),
            Answer::Lost { retried: true }
        );
        assert_eq!(
            answer_of(Err(google_io::NoAnswer::Unsent {
                error: Error::Other("keychain".into()),
                why: Unsent::Local
            })),
            Answer::Unsent(Unsent::Local)
        );
    }

    // --- the dev allow-list (scope-M5, L9) ---

    #[cfg(debug_assertions)]
    fn named(name: &str, provider: &str, remote: &str) -> calendar::Calendar {
        calendar::Calendar {
            id: format!("{provider}:{remote}"),
            source_id: "source".into(),
            provider: provider.into(),
            remote_id: Some(remote.into()),
            name: name.into(),
            color: None,
            selected: true,
            is_primary: false,
            quiet: false,
            kind: None,
            facts: CalendarFacts::default(),
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn the_dev_allow_list_refuses_other_calendars() {
        let calendars = [
            named("Work", "google", "work@group"),
            named("Home", "google", "home@group"),
            // A feed named alike isn't a Google calendar PM could write to.
            named("Work", "ics", "feed"),
        ];
        let allowed = Some(dev_allow::resolve("Work", &calendars));
        assert_eq!(
            allowed,
            Some(Some(HashSet::from(["work@group".to_string()])))
        );
        assert_eq!(
            dev_allow::decide(Some("Work"), allowed.as_ref(), "work@group", true),
            Ok(())
        );
        assert_eq!(
            dev_allow::decide(Some("Work"), allowed.as_ref(), "home@group", false),
            Err(dev_allow::REFUSED)
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn the_dev_allow_list_fails_closed_for_series_wide_writes_when_unset() {
        assert_eq!(dev_allow::decide(None, None, "home@group", false), Ok(()));
        assert_eq!(
            dev_allow::decide(None, None, "home@group", true),
            Err(dev_allow::UNSET)
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn an_allow_list_name_two_calendars_share_refuses_every_write() {
        let calendars = [
            named("Work", "google", "work@group"),
            named("Work", "google", "other@group"),
            named("Home", "google", "home@group"),
        ];
        assert_eq!(dev_allow::resolve("Work", &calendars), None);
        assert_eq!(dev_allow::resolve("Home, Nowhere", &calendars), None);
        for allowed in [None, Some(&None)] {
            assert_eq!(
                dev_allow::decide(Some("Work"), allowed, "work@group", false),
                Err(dev_allow::UNRESOLVED)
            );
        }
    }

    /// The sender's backstop: with the variable set, a write goes only under a calendar it names,
    /// never one whose id merely starts the same; unset, the shells decide.
    #[cfg(debug_assertions)]
    #[test]
    fn the_senders_backstop_permits_only_the_listed_calendars() {
        let to = |calendar: &str| {
            plan::patch_event(
                calendar,
                "e1",
                "\"1\"",
                json!({ "summary": "x" }),
                Notify::None,
                plan::VersionFlags::default(),
            )
            .url
        };
        let ids = Some(Some(HashSet::from(["work@group".to_string()])));
        assert!(dev_allow::permits(
            Some("Work"),
            ids.as_ref(),
            &to("work@group")
        ));
        assert!(!dev_allow::permits(
            Some("Work"),
            ids.as_ref(),
            &to("home@group")
        ));
        assert!(!dev_allow::permits(
            Some("Work"),
            ids.as_ref(),
            &to("work@group.evil")
        ));
        assert!(!dev_allow::permits(
            Some("Work"),
            Some(&None),
            &to("work@group")
        ));
        assert!(dev_allow::permits(None, None, &to("home@group")));
    }
}
