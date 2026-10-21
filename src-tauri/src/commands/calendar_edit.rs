// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Changing Google events from PM (#884): open an event for editing, save the fields the user
//! changed, delete it. The decisions are the pure core's (`crate::calendar_write`); this module does
//! the I/O around them, in a fixed order every write follows:
//!
//! 1. the main window and the vault owner only (`require_main_window`, `require_owner`). The one
//!    read that isn't for a write, how a repeating event repeats ([`series_summary`], for the event
//!    popover), checks the window alone (C9 design scope-L3);
//! 2. a short DB read: which Google calendar and event the mirror row stands for, and the calendar's
//!    facts. The webview only ever names PM's own row ids and the session ids PM minted;
//! 3. a fresh GET from Google, with no lock held, and the gate again on that copy (the mirror can be
//!    stale, and clips text on purpose);
//! 4. the save rules on the fresh copy: nothing Google changed meanwhile is overwritten (R2);
//! 5. one PATCH or DELETE guarded by the etag, through the private [`google_io`];
//! 6. Google's reply into the mirror (never the draft), remembered so a sync that started earlier
//!    can't put the old version back, and the briefing told to look again.
//!
//! Creating an event (C10) has nothing to read first: it goes out under an id PM chose, and an
//! answer that never arrives is looked for under that id before anything is sent again (A9), so a
//! dropped connection very rarely makes a second copy (Google can't always catch a repeated id,
//! which is why the posts under one id are capped, [`MAX_POSTS`]).
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
use crate::calendar_recur::expand;
use crate::calendar_recur::lines::{self, Applied};
use crate::calendar_recur::spec::{day_phrase, describe_lines, spec_of};
use crate::calendar_recur::wire::RepeatRule;
use crate::calendar_recur::{Point, SeriesStart};
use crate::calendar_write::classify::{Method, Verdict};
use crate::calendar_write::dto::{
    ChoiceRequest, DeleteSettled, DeleteStart, EditLoad, EventForEdit, EventPatchDraft,
    FieldPermissions, HeldDeleteInfo, MilestoneMoves, MilestoneOffer, NewEventDraft, Notify,
    ReadOnlyReason, ScopeRow, SeenSummary, SeriesChoices, SeriesInfo, SeriesScope,
    SeriesWarningDto, ShowAs, SplitAction, TimeDraft, Visibility, WriteOutcome, WriteWarning,
};
use crate::calendar_write::gate::{self, EditFacts};
use crate::calendar_write::patch::{self, DeleteCheck, UpdateCheck};
use crate::calendar_write::plan::{is_plain_segment, RequestPlan};
use crate::calendar_write::reconcile::{RecentWrites, Stamp};
use crate::calendar_write::series::diff::{self, Expect, GoneRow, Refetch, Removes, SeriesDiff};
use crate::calendar_write::series::journal::{self, Entry, Look, Orphan};
use crate::calendar_write::series::plan::{
    marks_from, plan_series_delete, plan_series_edit, Fresh, NotPlanned, Op, Opened, SeriesInput,
    SeriesPlan, SeriesWarning, Shows, Undo,
};
use crate::calendar_write::series::run::{
    Answer, End, Machine, Next, Ran, Rollback, Run, Settle, SettleAction, Settled, Unfit, Unsent,
    Why,
};
use crate::calendar_write::series::scope::{
    is_first, scope_options, Change, Row, Scope, ScopeReason,
};
use crate::calendar_write::series::{
    later_part, original_of, parts_overlap, text_only, Exceptions, FreshEvent, LaterPart, Listing,
    Master, Parts, SeriesRefusal,
};
use crate::calendar_write::{plan, time};
use crate::error::{Error, Result};
use crate::{briefing, google, milestones, secrets, AppState};

use super::shared::require_main_window;
use super::vaults::require_vault_owner;

/// What editing keeps in memory, in [`AppState`]. Lock order everywhere: the DB, then `recent`;
/// `partials`, `splitting`, `creates`, `in_flight` and `summaries` are each taken alone, never with
/// the DB or another of these held.
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
    /// "This and following" changes PM couldn't finish, by the token the webview names them with.
    /// No time limit: one lives until it's settled, or PM quits (C9 design §2.5).
    partials: Mutex<HashMap<String, HeldSplit>>,
    /// The new parts of the "this and following" changes running now, a send or a settle, by
    /// [`series_key`]: their journal records are theirs, no orphans.
    splitting: Mutex<HashSet<String>>,
    /// New events whose answer never arrived, by the retry token PM gave the webview (R5): what to
    /// look for and, if it's still not there, send again under the same id. Lapses after
    /// `SESSION_TTL`; lost on quit, which loses nothing (a create that landed shows at the next
    /// sync, and one that didn't is simply absent: a create has no half to leave behind).
    creates: Mutex<HashMap<String, HeldCreate>>,
    /// One calendar sync at a time, so a fetch that started earlier can never finish after a later
    /// one has settled a save (plan A23). A second caller waits, then runs; the change check
    /// ([`super::check_google_calendars`]) never waits, it skips.
    pub(crate) sync_lock: tokio::sync::Mutex<()>,
    /// What the change check knows about each Google calendar (F3). Lock order: the DB, then this;
    /// never held across an await.
    pub(crate) fresh: Mutex<crate::calendar_fresh::Freshness>,
    /// Each repeating event's words for the popover, by [`series_key`] (C9b-2). Forgotten by any
    /// write to the series ([`InFlight`]'s and [`HeldClaim`]'s drop).
    summaries: Mutex<Summaries>,
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

/// Where a PM calendar lives at Google, from the DB alone: an iCal feed or an Outlook calendar comes
/// back as `NotGoogle`, never a target.
#[derive(Clone, Debug, PartialEq)]
struct CalendarTarget {
    calendar_id: String,
    remote_calendar: String,
    email: String,
    token_key: String,
    facts: CalendarFacts,
    /// Synced into the mirror (`calendars.selected`): a write to one that isn't lands nothing
    /// (`calendar::apply_write_effect`).
    selected: bool,
}

/// Resolve PM's calendar `calendar_id` to its Google calendar. `missing`: the words for a calendar
/// that isn't in PM any more, each caller its own (an event's calendar, or the one chosen).
fn resolve_calendar(
    conn: &Connection,
    calendar_id: &str,
    missing: &str,
) -> Result<std::result::Result<CalendarTarget, ReadOnlyReason>> {
    let Some(cal) = calendar::calendar_by_id(conn, calendar_id)? else {
        return Err(Error::Other(missing.into()));
    };
    let email = match (cal.provider.as_str(), cal.source_id.strip_prefix("gcal:")) {
        ("google", Some(email)) => email.to_string(),
        _ => return Ok(Err(ReadOnlyReason::NotGoogle)),
    };
    let remote_calendar = cal.remote_id.clone().unwrap_or_else(|| cal.id.clone());
    if !is_plain_segment(&remote_calendar) {
        return Ok(Err(ReadOnlyReason::NotGoogle));
    }
    let Some(token_key) = secrets::token_key_for("google", "calendar", &email) else {
        return Ok(Err(ReadOnlyReason::NotGoogle));
    };
    Ok(Ok(CalendarTarget {
        calendar_id: cal.id,
        remote_calendar,
        email,
        token_key,
        facts: cal.facts,
        selected: cal.selected,
    }))
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
    let cal = match resolve_calendar(
        conn,
        &row.calendar_id,
        "That event's calendar is no longer connected.",
    )? {
        Ok(cal) => cal,
        Err(reason) => return Ok(Err(reason)),
    };
    let Some(remote_event) = row
        .id
        .strip_prefix(&format!("{}:", row.calendar_id))
        .filter(|e| is_plain_segment(e))
    else {
        return Ok(Err(ReadOnlyReason::NotGoogle));
    };
    Ok(Ok(Target {
        row_id: row.id.clone(),
        calendar_id: row.calendar_id.clone(),
        remote_calendar: cal.remote_calendar,
        remote_event: remote_event.to_string(),
        email: cal.email,
        token_key: cal.token_key,
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
        series_master: turned_series(fresh),
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

/// Google's copy of a single event is now a repeating event's series: it carries a rule and is no
/// occurrence ([`calendar::is_master`], where an empty rule is none). The sync never mirrors such a
/// copy (`calendar::parse_event`), so only a row that was a one-off can be under one.
fn turned_series(fresh: &Value) -> bool {
    calendar::is_master(fresh)
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
        // The mirror never holds a series itself (`calendar::parse_event`). An occurrence is
        // editable like a one-off here: which occurrences is asked at the write (rule R9).
        series_master: false,
        has_guests: row.attendees.iter().any(|a| !a.is_self),
        html_description: false,
    }
}

/// Whether a mirror row is an occurrence of a repeating event: its series id, or the flag a row
/// read before series ids existed carries (the webview's `isRepeating` asks the same).
fn row_repeats(row: &CalendarEvent) -> bool {
    row.recurring || row.series_id.is_some()
}

/// Whether the mirror row `row_id` is an occurrence of a repeating event, so a delete of it must
/// name which occurrences (rule R9; C9 design review scope-L8: refused before it's held, not 8 s
/// later). A row that's gone answers false: the delete's own read says so.
fn needs_scope(conn: &Connection, row_id: &str) -> Result<bool> {
    Ok(calendar::event_by_id(conn, row_id)?.is_some_and(|row| row_repeats(&row)))
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
        || (draft.visibility.is_some() && !perms.visibility)
        // How it repeats moves its times, so it takes the time's permission (C11).
        || (draft.recurrence.is_some() && !perms.time);
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
    let series = master
        .as_ref()
        .and_then(|m| series_info(&fresh, m))
        .map(Box::new);
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
    series_copy(get(wire, &target.remote_calendar, id).await, id)
}

/// Where the occurrence `fresh` sits in its series `master`, and how that repeats: in words, as a rule
/// from the editor's set when it is one (the Repeat row's seed, C11), from which day and in which zone.
/// `None` for a series PM can't read.
fn series_info(fresh: &Value, master: &Value) -> Option<SeriesInfo> {
    let read = Master::read(&FreshEvent::from_google(master.clone())).ok()?;
    Some(SeriesInfo {
        first: is_first(&read, &FreshEvent::from_google(fresh.clone())),
        repeats: describe_lines(&read.recurrence, &read.start),
        rule: spec_of(&read.recurrence, &read.start).map(|spec| RepeatRule::from_spec(&spec)),
        starts: read.start.date().format("%Y-%m-%d").to_string(),
        zone: match read.start {
            SeriesStart::Timed { zone, .. } => Some(zone.name().to_string()),
            SeriesStart::AllDay(_) => None,
        },
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
/// scope it's one event: a one-off (an occurrence of a repeating event answers `NeedsChoice`, rule
/// R9).
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
            missing_ok: MISSING_CHOICE_OK,
            new_series_id: plan::client_event_id,
        };
        return Ok(save_series(&ctx, &session, open, draft, scope.into(), choice.as_deref()).await);
    }
    let token_key = open.target.token_key.clone();
    Ok(save_event(
        &GoogleWire(&token_key),
        &app,
        &session,
        &open,
        &draft,
        editing,
    )
    .await)
}

/// A one-off event can't be made to repeat from the editor yet (C11d).
const ONE_OFF_REPEAT: &str =
    "PM can't change whether this event repeats yet, so nothing was saved.";

/// [`update_calendar_event`] for one event, past its shell: Google through `wire`, the mirror and
/// the claims through `host`.
async fn save_event<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    session: &str,
    open: &EditSession,
    draft: &EventPatchDraft,
    editing: EditingStatus,
) -> WriteOutcome {
    let Some(_flight) = InFlight::claim(host.edit(), &open.target.row_id) else {
        return WriteOutcome::Busy;
    };
    let target = &open.target;

    // At most two rounds: a 412 means the event changed between our GET and our PATCH, and gets one
    // fresh look (R2); a second 412 is a conflict.
    for round in 1..=2 {
        let fresh = match get(wire, &target.remote_calendar, &target.remote_event).await {
            Fetched::Event(fresh) => fresh,
            Fetched::Gone => return land_gone(host, target, None),
            Fetched::Reauth => return WriteOutcome::Reauth,
            Fetched::Busy => return WriteOutcome::Busy,
            Fetched::Failed(message) => return WriteOutcome::Failed { message },
        };
        if patch::is_gone(&fresh) {
            return land_gone(host, target, Some(&fresh));
        }
        let facts = facts_from_fresh(&fresh, &target.facts, editing);
        let perms = gate::edit_rights(&facts);
        if let Some(reason) = refused_field(&perms, draft) {
            return WriteOutcome::ReadOnly { reason };
        }
        // An occurrence of a repeating event: which occurrences is the user's choice, never a
        // default (rule R9). Only a GET has gone out.
        if fresh.get("recurringEventId").is_some() {
            return WriteOutcome::NeedsChoice;
        }
        // A one-off given a repeat is C11d's: until then it's refused, never saved without it
        // (C11 critique 2 L6). Only a GET has gone out.
        if draft.recurrence.is_some() {
            return WriteOutcome::Failed {
                message: ONE_OFF_REPEAT.into(),
            };
        }
        let body = match patch::build_patch(&fresh, draft) {
            Ok(body) => body,
            Err(e) => {
                return WriteOutcome::Failed {
                    message: e.to_string(),
                }
            }
        };
        if body.is_empty() {
            return WriteOutcome::NoChange;
        }
        // Fail closed: a single-event save sends nothing beyond the fields it may change.
        if body.keys().any(|k| !patch::PATCHABLE.contains(&k.as_str())) {
            return WriteOutcome::Failed {
                message: "PM built a change it isn't allowed to send, so nothing was saved.".into(),
            };
        }
        match patch::update_check(&open.base, &fresh, &body) {
            UpdateCheck::Proceed => {}
            UpdateCheck::Gone => return land_gone(host, target, Some(&fresh)),
            UpdateCheck::Conflict(fields) => return WriteOutcome::Conflict { fields },
        }
        let Some(etag) = fresh.get("etag").and_then(Value::as_str) else {
            return WriteOutcome::Failed {
                message: "Google sent this event without a version stamp, so PM won't change it."
                    .into(),
            };
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
        let reply = match wire.send(&request).await {
            Ok(reply) => reply,
            Err(google_io::NoAnswer::Unsent { why, .. }) => return unsent_outcome(why),
            // No answer, or a sign-in refused after a server error: it may or may not have landed.
            // Look before saying either (review H1).
            Err(google_io::NoAnswer::Lost { .. }) => {
                return after_lost_answer(wire, host, target, session, &body).await
            }
        };
        // A server error on the way may have been applied, whatever the last answer says (a retry
        // of a copy that landed is answered 412): look before reporting anything but success.
        if reply.maybe_applied && reply.verdict != Verdict::Ok {
            match get(wire, &target.remote_calendar, &target.remote_event).await {
                Fetched::Event(now) if patch::already_applied(&now, &body) => {
                    return land_saved(host, target, session, now, &body);
                }
                Fetched::Event(_) => {}
                Fetched::Gone => return land_gone(host, target, None),
                _ => return WriteOutcome::Unconfirmed,
            }
        }
        match reply.verdict {
            Verdict::Ok => {
                return match serde_json::from_str::<Value>(&reply.body) {
                    Ok(saved) => land_saved(host, target, session, saved, &body),
                    // Saved, but the reply wasn't JSON: the next sync will show it.
                    Err(_) => saved_pending(host, target),
                };
            }
            Verdict::Conflict if round == 1 => continue,
            Verdict::Gone => return land_gone(host, target, None),
            verdict => return outcome_for(verdict, editing),
        }
    }
    WriteOutcome::Conflict { fields: Vec::new() }
}

// --- creating an event (C10) ---

/// A calendar PM doesn't sync: a new event there would land nothing in the mirror.
const NOT_SYNCED: &str = "PM doesn't sync this calendar, so it won't create events in it. Choose \
                          it in Settings › Connectors first.";
/// A retry into a calendar PM stopped syncing after the first send, which may have landed: the
/// token stays, so choosing the calendar again lets Check again finish it.
const CREATE_NOT_SYNCED: &str = "PM doesn't sync this calendar any more, so it won't send this \
                                 new event again. PM may already have made it: choose the \
                                 calendar in Settings › Connectors and Check again, or look for \
                                 it in Google Calendar before creating it again, so it isn't made \
                                 twice.";
/// The retry token is unknown: PM restarted, or the eight hours passed. A constant, the title left
/// to the webview's notice, so the webview can tell it apart (it ends the token, C10 critiques).
const CREATE_LOST: &str = "PM lost touch with Google and can't tell whether this new event was \
                           made. Look for it in Google Calendar before creating it again, so it \
                           isn't made twice.";
/// A retry naming another calendar, or a draft other than the one sent (C10 critique 2 M2). The
/// webview drops its token on this, so the words can't offer a Check again; the earlier event may
/// be there all the same.
const CREATE_CHANGED: &str = "This differs from the new event PM was creating when it lost touch \
                              with Google, so PM didn't send it. PM may already have made the \
                              earlier one: look for it in Google Calendar before creating it \
                              again, so it isn't made twice.";
/// The new event went to Google [`MAX_POSTS`] times without a readable answer (C10 critique 1 M5).
const CREATE_LATER: &str = "PM has already sent this new event to Google more than once without \
                            hearing back, so it won't send it again. Check again later: PM looks \
                            for it first. If it still isn't there, look for it in Google Calendar \
                            before creating it again, so it isn't made twice.";

/// The most POSTs one new event's id gets, across its first run and every retry: the first and
/// two more. A run posts twice at most (A9) and a retry once, so the bound is what stops "Check
/// again" from sending it for ever (Google warns it can't always catch a repeated id).
const MAX_POSTS: usize = 3;

/// The pause between a retry's two looks for a new event: the same second `run.rs` waits before a
/// lost insert's second look (`LOOK_AGAIN`), kept here so that one stays private.
const RETRY_PAUSE: Duration = Duration::from_secs(1);

/// Create an event on one of the user's Google calendars, under an id PM chose. `calendar_id` is
/// PM's calendar row id; `draft` is what the editor holds; `retry_token` names a create whose
/// answer never arrived (from an `Ambiguous` outcome), which is looked for before anything is sent
/// again (R2). Nobody is emailed: a new event has no guests (R7).
#[tauri::command]
pub async fn create_calendar_event(
    app: AppHandle,
    window: tauri::Window,
    calendar_id: String,
    draft: NewEventDraft,
    retry_token: Option<String>,
) -> Result<WriteOutcome> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let state = app.state::<AppState>();
    let target = match resolve_calendar(
        &*state.conn()?,
        &calendar_id,
        "That calendar is no longer connected.",
    )? {
        Ok(target) => target,
        Err(reason) => return Ok(WriteOutcome::ReadOnly { reason }),
    };
    #[cfg(debug_assertions)]
    {
        // A create reaches one new event, or a whole repeating one: that is series-wide, so an
        // unset allow-list refuses it (C11 critique 1 L11).
        let series_wide = draft.recurrence.is_some();
        let allowed = dev_allow::check(&*state.conn()?, &target.remote_calendar, series_wide)?;
        if let Err(message) = allowed {
            return Ok(WriteOutcome::Failed {
                message: message.into(),
            });
        }
    }
    let editing_choice = calendar_editing::choice(&*state.conn()?, &target.email)?;
    // A keychain read, with no lock held.
    let editing = editing_status(editing_choice, &target.token_key);
    let token_key = target.token_key.clone();
    let ctx = CreateWrite {
        wire: &GoogleWire(&token_key),
        host: &app,
        target: &target,
        editing,
        new_id: plan::client_event_id,
        new_ref: new_create_ref,
    };
    Ok(create_event(&ctx, draft, retry_token.as_deref()).await)
}

/// What one create works with: Google through `wire`, the mirror and the held creates through
/// `host`, the calendar, the account's editing status, and the two ids PM mints: the new event's
/// ([`plan::client_event_id`]) and the retry token ([`new_create_ref`]), fixed ones in the tests.
struct CreateWrite<'a, W, H> {
    wire: &'a W,
    host: &'a H,
    target: &'a CalendarTarget,
    editing: EditingStatus,
    new_id: fn() -> String,
    new_ref: fn() -> String,
}

/// A retry token: PM's own random id, a separate one from the event's, so the webview never holds
/// or learns the id PM chose for Google (R5).
fn new_create_ref() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// A held create's key in the claims: never a row id (those start with PM's calendar id) or a
/// [`series_key`].
fn create_key(token: &str) -> String {
    format!("create\u{1f}{token}")
}

/// A new event whose answer never arrived, held under its retry token.
#[derive(Clone)]
struct HeldCreate {
    /// Where it was sent: the retry must name the same calendar.
    target: CalendarTarget,
    /// The one-step plan, PM's id inside it.
    plan: SeriesPlan,
    /// What was sent, for telling a retry of the same event from a changed one.
    body: serde_json::Map<String, Value>,
    /// POSTs under its id so far, the first run's and every retry's ([`MAX_POSTS`]).
    posts: usize,
    /// When it was last sent, for [`SESSION_TTL`].
    at: Instant,
}

impl CalendarEditState {
    fn hold_create(&self, token: &str, held: HeldCreate) {
        if let Ok(mut creates) = self.creates.lock() {
            creates.insert(token.to_string(), held);
        }
    }

    /// Let held creates sent [`SESSION_TTL`] or more before `now` go.
    fn prune_creates(&self, now: Instant) {
        if let Ok(mut creates) = self.creates.lock() {
            creates.retain(|_, held| now.saturating_duration_since(held.at) < SESSION_TTL);
        }
    }
}

/// A held create taken out for a retry: held again under its token when this goes, unless it was
/// consumed (a landing, or Google showing it deleted), a panic or an early return included, so a
/// retry that stops partway never loses it (the [`TakenSplit`] precedent; C10 critique 1 M1).
struct TakenCreate<'a> {
    edit: &'a CalendarEditState,
    token: String,
    back: Option<HeldCreate>,
}

impl<'a> TakenCreate<'a> {
    /// Take the create out, so a second retry of it finds nothing: the guard, and a copy to work on.
    /// Only a token PM still holds; the map's key is PM's own, never fetched data.
    fn take(edit: &'a CalendarEditState, token: &str) -> Option<(Self, HeldCreate)> {
        let held = edit.creates.lock().ok()?.remove(token)?;
        let taken = TakenCreate {
            edit,
            token: token.to_string(),
            back: Some(held.clone()),
        };
        Some((taken, held))
    }

    /// What goes back: `held`, as the retry left it.
    fn keep(&mut self, held: HeldCreate) {
        self.back = Some(held);
    }

    /// Nothing goes back: Google showed the new event, or showed it deleted.
    fn consume(&mut self) {
        self.back = None;
    }
}

impl Drop for TakenCreate<'_> {
    fn drop(&mut self) {
        if let Some(held) = self.back.take() {
            self.edit.hold_create(&self.token, held);
        }
    }
}

/// [`create_calendar_event`] past its shell: Google through `ctx.wire`, the mirror and the held
/// creates through `ctx.host`.
async fn create_event<W: Wire, H: Host>(
    ctx: &CreateWrite<'_, W, H>,
    draft: NewEventDraft,
    retry_token: Option<&str>,
) -> WriteOutcome {
    let body = match patch::insert_body(&draft) {
        Ok(body) => body,
        Err(e) => {
            return WriteOutcome::Failed {
                message: e.to_string(),
            }
        }
    };
    ctx.host.edit().prune_creates(Instant::now());
    if let Some(token) = retry_token {
        return retry_create(ctx, token, body).await;
    }
    if !ctx.target.selected {
        return WriteOutcome::Failed {
            message: NOT_SYNCED.into(),
        };
    }
    if let Some(reason) = create_blocked(ctx, &body) {
        return WriteOutcome::ReadOnly { reason };
    }
    let id = (ctx.new_id)();
    let Some(plan) = SeriesPlan::create(&ctx.target.remote_calendar, &id, body.clone()) else {
        return WriteOutcome::Failed {
            message: UNFIT.into(),
        };
    };
    let Ok(mut run) = Run::new(plan.clone()) else {
        return WriteOutcome::Failed {
            message: UNFIT.into(),
        };
    };
    let wire = Posts::new(ctx.wire);
    match drive(&wire, &mut run).await {
        Ran::Done { ends } => landed_create(ctx.wire, ctx.host, ctx.target, &ends).await,
        // Gone: made, then deleted in Google before PM could look.
        Ran::NotDone { why, .. } => not_done_outcome(why, ctx.editing, false),
        Ran::Unknown { .. } => {
            let token = (ctx.new_ref)();
            ctx.host.edit().hold_create(
                &token,
                HeldCreate {
                    target: ctx.target.clone(),
                    plan,
                    body,
                    posts: wire.count(),
                    at: Instant::now(),
                },
            );
            WriteOutcome::Ambiguous { retry_token: token }
        }
    }
}

/// Why the account and calendar can't take the new event `body`: the gate's rules for a solo
/// one-off you organise ([`gate::create_block`]), on the calendar's role as the DB has it. A role
/// Google lowered since gets its own 403, read by [`outcome_for`].
fn create_blocked<W, H>(
    ctx: &CreateWrite<'_, W, H>,
    body: &serde_json::Map<String, Value>,
) -> Option<ReadOnlyReason> {
    gate::create_block(
        ctx.editing,
        ctx.target.facts.access_role.as_deref(),
        body.get("visibility").and_then(Value::as_str),
    )
}

/// A look a retry makes for its new event, read off the GET. No `Debug`: it holds Google's copy,
/// which never goes into a message.
enum RetryLook {
    Live(Value),
    /// There, cancelled: made, then deleted in Google.
    Cancelled,
    /// 404 or 410: no copy yet, which proves nothing on its own (DS5).
    Absent,
    Reauth,
    Busy,
    /// Offline, a server error, an answer PM can't read, a refused read.
    Unknown,
}

impl RetryLook {
    fn of(fetched: Fetched) -> RetryLook {
        match fetched {
            Fetched::Event(v) if patch::is_gone(&v) => RetryLook::Cancelled,
            Fetched::Event(v) => RetryLook::Live(v),
            Fetched::Gone => RetryLook::Absent,
            Fetched::Reauth => RetryLook::Reauth,
            Fetched::Busy => RetryLook::Busy,
            Fetched::Failed(_) => RetryLook::Unknown,
        }
    }
}

/// Why a retry stops with the create still held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kept {
    Reauth,
    Busy,
    Ambiguous,
}

impl Kept {
    fn outcome(self, token: &str) -> WriteOutcome {
        match self {
            Kept::Reauth => WriteOutcome::Reauth,
            Kept::Busy => WriteOutcome::Busy,
            Kept::Ambiguous => WriteOutcome::Ambiguous {
                retry_token: token.to_string(),
            },
        }
    }
}

/// What a retry does after one look.
enum RetryStep {
    /// It's there: land it.
    Land(Value),
    /// Made, then deleted in Google.
    Gone,
    /// Not there yet: wait [`RETRY_PAUSE`] and look again.
    LookAgain,
    /// Not there after two looks: send it again under the same id.
    Send,
    /// Stop, the create held as it was.
    Keep(Kept),
}

/// A retry's next step after `look`, `first` saying whether it was the first of its two looks.
/// Google's insert-then-get isn't guaranteed consistent, so one 404 proves nothing (DS5): only two,
/// a second apart, let the same request go again, and the same id normally makes that safe (a copy
/// that landed meanwhile turns it into a 409, which the run then looks up; Google can't always catch
/// a repeated id, hence [`MAX_POSTS`]).
fn retry_next(look: RetryLook, first: bool) -> RetryStep {
    match look {
        RetryLook::Live(copy) => RetryStep::Land(copy),
        RetryLook::Cancelled => RetryStep::Gone,
        RetryLook::Absent if first => RetryStep::LookAgain,
        RetryLook::Absent => RetryStep::Send,
        RetryLook::Reauth => RetryStep::Keep(Kept::Reauth),
        RetryLook::Busy => RetryStep::Keep(Kept::Busy),
        RetryLook::Unknown => RetryStep::Keep(Kept::Ambiguous),
    }
}

/// A retry of the create `token` named: look for it under its id first, and only after looking
/// twice and not finding it, send it again under the same id (A9). Never with a changed calendar or
/// body: a copy that landed holds what was first sent, and a retry can't edit it. The looks are
/// reads, so the account's editing and the calendar's role are asked only before a re-send (C10
/// critique 1 M1). Every end but Google showing the event, or showing it deleted, keeps it held
/// under the same token.
async fn retry_create<W: Wire, H: Host>(
    ctx: &CreateWrite<'_, W, H>,
    token: &str,
    body: serde_json::Map<String, Value>,
) -> WriteOutcome {
    let edit = ctx.host.edit();
    // Claim before take, so a second retry of the same token is Busy and never finds it missing.
    let Some(_claim) = InFlight::claim(edit, &create_key(token)) else {
        return WriteOutcome::Busy;
    };
    let Some((mut taken, held)) = TakenCreate::take(edit, token) else {
        return WriteOutcome::Failed {
            message: CREATE_LOST.into(),
        };
    };
    if held.target.calendar_id != ctx.target.calendar_id || held.body != body {
        return WriteOutcome::Failed {
            message: CREATE_CHANGED.into(),
        };
    }
    let Some(Op::Insert { calendar, id, .. }) = held.plan.steps.first().map(|s| &s.op) else {
        return WriteOutcome::Failed {
            message: UNFIT.into(),
        };
    };
    let mut first = true;
    loop {
        match retry_next(RetryLook::of(get(ctx.wire, calendar, id).await), first) {
            RetryStep::Land(copy) => {
                taken.consume();
                return land_copy(ctx.wire, ctx.host, &held.target, &copy).await;
            }
            RetryStep::Gone => {
                taken.consume();
                return WriteOutcome::Gone;
            }
            RetryStep::LookAgain => {
                ctx.wire.wait(RETRY_PAUSE).await;
                first = false;
            }
            RetryStep::Send => break,
            RetryStep::Keep(kept) => return kept.outcome(token),
        }
    }
    if held.posts >= MAX_POSTS {
        return WriteOutcome::Failed {
            message: CREATE_LATER.into(),
        };
    }
    // A re-send meets the refusals a first send does: a calendar PM stopped syncing since would
    // land nothing. The create stays held, so a look can still find it; the words say the first
    // send may have landed.
    if !ctx.target.selected {
        return WriteOutcome::Failed {
            message: CREATE_NOT_SYNCED.into(),
        };
    }
    if let Some(reason) = create_blocked(ctx, &held.body) {
        return WriteOutcome::ReadOnly { reason };
    }
    let Ok(mut run) = Run::retry_insert(held.plan.clone()) else {
        return WriteOutcome::Failed {
            message: UNFIT.into(),
        };
    };
    let wire = Posts::new(ctx.wire);
    let ran = drive(&wire, &mut run).await;
    match ran {
        Ran::Done { ends } => {
            taken.consume();
            landed_create(ctx.wire, ctx.host, &held.target, &ends).await
        }
        Ran::NotDone { why: Why::Gone, .. } => {
            taken.consume();
            WriteOutcome::Gone
        }
        ran => {
            // Sent again: counted, and its eight hours start over.
            taken.keep(HeldCreate {
                posts: held.posts + wire.count(),
                at: Instant::now(),
                ..held
            });
            let kept = match ran {
                Ran::NotDone {
                    why: Why::Refused(Verdict::Reauth) | Why::Unsent(Unsent::SignIn),
                    ..
                } => Kept::Reauth,
                Ran::NotDone {
                    why: Why::Refused(Verdict::RateLimited),
                    ..
                } => Kept::Busy,
                // A retry starts in doubt (`Run::retry_insert`), so nothing it meets is proof the
                // first copy isn't there.
                _ => Kept::Ambiguous,
            };
            kept.outcome(token)
        }
    }
}

/// A create's run proved it landed: Google's copy into the mirror, from the step's end.
async fn landed_create<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    target: &CalendarTarget,
    ends: &[End],
) -> WriteOutcome {
    match ends.first() {
        Some(End::Done {
            reply: Some(copy), ..
        }) => land_copy(wire, host, target, copy).await,
        // Proved without a copy to show: under `Shows::Created` every proof carries one, but the
        // next sync shows it anyway.
        _ => created_pending(host, target),
    }
}

/// Google's copy of a new event into the mirror. A one-off's copy is its row; a repeating event's is
/// its master, which never becomes a row (`calendar::parse_event`), so its occurrences are listed
/// and landed instead (C11).
async fn land_copy<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    target: &CalendarTarget,
    copy: &Value,
) -> WriteOutcome {
    if calendar::is_master(copy) {
        return land_new_series(wire, host, &CalIds::of(target), copy).await;
    }
    land_created(host, target, copy)
}

/// PM's and Google's ids for one calendar, for landing what a write made there.
struct CalIds<'a> {
    calendar_id: &'a str,
    remote_calendar: &'a str,
}

impl<'a> CalIds<'a> {
    fn of(target: &'a CalendarTarget) -> Self {
        CalIds {
            calendar_id: &target.calendar_id,
            remote_calendar: &target.remote_calendar,
        }
    }
}

/// A repeating event Google now holds, from its copy `copy` (its master): its occurrences in the
/// months PM mirrors, listed and landed as CREATED rows, so a list that lags can't take them off the
/// screen (A28). One with no occurrence in those months (it starts after them, or a COUNT or UNTIL
/// ended it before them) is saved and said to be outside, with nothing listed (C10 §6.2, C11
/// critique 1 L8). Looks up to three times (now, 1 s, 3 s) while the listing is empty or
/// incomplete; then "saved, the calendar will catch up" rather than a guess. No lock is held across
/// a listing; the landing takes the DB, then `recent`.
async fn land_new_series<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    cal: &CalIds<'_>,
    copy: &Value,
) -> WriteOutcome {
    let pending = || {
        drop_hash(host, cal.calendar_id);
        WriteOutcome::Saved {
            warnings: vec![WriteWarning::MirrorRefreshPending],
            left_behind: Vec::new(),
            milestone_moves: None,
        }
    };
    // Its id one path segment, its start and rule readable (never a guessed zone).
    let Ok(master) = Master::read(&FreshEvent::from_google(copy.clone())) else {
        return pending();
    };
    let Ok(mirror) = host.mirror(cal.calendar_id) else {
        return pending();
    };
    let Some(window) = mirror.instants() else {
        return pending();
    };
    if series_in_window(&master, window) == Some(false) {
        return WriteOutcome::Saved {
            warnings: vec![WriteWarning::OutsideMirror],
            left_behind: Vec::new(),
            milestone_moves: None,
        };
    }
    let mut listing = Listing::default();
    for pause in [None, Some(REFETCH_AGAIN[0]), Some(REFETCH_AGAIN[1])] {
        if let Some(pause) = pause {
            wire.wait(pause).await;
        }
        listing = refetch(wire, cal.remote_calendar, &master.id, &mirror.window, false).await;
        if listing.complete && !listing.items.is_empty() {
            break;
        }
    }
    if !listing.complete {
        return pending();
    }
    let rows: Vec<CalendarEvent> = listing
        .items
        .iter()
        .filter_map(|item| calendar::parse_event(cal.calendar_id, item))
        .collect();
    if rows.is_empty() {
        return pending();
    }
    // Every row is new: the mirror never held one of this event's.
    let created: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    match land_on(host, cal.calendar_id, rows, &created, Vec::new()) {
        Ok(()) => WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
            milestone_moves: None,
        },
        Err(_) => pending(),
    }
}

/// Whether a repeating event has an occurrence in the window PM mirrors (`[from, until)`): its
/// first at or after `from` comes before `until`. `None` when PM can't walk its rule, which lists it
/// anyway. An occurrence begun before `from` that runs into the window doesn't count: at worst the
/// next sync shows it.
fn series_in_window(
    master: &Master,
    (from, until): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>),
) -> Option<bool> {
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).ok()?;
    let (from, until) = match master.start {
        SeriesStart::AllDay(_) => (
            Point::Day(from.date_naive()),
            Point::Day(until.date_naive()),
        ),
        SeriesStart::Timed { .. } => (Point::At(from), Point::At(until)),
    };
    let first = expand::occurrences_from(&facts, &master.start, from, 1).ok()?;
    Some(
        first
            .first()
            .is_some_and(|at| at.cmp_in(&until, master.start.zone()) == std::cmp::Ordering::Less),
    )
}

/// A new event Google holds: its copy into the mirror as a CREATED row (a list that lags can't take
/// it off the screen, A28), unless it starts outside the months PM mirrors ([`in_mirror`]).
fn land_created(host: &impl Host, target: &CalendarTarget, copy: &Value) -> WriteOutcome {
    let Some(row) = calendar::parse_event(&target.calendar_id, copy) else {
        return created_pending(host, target);
    };
    let window = host
        .mirror(&target.calendar_id)
        .ok()
        .and_then(|m| m.instants());
    if in_mirror(&row, window) == Some(false) {
        return WriteOutcome::Saved {
            warnings: vec![WriteWarning::OutsideMirror],
            left_behind: Vec::new(),
            milestone_moves: None,
        };
    }
    let id = row.id.clone();
    match land_on(host, &target.calendar_id, vec![row], &[id], Vec::new()) {
        Ok(()) => WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
            milestone_moves: None,
        },
        Err(_) => created_pending(host, target),
    }
}

/// Created in Google, not mirrored yet: the next sync shows it.
fn created_pending(host: &impl Host, target: &CalendarTarget) -> WriteOutcome {
    drop_hash(host, &target.calendar_id);
    WriteOutcome::Saved {
        warnings: vec![WriteWarning::MirrorRefreshPending],
        left_behind: Vec::new(),
        milestone_moves: None,
    }
}

/// Whether a mirror row overlaps the window PM syncs (`[start, end)`, as instants): what the next
/// sync will fetch. Landing one outside it would show it until the created entry's ten minutes ran
/// out, then the next complete sync would drop it, so it isn't landed (C10 §6.2). `None` when the
/// row's times or the window can't be read, which lands it. All-day dates read as UTC midnight,
/// while Google bounds them in the calendar's zone: at the window's very edges a day can read a few
/// hours off, which at worst skips one that the next sync shows, or lands one it then drops. Never
/// a wrong write (C10 critique 1 L9).
fn in_mirror(
    row: &CalendarEvent,
    window: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
) -> Option<bool> {
    let (from, until) = window?;
    let instant = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
        if let Ok(at) = chrono::DateTime::parse_from_rfc3339(s.trim()) {
            return Some(at.with_timezone(&chrono::Utc));
        }
        chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|t| t.and_utc())
    };
    let start = instant(&row.start)?;
    let end = match row.end.as_deref() {
        Some(end) => instant(end)?,
        None => start,
    };
    Some(start < until && end > from)
}

/// A wire that counts the POSTs it passes on: the new event's copies, for [`MAX_POSTS`].
struct Posts<'w, W> {
    wire: &'w W,
    count: std::sync::atomic::AtomicUsize,
}

impl<'w, W: Wire> Posts<'w, W> {
    fn new(wire: &'w W) -> Self {
        Posts {
            wire,
            count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl<W: Wire> Wire for Posts<'_, W> {
    fn send(&self, plan: &RequestPlan) -> impl std::future::Future<Output = Sent> + Send {
        if plan.method == Method::Insert {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.wire.send(plan)
    }

    fn wait(&self, pause: Duration) -> impl std::future::Future<Output = ()> + Send {
        self.wire.wait(pause)
    }
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
    let state = app.state::<AppState>();
    // One lock for the row's target, its account's choice and whether it repeats (the DB mutex
    // isn't re-entrant).
    let (target, editing_choice, in_series) = {
        let conn = state.conn()?;
        match resolve_target(&conn, &event_id)? {
            Ok(target) => {
                let editing_choice = calendar_editing::choice(&conn, &target.email)?;
                let in_series = needs_scope(&conn, &event_id)?;
                (target, editing_choice, in_series)
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
    // An occurrence of a repeating event with no choice: ask now, rather than hold it for the Undo
    // window and refuse it at the send (rule R9). A one-off Google has turned into a series since
    // isn't one here; it holds, and the gate refuses it at the send.
    if scope.is_none() && in_series {
        return refused(WriteOutcome::NeedsChoice);
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
        let result = send_delete(&GoogleWire(&target.token_key), &task_app, &target, &seen).await;
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
async fn send_delete<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    target: &Target,
    seen: &SeenSummary,
) -> WriteOutcome {
    let editing = match host.editing(target) {
        Ok(editing) => editing,
        Err(e) => {
            return WriteOutcome::Failed {
                message: e.to_string(),
            }
        }
    };
    for round in 1..=2 {
        let fresh = match get(wire, &target.remote_calendar, &target.remote_event).await {
            Fetched::Event(fresh) => fresh,
            Fetched::Gone => return land_gone(host, target, None),
            Fetched::Reauth => return WriteOutcome::Reauth,
            Fetched::Busy => return WriteOutcome::Busy,
            Fetched::Failed(message) => return WriteOutcome::Failed { message },
        };
        if patch::is_gone(&fresh) {
            return land_gone(host, target, Some(&fresh));
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
        // An occurrence of a repeating event: which occurrences is the user's choice, never a
        // default (rule R9). Only a GET has gone out.
        if fresh.get("recurringEventId").is_some() {
            return WriteOutcome::NeedsChoice;
        }
        match patch::delete_still_matches(seen, &fresh) {
            DeleteCheck::Matches => {}
            DeleteCheck::Gone => return land_gone(host, target, Some(&fresh)),
            DeleteCheck::Changed(fields) => return land_changed(host, target, &fresh, fields),
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
        let reply = match wire.send(&request).await {
            Ok(reply) => reply,
            Err(google_io::NoAnswer::Unsent { why, .. }) => return unsent_outcome(why),
            // No answer, or a sign-in refused after a server error: look. Gone means it landed;
            // still there means it didn't (review H1).
            Err(google_io::NoAnswer::Lost { .. }) => {
                return match still_there(wire, target).await {
                    Some(false) => land_deleted(host, target, &fresh),
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
            match still_there(wire, target).await {
                Some(false) => return land_deleted(host, target, &fresh),
                Some(true) => {}
                None => return WriteOutcome::Unconfirmed,
            }
        }
        match reply.verdict {
            // 2xx, or already gone: either way it's gone from Google now.
            Verdict::Ok | Verdict::Gone => return land_deleted(host, target, &fresh),
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
        &app,
        &target,
        editing,
        &mirror,
        &request,
        open.as_ref().map(EditSession::opened),
    )
    .await)
}

// --- how a repeating event repeats, for the event popover (C9b-2) ---

/// How the repeating event a mirror row belongs to repeats, in Google Calendar's words ("Weekly on
/// Monday"), for the event popover. `None` when PM can't say: a one-off, a row that isn't Google's, a
/// series Google won't send or PM can't read, no network. Only a GET goes to Google. A read, so no
/// owner check (C9 design scope-L3): on a shared vault, someone else's profile simply has no sign-in
/// here, and the line reads "Repeats". `Err` only for the window and a DB PM can't open.
#[tauri::command]
pub async fn get_series_summary(
    app: AppHandle,
    window: tauri::Window,
    event_id: String,
) -> Result<Option<String>> {
    require_main_window(&window)?;
    let state = app.state::<AppState>();
    let ask = {
        let conn = state.conn()?;
        summary_ask(&conn, &event_id)
    };
    let Some(ask) = ask else { return Ok(None) };
    let token_key = ask.token_key.clone();
    Ok(series_summary(&GoogleWire(&token_key), &state.calendar_edit, &ask).await)
}

/// What a summary needs from the mirror: Google's calendar and series ids, the clicked row's etag,
/// and the sign-in to read with.
struct SummaryAsk {
    remote_calendar: String,
    series: String,
    /// `None` for a row mirrored before v57: never kept.
    etag: Option<String>,
    token_key: String,
}

/// `None` for a row PM can't ask about: gone from the mirror, a one-off, not Google's, an id that
/// isn't a plain path segment, or a DB read that failed. The popover then just says "Repeats".
fn summary_ask(conn: &Connection, row_id: &str) -> Option<SummaryAsk> {
    let row = calendar::event_by_id(conn, row_id).ok().flatten()?;
    let series = row.series_id.filter(|s| is_plain_segment(s))?;
    let target = resolve_target(conn, row_id).ok()?.ok()?;
    Some(SummaryAsk {
        remote_calendar: target.remote_calendar,
        series,
        etag: row.etag,
        token_key: target.token_key,
    })
}

/// Google's copy of the series `id` from a GET, when it is that series and not deleted.
fn series_copy(fetched: Fetched, id: &str) -> Option<Value> {
    match fetched {
        Fetched::Event(master)
            if !patch::is_gone(&master) && master.get("id").and_then(Value::as_str) == Some(id) =>
        {
            Some(master)
        }
        _ => None,
    }
}

/// The series' words: kept ones while they still stand for this row, else one GET. The editor's
/// "repeats" line comes from the same [`describe_lines`], so the two never disagree.
async fn series_summary<W: Wire>(
    wire: &W,
    edit: &CalendarEditState,
    ask: &SummaryAsk,
) -> Option<String> {
    let key = series_key(&ask.remote_calendar, &ask.series);
    let etag = ask.etag.as_deref();
    if let Some(words) = known_summary(edit, &key, etag, Instant::now()) {
        return Some(words);
    }
    let since = summary_writes(edit);
    let master = series_copy(
        get(wire, &ask.remote_calendar, &ask.series).await,
        &ask.series,
    )?;
    let read = Master::read(&FreshEvent::from_google(master)).ok()?;
    let words = describe_lines(&read.recurrence, &read.start);
    keep_summary(edit, key, etag, &words, since, Instant::now());
    Some(words)
}

/// How long a series' words stand, whatever the row etags say (C9b-2 §0.4.1: whether Google moves an
/// occurrence's etag when only its series' rule changes is unproven).
const SUMMARY_TTL: Duration = Duration::from_secs(15 * 60);
/// At most this many series' words are kept; past it the stalest go first.
const SUMMARY_KEEP: usize = 200;

/// The popover's kept words, and how many writes have let go of what they claimed.
#[derive(Default)]
struct Summaries {
    known: HashMap<String, KnownSummary>,
    /// Moves each time a write lets go ([`series_written`]): words read while one ended are never
    /// kept, nor words read while one still holds the series (C9b-2 critiques C1-M2, C2-L3). One
    /// count for every series, so an unrelated write only costs a read of Google again.
    writes: u64,
}

/// A series' words, and the row etags they were read for.
#[derive(Clone)]
struct KnownSummary {
    words: String,
    etags: HashSet<String>,
    read: Instant,
}

/// The words for `key`, if read within [`SUMMARY_TTL`] of `now` for a row at `etag`. A row with no
/// etag never hits, and a lock PM can't take reads as nothing kept.
fn known_summary(
    edit: &CalendarEditState,
    key: &str,
    etag: Option<&str>,
    now: Instant,
) -> Option<String> {
    let etag = etag?;
    let summaries = edit.summaries.lock().ok()?;
    summaries
        .known
        .get(key)
        .filter(|k| k.etags.contains(etag) && now.saturating_duration_since(k.read) < SUMMARY_TTL)
        .map(|k| k.words.clone())
}

/// Where the writes stood when an ask began, for [`keep_summary`].
fn summary_writes(edit: &CalendarEditState) -> Option<u64> {
    edit.summaries.lock().ok().map(|s| s.writes)
}

/// Keep `words` for `key`, for a row at `etag` (nothing kept without one), unless a write let go of
/// what it claimed since the ask began (`since`), or holds this series now. The same words read again
/// add the etag; different words replace the entry. Past [`SUMMARY_KEEP`], entries older than the TTL
/// go, then the oldest.
fn keep_summary(
    edit: &CalendarEditState,
    key: String,
    etag: Option<&str>,
    words: &str,
    since: Option<u64>,
    now: Instant,
) {
    let Some(etag) = etag else { return };
    // Taken alone, and let go before `summaries` is.
    let unclaimed = edit.in_flight.lock().is_ok_and(|held| !held.contains(&key));
    if !unclaimed {
        return;
    }
    let Ok(mut summaries) = edit.summaries.lock() else {
        return;
    };
    if since != Some(summaries.writes) {
        return;
    }
    if let Some(known) = summaries.known.get_mut(&key) {
        if known.words == words {
            known.etags.insert(etag.to_string());
            known.read = now;
            return;
        }
    }
    if !summaries.known.contains_key(&key) && summaries.known.len() >= SUMMARY_KEEP {
        summaries
            .known
            .retain(|_, k| now.saturating_duration_since(k.read) < SUMMARY_TTL);
        if summaries.known.len() >= SUMMARY_KEEP {
            let oldest = summaries
                .known
                .iter()
                .min_by_key(|(_, k)| k.read)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                summaries.known.remove(&oldest);
            }
        }
    }
    summaries.known.insert(
        key,
        KnownSummary {
            words: words.to_string(),
            etags: HashSet::from([etag.to_string()]),
            read: now,
        },
    );
}

/// A write let go of `keys`, after its landing: the words of every series among them go (row ids
/// match nothing), so a popover opened from now on reads Google again, and an ask on its way keeps
/// nothing it read. A lock PM can't take does nothing.
fn series_written(summaries: &Mutex<Summaries>, keys: &[String]) {
    if let Ok(mut summaries) = summaries.lock() {
        summaries.writes = summaries.writes.wrapping_add(1);
        for key in keys {
            summaries.known.remove(key);
        }
    }
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
    /// Whether a part PM split off it still overlaps it (scope-M4).
    parts: Parts,
    /// Where its earliest later part starts, which a new rule mustn't repeat over (C11 critique 1 H2).
    later: LaterPart,
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
        // saving it unscoped would change every occurrence with no warning (C9 design §0.7). The
        // gate refuses it first on every write path; the dev report reaches here without the gate.
        if !turned_series(fresh) {
            return Ok(None);
        }
        return Err(WriteOutcome::Failed {
            message: "This became a repeating event in Google. Refresh, then open one of its \
                      occurrences."
                .into(),
        });
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
    // The parts PM split off this series, whatever restarted since: one still overlapping it
    // refuses every change past this occurrence (C9 design review scope-M4).
    let parts = list_pages(
        wire,
        |page: Option<&str>| plan::split_parts(calendar, master_id, page),
        EXCEPTION_PAGES,
    )
    .await;
    let parts = Listing {
        items: parts.items,
        complete: parts.complete,
    };
    #[cfg(debug_assertions)]
    let listing_complete = listed.complete;
    let exceptions = Exceptions::sorted(listed.items, listed.complete).cross_checked(
        master_fresh.json(),
        &master,
        &window,
    );
    // The same parts, and Google's own `_R` ones the mirror holds, as where a new rule must stop.
    let later = later_part(&parts, &master, &mirror.series_ids);
    let parts = parts_overlap(&parts, &master, &window, &exceptions);
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
        parts,
        later,
        #[cfg(debug_assertions)]
        listing_complete,
        #[cfg(debug_assertions)]
        window,
    }))
}

/// [`series_choices`] past its shell: Google through `wire`; the changes PM couldn't finish and its
/// journal through `host`; the mirror's part in `mirror`; and for an edit `opened`, the occurrence
/// and the series as the editor opened them.
async fn choices<W: Wire, H: Host>(
    wire: &W,
    host: &H,
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
        Prepared::Changed { fresh, fields } => {
            return SeriesChoices::Refused {
                result: land_changed(host, target, &fresh, fields),
            }
        }
        Prepared::Refused(result) => return SeriesChoices::Refused { result },
    };
    let remote = target.remote_calendar.as_str();
    let master = planning.series.master.id.as_str();
    let held = host.edit().held_on(remote, master);
    let original = original_of(planning.instance.json());
    // A record of a change PM was making when it closed (review H3).
    let orphans = match orphaned(wire, host, remote, mirror, &planning).await {
        Ok(orphans) => orphans,
        Err(e) => {
            return SeriesChoices::Refused {
                result: WriteOutcome::Failed {
                    message: e.to_string(),
                },
            }
        }
    };
    // What the record says once, then forgets: the change stands, or Google changed it since. Every
    // record forgotten here is said, since none can be said again.
    let mut said = Vec::new();
    for found in &orphans {
        let e = &found.entry;
        match found.orphan {
            Orphan::Clear => forget(host, &e.calendar, &e.series, &e.tail),
            Orphan::CutLanded | Orphan::Changed => {
                said.push(found.words());
                forget(host, &e.calendar, &e.series, &e.tail);
            }
            Orphan::Twice | Orphan::Unknown => {}
        }
    }
    let notice = (!said.is_empty()).then(|| said.join(" "));
    let row = |scope: Scope| -> ScopeRow {
        // A change PM couldn't finish holds this series: a row it blocks is settled first, and the
        // save would answer the same (review M1).
        if let Some(view) = held.as_ref().filter(|v| blocks(v, master, original, scope)) {
            return ScopeRow::Pending {
                token: view.token.clone(),
                offers: view.offers.clone(),
                from: view.from.clone(),
            };
        }
        // One from before PM closed has no token to settle by: refused, with what to do.
        if let Some(found) = orphans.iter().find(|o| {
            matches!(o.orphan, Orphan::Twice | Orphan::Unknown)
                && o.reaches(master, original, scope)
        }) {
            return ScopeRow::Refused {
                message: found.words(),
            };
        }
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
        notice,
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
            later: self.series.later,
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
    /// A delete's occurrence changed in Google since the user saw it: Google's copy, for the caller
    /// to land (so a second delete compares against it, as the one-event delete does), and the
    /// fields.
    Changed {
        fresh: Value,
        fields: Vec<String>,
    },
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
    // A part split off earlier that still overlaps: refused past this occurrence, never undone
    // (scope-M4). Choices and the send plan here alike, so both refuse.
    if scope != Scope::This {
        let refused = |why| Planned::Planned(Err(NotPlanned::Refused(why)));
        match planning.series.parts {
            Parts::Overlap(day) => return refused(SeriesRefusal::SplitOverlap(day)),
            Parts::Unread => return refused(SeriesRefusal::PartsUnread),
            Parts::Clear => {}
        }
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
    // The gate on Google's copy. A one-off turned into a series keeps its own reason: nothing here
    // may clear `series_master`.
    let perms = gate::edit_rights(&facts_from_fresh(&fresh, &target.facts, editing));
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
            DeleteCheck::Changed(fields) => return Prepared::Changed { fresh, fields },
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
        SeriesWarning::BecomesOneEvent { day } => {
            SeriesWarningDto::BecomesOneEvent { day: date(day) }
        }
        SeriesWarning::ChangesLost { count } => SeriesWarningDto::ChangesLost { count },
    }
}

// --- sending a change to a repeating event (C9a-3) ---

const NOT_REPEATING: &str =
    "This event doesn't repeat, so there are no occurrences to choose from. Save it without choosing.";
const NOT_REPEATING_DELETE: &str = "This event doesn't repeat, so there are no occurrences to \
                                    choose from. Delete it without choosing.";
const UNFIT: &str = "PM built a change it isn't allowed to send, so nothing was saved.";
/// A new series' own POST answered 404 with nothing in doubt (C9a-3a critique L6).
const NO_CALENDAR: &str = "Google couldn't find this calendar for this account, so nothing was \
                           changed. Refresh the calendar and try again.";
const SETTLING: &str =
    "This change is already being settled, or it has been. Refresh the calendar \
                        to see where it stands.";
const FINISH_UNREAD: &str = "PM couldn't read every occurrence it has to check, so it didn't \
                             finish. Try again in a moment.";
const FINISH_CHANGED: &str = "Occurrences of this repeating event changed in Google since PM's \
                              save, so PM won't finish it. Remove the new part, then make the \
                              change again.";
const TAIL_GONE: &str = "The earlier part of this repeating event now ends before the day you \
                         chose, but the new part PM made from that day is gone from Google. Look \
                         for it in Google Calendar's Trash.";
const ONE_GONE: &str = "That occurrence was deleted in Google, so nothing else was changed.";
const OFFLINE: &str =
    "PM couldn't reach Google. Nothing was changed; try again when you're back online.";
const NOT_SENT: &str = "PM couldn't get this account's Google sign-in ready, so nothing was sent \
                        and nothing changed. Try again in a moment.";
const PUT_BACK: &str = "PM put this repeating event's times and repeat back as they were before \
                        this save. Anything else in this save may have gone through; PM's calendar \
                        shows it after the next refresh. Open the event again to change it.";
const RETIME_UNSURE: &str = "PM lost touch with Google while moving the occurrences, so they may \
                             have moved, or moved only partly. Check them in Google Calendar.";

/// Whether a scoped write may leave out the dialog's choice: dev builds only, for the devtools
/// probes. The one release gate for it, which every shell's [`SeriesWrite`] takes (a settle, which
/// plans nothing anew, takes `false`); a test reads this file to hold them to it (C9b-1 review M8).
const MISSING_CHOICE_OK: bool = cfg!(debug_assertions);

/// Whether the save names the plan the user chose. A missing choice passes only in dev builds
/// (the devtools probes); in release it's asked again (review scope-M2).
fn choice_matches(choice: Option<&str>, token: &str, missing_ok: bool) -> bool {
    match choice {
        Some(choice) => choice == token,
        None => missing_ok,
    }
}

/// What a write that wasn't proved done means for the user. `split`: a "this and following"
/// change's new part was removed with it.
fn not_done_outcome(why: Why, editing: EditingStatus, split: bool) -> WriteOutcome {
    match why {
        Why::Ignored => WriteOutcome::IgnoredByGoogle { split },
        Why::Conflict(fields) => WriteOutcome::Conflict { fields },
        Why::Gone => WriteOutcome::Gone,
        // Only a new series' first POST reads a 404 as a refusal: its calendar.
        Why::Refused(Verdict::Gone) => WriteOutcome::Failed {
            message: NO_CALENDAR.into(),
        },
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
    /// A new series' id for a plan that will be sent: [`plan::client_event_id`] in the app, a fixed
    /// one in the tests. Called once per plan (one plan, one tail id); a preview mints its own.
    new_series_id: fn() -> String,
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
    let tail = (ctx.new_series_id)();
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
            Prepared::Changed { fresh, fields } => {
                return Err(land_changed(ctx.host, ctx.target, &fresh, fields))
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
    // A change PM couldn't finish holds this series: nothing it blocks goes ahead until it's
    // settled (review M1). Only reads have gone out.
    let master = planning.series.master.id.as_str();
    let original = original_of(planning.instance.json());
    if let Some(view) = ctx.host.edit().held_on(&ctx.target.remote_calendar, master) {
        if blocks(&view, master, original, scope) {
            return Err(WriteOutcome::PartialSplit {
                token: view.token,
                offers: view.offers,
                from: view.from,
                note: None,
            });
        }
    }
    // One PM was making when it closed: Google's copies say what it came to (review H3). The claim
    // on this series is held, so no run of this PM is behind an entry found here.
    let remote = ctx.target.remote_calendar.as_str();
    let orphans = orphaned(ctx.wire, ctx.host, remote, &mirror, &planning)
        .await
        .map_err(|e| WriteOutcome::Failed {
            message: e.to_string(),
        })?;
    for found in orphans {
        let e = &found.entry;
        match found.orphan {
            Orphan::Clear => forget(ctx.host, &e.calendar, &e.series, &e.tail),
            _ if !found.reaches(master, original, scope) => {}
            // Said once, then it goes.
            Orphan::CutLanded | Orphan::Changed => {
                forget(ctx.host, &e.calendar, &e.series, &e.tail);
                return Err(WriteOutcome::Failed {
                    message: found.words(),
                });
            }
            Orphan::Twice | Orphan::Unknown => {
                return Err(WriteOutcome::Failed {
                    message: found.words(),
                })
            }
        }
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
    if !choice_matches(choice, &plan.choice_digest(&planning.bind), ctx.missing_ok) {
        return WriteOutcome::NeedsChoice;
    }
    // A new series is claimed too, through its landing and the look after a partial one: a sync
    // may mirror it mid-run, and a write to it then is Busy (C9a-3b critique M9).
    let _tail = match new_series(&plan) {
        Some(tail) => {
            let key = series_key(&ctx.target.remote_calendar, tail);
            match InFlight::claim(ctx.host.edit(), &key) {
                Some(claim) => Some(claim),
                None => return WriteOutcome::Busy,
            }
        }
        None => None,
    };
    let series_wide = plan.scope != Scope::This;
    let outcome = send_series(ctx, &mirror, &planning, plan, Landing::Edit { session }).await;
    // After a change past this occurrence, the editor's copies no longer stand for Google's.
    let ended = matches!(
        outcome,
        WriteOutcome::Saved { .. } | WriteOutcome::PartialSplit { .. }
    ) || matches!(&outcome, WriteOutcome::Failed { message } if message == PUT_BACK);
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
    // A "this and following" change lands and settles by facts read once, now.
    let mut facts = match new_series(&plan) {
        None => None,
        Some(_) => match split_facts(planning, &plan) {
            Some(facts) => Some(facts),
            None => {
                return WriteOutcome::Failed {
                    message: SeriesRefusal::Unreadable.to_string(),
                }
            }
        },
    };
    let mut run = match Run::new(plan) {
        Ok(run) => run,
        Err(Unfit) => {
            return WriteOutcome::Failed {
                message: UNFIT.into(),
            }
        }
    };
    let start = planning
        .series
        .master_fresh
        .json()
        .get("start")
        .unwrap_or(&Value::Null);
    let entry = match &facts {
        None => None,
        Some(facts) => match journal_entry(ctx.target, run.plan(), facts, start) {
            Some(entry) => Some(entry),
            None => {
                return WriteOutcome::Failed {
                    message: UNFIT.into(),
                }
            }
        },
    };
    // The series' rows before the first send, so a sync that lands mid-run can't hide which rows
    // are new (review M4). A new series has none yet. A split's record is kept in the same lock,
    // before anything goes out, so a quit mid-change leaves it (review H3); failing that, nothing
    // is sent. The record is this run's from before it's written to the end of the landing and look.
    // The milestones a split may offer to move are read in the same lock (C9b-2): a read that fails
    // stops the change, as a record that can't be kept does, rather than strand them unsaid.
    let _splitting = entry
        .as_ref()
        .map(|e| Splitting::mark(ctx.host.edit(), series_key(&e.calendar, &e.tail)));
    let snapshot = match ctx.host.with_conn(|conn| {
        let rows = calendar::series_rows(
            conn,
            &ctx.target.calendar_id,
            &[planning.series.master.id.as_str()],
        )?;
        let movable = match &facts {
            Some(facts) => read_movable(conn, ctx.host, planning, run.plan(), facts, &rows)?,
            None => None,
        };
        if let Some(entry) = &entry {
            journal_add(conn, entry)?;
        }
        Ok((rows, movable))
    }) {
        Ok((rows, movable)) => {
            if let Some(facts) = facts.as_mut() {
                facts.movable = movable;
            }
            rows
        }
        Err(e) => {
            return WriteOutcome::Failed {
                message: e.to_string(),
            }
        }
    };
    let ran = drive(ctx.wire, &mut run).await;
    let (Some(facts), Some(entry)) = (facts, entry) else {
        return land_series(ctx, mirror, planning, run.plan(), ran, snapshot, landing).await;
    };
    let plan = run.plan().clone();
    let outcome = land_split(ctx, mirror, plan, facts, ran, snapshot, None).await;
    // Held, or Google may hold part of it unsaid: the record stays. Otherwise it's settled. The
    // claims on both series are still held.
    if !matches!(
        outcome,
        WriteOutcome::PartialSplit { .. } | WriteOutcome::Unconfirmed
    ) {
        forget(ctx.host, &entry.calendar, &entry.series, &entry.tail);
    }
    outcome
}

/// The id of the new series `plan` inserts, for a "this and following" change.
fn new_series(plan: &SeriesPlan) -> Option<&str> {
    plan.steps.iter().find_map(|s| match &s.op {
        Op::Insert { id, .. } => Some(id.as_str()),
        _ => None,
    })
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
                // At the version planned from, so a restore after it shows.
                return land_gone(host, target, Some(planning.instance.json()));
            }
            let moments = series_moments(planning);
            return land_series_gone(host, cal, master, &snapshot, zone, &moments);
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
            return not_done_outcome(why, ctx.editing, false);
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
                    let before =
                        rows_union(&snapshot, calendar::series_rows(conn, cal, &[master])?);
                    let gone = plan.removes.rows(cal, &before, zone, floor.as_deref());
                    record(conn, host.edit(), cal, Vec::new(), &[], gone)
                });
                match landed {
                    Ok(()) => {
                        host.landed(cal);
                        WriteOutcome::Saved {
                            warnings: Vec::new(),
                            left_behind: Vec::new(),
                            milestone_moves: None,
                        }
                    }
                    Err(_) => saved_pending(host, target),
                }
            }
        };
    }
    // "Does not repeat" for every occurrence: Google's answer is the one event it now is (C11 §9.7).
    if plan.scope == Scope::All
        && matches!(&plan.steps[0].op,
            Op::Patch { body, .. } if body.get("recurrence").is_some_and(Value::is_null))
    {
        return land_one_event(ctx, mirror, planning, reply, date, snapshot).await;
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
        let before = rows_union(&snapshot, calendar::series_rows(conn, cal, &[master])?);
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

/// "Does not repeat" for every occurrence, landed (C11 §9.7; critiques 1 M2, 2 M1): Google's `copy`
/// (its answer) is the one event the series now is, recorded as created so a list that lags can't
/// take it off the screen, unless it's outside the months PM mirrors. No instances listing: the old
/// series' rows go, but those Google still holds live under its iCal UID (what it does with
/// occurrences changed on their own is probe P-R2's to show); a listing that didn't run to its end
/// removes none, and the next sync settles them. Floored at the answer's own time.
async fn land_one_event<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    mirror: &MirrorFacts,
    planning: &Planning,
    copy: Option<Value>,
    date: Option<chrono::DateTime<chrono::Utc>>,
    snapshot: Vec<CalendarEvent>,
) -> WriteOutcome {
    let (host, target) = (ctx.host, ctx.target);
    let cal = target.calendar_id.as_str();
    let master = planning.series.master.id.as_str();
    let zone = planning.series.master.start.zone();
    let pending = || {
        drop_hash(host, cal);
        host.landed(cal);
        series_saved(vec![WriteWarning::MirrorRefreshPending], Vec::new())
    };
    let Some(copy) = copy else {
        return pending();
    };
    let Some(row) = calendar::parse_event(cal, &copy) else {
        return pending();
    };
    let still: Option<Vec<String>> = match copy.get("iCalUID").and_then(Value::as_str) {
        Some(ical) => {
            let remote = target.remote_calendar.as_str();
            let listed = list_pages(
                ctx.wire,
                |page: Option<&str>| plan::list_by_ical(remote, ical, page),
                EXCEPTION_PAGES,
            )
            .await;
            listed.complete.then(|| {
                listed
                    .items
                    .iter()
                    .filter(|item| !patch::is_gone(item))
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(|id| format!("{cal}:{id}"))
                    .collect()
            })
        }
        None => None,
    };
    let floor = diff::floor_of(
        [updated_of(Some(&copy)), date_floor(date)]
            .iter()
            .flatten()
            .map(String::as_str),
    );
    let outside = in_mirror(&row, mirror.instants()) == Some(false);
    let landed = host.with_conn(|conn| {
        let before = rows_union(&snapshot, calendar::series_rows(conn, cal, &[master])?);
        let gone: Vec<GoneRow> = match &still {
            Some(still) => Removes::Series {
                series: master.to_string(),
            }
            .rows(cal, &before, zone, floor.as_deref())
            .into_iter()
            .filter(|g| !still.contains(&g.row_id) && g.row_id != row.id)
            .collect(),
            None => Vec::new(),
        };
        let (upserts, created) = if outside {
            (Vec::new(), Vec::new())
        } else {
            (vec![row.clone()], vec![row.id.clone()])
        };
        record(conn, host.edit(), cal, upserts, &created, gone)
    });
    if landed.is_err() {
        return pending();
    }
    host.landed(cal);
    let mut warnings = Vec::new();
    if outside {
        warnings.push(WriteWarning::OutsideMirror);
    }
    if still.is_none() {
        drop_hash(host, cal);
        warnings.push(WriteWarning::MirrorRefreshPending);
    }
    series_saved(warnings, Vec::new())
}

/// `Saved`, with `LeftOnOldSeries` when occurrences were left behind.
fn series_saved(mut warnings: Vec<WriteWarning>, left_behind: Vec<String>) -> WriteOutcome {
    if !left_behind.is_empty() {
        warnings.push(WriteWarning::LeftOnOldSeries);
    }
    WriteOutcome::Saved {
        warnings,
        left_behind,
        milestone_moves: None,
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
            Some(series) => calendar::series_rows(conn, cal, &[series.as_str()])?,
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
            calendar::series_rows(
                conn,
                &ctx.target.calendar_id,
                &[planning.series.master.id.as_str()],
            )
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
        missing_ok: MISSING_CHOICE_OK,
        new_series_id: plan::client_event_id,
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
    let held = HeldSeriesDelete {
        token: undo_token.clone(),
        event_id,
        seen,
        scope,
        plan: held,
    };
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _claim = claim;
        let undone = tokio::select! {
            _ = tokio::time::sleep(UNDO) => false,
            _ = undone => true,
        };
        let wire = GoogleWire(&target.token_key);
        if let Some(result) =
            finish_held_series_delete(&wire, &task_app, &target, &held, undone).await
        {
            let _ = task_app.emit_to(
                "main",
                "calendar://delete-settled",
                DeleteSettled {
                    undo_token: held.token.clone(),
                    event_id: held.event_id.clone(),
                    result,
                },
            );
        }
    });
    Ok(DeleteStart::Held {
        undo_token,
        undo_seconds: UNDO.as_secs(),
        hidden,
    })
}

/// A series delete waiting out its Undo window: its token there, the row and what the user saw of
/// it, the scope, and the plan chosen.
struct HeldSeriesDelete {
    token: String,
    event_id: String,
    seen: SeenSummary,
    scope: Scope,
    plan: HeldPlan,
}

/// A held series delete at the end of its wait (`undone`: an Undo ended it). Whoever takes its entry
/// decides, so an Undo that got there first wins; then editing is read again, since it may have
/// been turned off meanwhile, and the delete is planned again and sent. `None`: undone, nothing
/// sent and nothing to say.
async fn finish_held_series_delete<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    target: &Target,
    held: &HeldSeriesDelete,
    undone: bool,
) -> Option<WriteOutcome> {
    if undone || !host.edit().held_deletes.take(&held.token) {
        return None;
    }
    let editing = match editing_now(host, target) {
        Ok(editing) => editing,
        Err(result) => return Some(result),
    };
    let ctx = SeriesWrite {
        wire,
        host,
        target,
        editing,
        missing_ok: MISSING_CHOICE_OK,
        new_series_id: plan::client_event_id,
    };
    Some(send_held_series_delete(&ctx, &held.event_id, &held.seen, held.scope, &held.plan).await)
}

/// The account's editing status now, for a send after a wait.
fn editing_now(
    host: &impl Host,
    target: &Target,
) -> std::result::Result<EditingStatus, WriteOutcome> {
    let editing = host.editing(target).map_err(|e| WriteOutcome::Failed {
        message: e.to_string(),
    })?;
    if editing != EditingStatus::On {
        return Err(WriteOutcome::ReadOnly {
            reason: ReadOnlyReason::EditingOff,
        });
    }
    Ok(editing)
}

// --- "this and following": a new series, and settling a change PM couldn't finish (C9a-3b) ---

/// Settle a "this and following" change PM couldn't finish (`token`, from a `partial_split` outcome
/// or a `pending` row): look again, remove the new part PM made, or finish the change. What Google
/// shows now decides, read again first every time, and one settle of a change runs at a time.
#[tauri::command]
pub async fn settle_series_split(
    app: AppHandle,
    window: tauri::Window,
    token: String,
    action: SplitAction,
) -> Result<WriteOutcome> {
    require_main_window(&window)?;
    require_owner(&app)?;
    let state = app.state::<AppState>();
    let Some((target, _)) = state.calendar_edit.partial_claim(&token) else {
        return Ok(WriteOutcome::Failed {
            message: SETTLING.into(),
        });
    };
    let token_key = target.token_key.clone();
    Ok(settle_split(&GoogleWire(&token_key), &app, &token, action).await)
}

/// What a "this and following" change lands and settles by, read once at its send and kept with a
/// partial one.
#[derive(Clone)]
struct SplitFacts {
    /// The original series (its master's id), and the new part the change inserts.
    master: String,
    tail: String,
    /// The insert's step; the cut is the next.
    insert: usize,
    /// The occurrence the new part starts at, and the series' zone.
    at: Point,
    zone: chrono_tz::Tz,
    /// `at` as `YYYY-MM-DD` in `zone`, for the words.
    from: String,
    /// The series' and its listed exceptions' `updated` ([`series_moments`]): a gone original's floor.
    moments: Vec<String>,
    /// The original's exceptions from `at` on, as planned ([`marks_from`]): what a Finish compares a
    /// fresh listing with (review M5).
    marks: Vec<(String, serde_json::Map<String, Value>)>,
    /// The original's iCal UID, to list its exceptions again.
    ical: String,
    /// The milestones it may offer to move, read before its first send; `None`: none (C9b-2).
    movable: Option<Movable>,
}

/// The milestones a split may offer to move to its new part, read before its first send: after the
/// cut, the original's iCal UID resolves only to rows the earlier part keeps, and the landing makes
/// the calendar read milestones again before the save returns (C9b-2 §0.4.2).
#[derive(Clone)]
struct Movable {
    /// The user's zone, the one the milestone list resolves dates in: every day here is in it.
    zone: chrono_tz::Tz,
    /// The original's title and the new part's, as the plan sends them.
    earlier_title: String,
    new_title: String,
    /// Never empty.
    milestones: Vec<MovableMilestone>,
}

/// A milestone a split may offer to move, and where the occurrences on its date sit in the series.
#[derive(Clone)]
struct MovableMilestone {
    offer: MilestoneOffer,
    /// The slots (`originalStartTime`) of the original's rows on its date that the split moves to
    /// the new part: what the landing looks for on the earlier part. Empty when none was on it.
    slots: Vec<Point>,
}

/// What `facts`' split may offer to move, read under the DB lock the send's snapshot holds, `rows`
/// being the original's rows then: the unmet milestones on its iCal UID dated on or after the day
/// the change starts, as decided (DECISIONS 17-10 #2): "Earlier dates stay put with no prompt", even
/// one on an occurrence moved there from a later week, which the split takes to the new part. That
/// day is the one the user splits from as they see it, the "this day" the dialog names: the earlier
/// of the occurrence's place in the series and the day it shows on, when it was moved (C9b-2 review
/// round 3, R3-F1). Each keeps the slots of the occurrences on its date that the split moves, so the
/// landing can leave one whose occurrence stayed on the earlier part unasked ([`with_moves`],
/// C1-L9). Days are in the user's zone, the one the milestone list resolves dates in (the series'
/// zone stays for the dialog's words, C9b-2 critique C1-L8).
fn read_movable(
    conn: &Connection,
    host: &impl Host,
    planning: &Planning,
    plan: &SeriesPlan,
    facts: &SplitFacts,
    rows: &[CalendarEvent],
) -> Result<Option<Movable>> {
    let zone = super::shared::resolve_zone(conn);
    let slot_day = facts.at.day_in(zone).format("%Y-%m-%d").to_string();
    let split_day = start_day(planning.opened_instance.json(), zone)
        .filter(|shown| *shown < slot_day)
        .unwrap_or(slot_day);
    let found =
        milestones::linked_on_or_after(conn, &facts.ical, &split_day, &host.today_in(zone))?;
    let milestones: Vec<MovableMilestone> = found
        .into_iter()
        .filter_map(|m| {
            let date = m.due_date?.get(..10)?.to_string();
            let slots: Vec<Point> = rows
                .iter()
                .filter(|r| crate::clock::zone_date_of(&r.start, zone) == date)
                .filter_map(diff::row_point)
                .filter(|p| !p.cmp_in(&facts.at, facts.zone).is_lt())
                .collect();
            Some(MovableMilestone {
                offer: MilestoneOffer {
                    date,
                    id: m.id,
                    label: m.label,
                    project: m.project_name,
                },
                slots,
            })
        })
        .collect();
    if milestones.is_empty() {
        return Ok(None);
    }
    let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
    let inserted = match plan.steps.get(facts.insert).map(|s| &s.op) {
        Some(Op::Insert { body, .. }) => body.get("summary"),
        _ => None,
    };
    Ok(Some(Movable {
        zone,
        earlier_title: text(planning.series.master_fresh.json().get("summary")),
        new_title: text(inserted),
        milestones,
    }))
}

/// A landed split's `Saved`, with the milestones it offers to move to its new part (C9b-2). The new
/// part is named from what PM already holds (the insert's answer, then its landed rows), else by
/// one GET of it; one PM can't name still lists the milestones, beside a warning that they stayed
/// on the earlier part (C9b-2 critique C2-M1). A milestone on an occurrence left on the earlier part
/// stays there unasked (C1-L9): one the split was to move that the landing saw still there, or, when
/// the landing couldn't read Google, one whose cancel surely never landed (never sent, or refused).
/// One whose cancel's answer was lost, with nothing read since, may have gone, so it is asked about
/// and none goes overdue unsaid (C9b-2 review F5, round 2). Any other outcome, or nothing to offer,
/// passes through.
async fn with_moves<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    plan: &SeriesPlan,
    facts: &SplitFacts,
    ran: &Ran,
    outcome: WriteOutcome,
) -> WriteOutcome {
    let Some(movable) = &facts.movable else {
        return outcome;
    };
    let (mut warnings, left_behind) = match outcome {
        WriteOutcome::Saved {
            warnings,
            left_behind,
            milestone_moves: None,
        } => (warnings, left_behind),
        other => return other,
    };
    let zone = movable.zone;
    let cal = ctx.target.calendar_id.as_str();
    // Each part's rows as landed (none when the landing couldn't show them).
    let (tail_rows, earlier_rows) = ctx
        .host
        .with_conn(|conn| {
            Ok((
                calendar::series_rows(conn, cal, &[facts.tail.as_str()])?,
                calendar::series_rows(conn, cal, &[facts.master.as_str()])?,
            ))
        })
        .unwrap_or_default();
    // Where the earlier part's occurrences sit in the series: as the landing read them from Google,
    // else, as one pending keeps rows PM can't vouch for, those whose cancel surely never landed.
    let seen: Vec<Point> = if warnings.contains(&WriteWarning::MirrorRefreshPending) {
        plan.left_behind(&ran.not_done())
    } else {
        earlier_rows.iter().filter_map(diff::row_point).collect()
    };
    let milestones: Vec<MilestoneOffer> = movable
        .milestones
        .iter()
        .filter(|m| m.slots.is_empty() || m.slots.iter().any(|s| !seen.contains(s)))
        .map(|m| m.offer.clone())
        .collect();
    if milestones.is_empty() {
        return WriteOutcome::Saved {
            warnings,
            left_behind,
            milestone_moves: None,
        };
    }
    let mut copy = inserted_copy(ran, facts.insert).cloned();
    let mut named = copy.as_ref().and_then(ical_of).or_else(|| {
        tail_rows
            .iter()
            .find_map(|r| r.uid.as_deref().and_then(plain_text))
    });
    if named.is_none() {
        if let Fetched::Event(fetched) =
            get(ctx.wire, &ctx.target.remote_calendar, &facts.tail).await
        {
            if !patch::is_gone(&fetched) {
                named = ical_of(&fetched);
                copy = Some(fetched);
            }
        }
    }
    let new_series_ical = named.filter(|ical| *ical != facts.ical);
    if new_series_ical.is_none() {
        warnings.push(WriteWarning::MilestoneNotOffered);
    }
    // The new part's first occurrence, and the earlier part's last, as the user's calendar shows them:
    // the earlier part's by where its rows sit in the series, so one left from the split on, which
    // may still show, isn't its last.
    let day_of =
        |r: &CalendarEvent| Some(crate::clock::zone_date_of(&r.start, zone)).filter(|d| is_day(d));
    let first_day = tail_rows
        .iter()
        .filter_map(day_of)
        .min()
        .or_else(|| copy.as_ref().and_then(|c| start_day(c, zone)));
    let earlier_last_day = earlier_rows
        .iter()
        .filter(|r| diff::row_point(r).is_some_and(|p| p.cmp_in(&facts.at, facts.zone).is_lt()))
        .filter_map(day_of)
        .max();
    WriteOutcome::Saved {
        warnings,
        left_behind,
        milestone_moves: Some(Box::new(MilestoneMoves {
            new_series_ical,
            earlier_series_ical: facts.ical.clone(),
            new_title: movable.new_title.clone(),
            earlier_title: movable.earlier_title.clone(),
            first_day,
            earlier_last_day,
            today: ctx.host.today_in(zone),
            milestones,
        })),
    }
}

/// A copy's iCal UID, when it has a usable one.
fn ical_of(copy: &Value) -> Option<String> {
    copy.get("iCalUID")
        .and_then(Value::as_str)
        .and_then(plain_text)
}

/// `s` trimmed, unless that leaves nothing.
fn plain_text(s: &str) -> Option<String> {
    Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Whether `s` is a `YYYY-MM-DD` day.
fn is_day(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// The day a Google copy starts on in `zone`, `YYYY-MM-DD`.
fn start_day(copy: &Value, zone: chrono_tz::Tz) -> Option<String> {
    let start = copy.get("start")?;
    let day = match start.get("dateTime").and_then(Value::as_str) {
        Some(at) => crate::clock::zone_date_of(at, zone),
        None => start.get("date").and_then(Value::as_str)?.to_string(),
    };
    is_day(&day).then_some(day)
}

/// `None` for a plan not shaped as a split, or a series without an iCal UID (whose exceptions
/// couldn't have been listed whole, so the planners refused it first).
fn split_facts(planning: &Planning, plan: &SeriesPlan) -> Option<SplitFacts> {
    let insert = plan
        .steps
        .iter()
        .position(|s| matches!(s.op, Op::Insert { .. }))?;
    let at = plan.expect.iter().find_map(|e| match e {
        Expect::EndsBefore { at, .. } => Some(*at),
        _ => None,
    })?;
    let master = planning.series.master.id.clone();
    let zone = planning.series.master.start.zone();
    Some(SplitFacts {
        tail: new_series(plan)?.to_string(),
        insert,
        at,
        zone,
        from: at.day_in(zone).format("%Y-%m-%d").to_string(),
        moments: series_moments(planning),
        marks: marks_from(&planning.series.exceptions.items, &master, at, zone),
        ical: planning
            .series
            .master_fresh
            .json()
            .get("iCalUID")
            .and_then(Value::as_str)?
            .to_string(),
        master,
        movable: None,
    })
}

/// A change the run left partial: everything a settle needs to read Google again and act.
#[derive(Clone)]
struct HeldSplit {
    target: Target,
    /// The very plan that ran: its new series' id and its etags.
    plan: SeriesPlan,
    /// Where it stopped.
    ran: Ran,
    facts: SplitFacts,
    /// What the last look allowed.
    offers: Vec<SplitAction>,
    /// Google's answer to the insert (or the copy that proved it), when known: the only copy that
    /// stands for the new part as PM made it, so a later copy at its etag is unchanged. A look's copy
    /// never does: it may already hold a change made in Google (review round 1).
    tail_seen: Option<Value>,
    /// The new part was changed in Google since PM made it: neither Remove nor Finish, until a look
    /// shows it gone or as it was (C9a-3b critiques M6, M7).
    tail_changed: bool,
}

/// What a blocked change or a choices row sees of a held split.
struct HeldView {
    token: String,
    offers: Vec<SplitAction>,
    from: String,
    master: String,
    tail: String,
    at: Point,
    zone: chrono_tz::Tz,
}

impl CalendarEditState {
    /// The split held under `token`: its event, and the claim keys of both its series.
    fn partial_claim(&self, token: &str) -> Option<(Target, [String; 2])> {
        let partials = self.partials.lock().ok()?;
        let held = partials.get(token)?;
        let remote = held.target.remote_calendar.as_str();
        Some((
            held.target.clone(),
            [
                series_key(remote, &held.facts.master),
                series_key(remote, &held.facts.tail),
            ],
        ))
    }

    /// Whether the split whose new part is `key` ([`series_key`]) is running now. A lock PM can't
    /// take reads as not running, so its record is read from Google.
    fn splitting(&self, key: &str) -> bool {
        self.splitting
            .lock()
            .is_ok_and(|running| running.contains(key))
    }

    /// What the split held under `token` allows now, and its day: `None` once it's settled or taken.
    fn held_split(&self, token: &str) -> Option<(Vec<SplitAction>, String)> {
        let partials = self.partials.lock().ok()?;
        let held = partials.get(token)?;
        Some((held.offers.clone(), held.facts.from.clone()))
    }

    fn hold_partial(&self, token: String, held: HeldSplit) {
        if let Ok(mut partials) = self.partials.lock() {
            partials.insert(token, held);
        }
    }

    /// The split held on `series` of `remote_calendar`, as its original or as its new part.
    fn held_on(&self, remote_calendar: &str, series: &str) -> Option<HeldView> {
        let partials = self.partials.lock().ok()?;
        partials
            .iter()
            .find(|(_, h)| {
                h.target.remote_calendar == remote_calendar
                    && (h.facts.master == series || h.facts.tail == series)
            })
            .map(|(token, h)| HeldView {
                token: token.clone(),
                offers: h.offers.clone(),
                from: h.facts.from.clone(),
                master: h.facts.master.clone(),
                tail: h.facts.tail.clone(),
                at: h.facts.at,
                zone: h.facts.zone,
            })
    }
}

/// A split taken out to settle: held again under its token when this goes, unless it was settled,
/// a panic included, so a settle that stops partway never loses it (C9a-3b critique M-3).
struct TakenSplit<'a> {
    edit: &'a CalendarEditState,
    token: String,
    back: Option<HeldSplit>,
}

impl<'a> TakenSplit<'a> {
    /// Take the split out, so a second settle of it finds nothing: the guard, and a copy to work on.
    fn take(edit: &'a CalendarEditState, token: &str) -> Option<(Self, HeldSplit)> {
        let held = edit.partials.lock().ok()?.remove(token)?;
        let taken = TakenSplit {
            edit,
            token: token.to_string(),
            back: Some(held.clone()),
        };
        Some((taken, held))
    }

    /// What goes back: `held`, as the latest look left it.
    fn keep(&mut self, held: &HeldSplit) {
        self.back = Some(held.clone());
    }

    /// Nothing goes back: it's settled, or a new run decides.
    fn settled(&mut self) {
        self.back = None;
    }
}

impl Drop for TakenSplit<'_> {
    fn drop(&mut self) {
        if let Some(held) = self.back.take() {
            self.edit
                .hold_partial(std::mem::take(&mut self.token), held);
        }
    }
}

/// A "this and following" change running, marked in [`CalendarEditState::splitting`] until this goes.
struct Splitting<'a> {
    edit: &'a CalendarEditState,
    key: String,
}

impl<'a> Splitting<'a> {
    /// Mark the split whose new part is `key` ([`series_key`]). Before its record is written, or
    /// before it leaves `partials`, so no moment shows a record with neither (review round 3).
    fn mark(edit: &'a CalendarEditState, key: String) -> Self {
        if let Ok(mut running) = edit.splitting.lock() {
            running.insert(key.clone());
        }
        Splitting { edit, key }
    }
}

impl Drop for Splitting<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.edit.splitting.lock() {
            running.remove(&self.key);
        }
    }
}

/// A partial split's token: PM's own random id, never Google's.
fn new_split_ref() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// What's known of a partial split's cut, read off how its run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CutKind {
    /// Never sent: the insert before it is unknown.
    NotSent,
    /// Sent, and unknown.
    Sent,
    /// Proved not done, the new part's delete unproved.
    Proved,
}

/// The cut's state for a run that left a split partial, `None` for any other run.
fn cut_kind(insert: usize, ran: &Ran) -> Option<CutKind> {
    match ran {
        Ran::Unknown { step, .. } if *step == insert => Some(CutKind::NotSent),
        Ran::Unknown { step, ends, .. }
            if *step == insert + 1 && matches!(ends.get(insert), Some(End::Done { .. })) =>
        {
            Some(CutKind::Sent)
        }
        Ran::NotDone {
            rollback: Rollback::Unknown { step },
            ..
        } if *step == insert => Some(CutKind::Proved),
        _ => None,
    }
}

/// What a look at a partial split allows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Offers {
    /// The new part is gone and the original wasn't cut: nothing left to settle.
    Removed,
    /// The original is gone from Google: nothing to finish, and the new part stays.
    OriginalGone,
    Hold(Vec<SplitAction>),
}

/// The offers Google's state allows (C9 design §2.4; plan.rs: "unknown offers neither"). Remove
/// never after a cut that landed, and neither Remove nor a Finish that would send the cut while the
/// new part was changed in Google since; Check whenever Google's state can still change by itself.
fn offers_for(cut: CutKind, settled: &Settled, tail_changed: bool) -> Offers {
    use SplitAction::{Check, Finish, Remove};
    let cut_done = matches!(settled, Settled::CutDone { .. });
    let mut offers = match (cut, settled) {
        (_, Settled::Removed) => return Offers::Removed,
        (_, Settled::OriginalGone) => return Offers::OriginalGone,
        (_, Settled::CutDone { .. }) => vec![Finish],
        (CutKind::NotSent, Settled::TailThere { .. }) => vec![Finish, Remove],
        // The cut was never sent, so Remove is safe whatever the look found.
        (CutKind::NotSent, Settled::TailAbsent | Settled::Unknown) => vec![Remove, Check],
        (CutKind::Sent | CutKind::Proved, Settled::CutNotDone) => vec![Remove],
        // Only Remove's fence can prove it (run.rs).
        (CutKind::Sent, Settled::CutUnproved) => vec![Remove, Check],
        (CutKind::Proved, Settled::Unknown) => vec![Remove, Check],
        _ => vec![Check],
    };
    if tail_changed {
        offers.retain(|o| *o != Remove && (*o != Finish || cut_done));
        if !offers.contains(&Check) {
            offers.push(Check);
        }
    }
    Offers::Hold(offers)
}

/// Whether the held split blocks `scope` on an occurrence of `series` at `original` (its slot in
/// the series; review M1).
fn blocks(view: &HeldView, series: &str, original: Option<Point>, scope: Scope) -> bool {
    let at = Some((view.at, view.zone));
    reaches(&view.master, &view.tail, at, series, original, scope)
}

/// Whether a split of `master` into `tail` from `at` reaches `scope` on an occurrence of `series` at
/// `original`: every scope on the new part, and on the original every one but "this event" before
/// the split. With `at` unknown, every scope on either.
fn reaches(
    master: &str,
    tail: &str,
    at: Option<(Point, chrono_tz::Tz)>,
    series: &str,
    original: Option<Point>,
    scope: Scope,
) -> bool {
    if series == tail {
        return true;
    }
    if series != master {
        return false;
    }
    match (scope, original, at) {
        (Scope::This, Some(p), Some((at, zone))) => p.cmp_in(&at, zone) != std::cmp::Ordering::Less,
        _ => true,
    }
}

/// `YYYY-MM-DD` as the design system writes a date, DD-MM-YYYY.
fn day_words(from: &str) -> String {
    chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .map(|d| d.format("%d-%m-%Y").to_string())
        .unwrap_or_else(|_| from.to_string())
}

fn tail_changed_words(from: &str) -> String {
    format!(
        "The new part PM made from {} has been changed in Google since, so PM won't remove or \
         finish it. Change it back or delete it in Google Calendar, then check again.",
        day_words(from)
    )
}

fn original_gone_words(from: &str) -> String {
    format!(
        "The earlier part of this repeating event was deleted in Google, so there's nothing left \
         to finish. The new part from {} is still there.",
        day_words(from)
    )
}

fn partial_outcome(token: &str, held: &HeldSplit) -> WriteOutcome {
    WriteOutcome::PartialSplit {
        token: token.to_string(),
        offers: held.offers.clone(),
        from: held.facts.from.clone(),
        note: None,
    }
}

/// Google's copy of the new part from the insert's answer (or the look that proved it), if known.
fn inserted_copy(ran: &Ran, insert: usize) -> Option<&Value> {
    match ran.ends().get(insert) {
        Some(End::Done {
            reply: Some(copy), ..
        }) => Some(copy),
        _ => None,
    }
}

/// The new part is gone from Google and nothing is left to settle: its rows go, and when the run
/// found the original deleted in Google (its cut answered 404), the original's too, which is then
/// what to say (review round 1). `None`: the original stands.
fn land_removed(
    host: &impl Host,
    cal: &str,
    facts: &SplitFacts,
    ran: &Ran,
    seen: &[Option<&Value>],
) -> Option<WriteOutcome> {
    land_tail_removed(host, cal, facts, seen);
    match ran {
        Ran::NotDone {
            step,
            why: Why::Gone,
            ..
        } if *step == facts.insert + 1 => Some(land_series_gone(
            host,
            cal,
            &facts.master,
            &[],
            facts.zone,
            &facts.moments,
        )),
        _ => None,
    }
}

/// The original series is gone from Google: every row of it goes, no older than its last copies.
fn land_series_gone(
    host: &impl Host,
    cal: &str,
    master: &str,
    snapshot: &[CalendarEvent],
    zone: chrono_tz::Tz,
    moments: &[String],
) -> WriteOutcome {
    let floor = diff::floor_of(moments.iter().map(String::as_str));
    let _ = host.with_conn(|conn| {
        let before = rows_union(snapshot, calendar::series_rows(conn, cal, &[master])?);
        let gone = Removes::Series {
            series: master.to_string(),
        }
        .rows(cal, &before, zone, floor.as_deref());
        record(conn, host.edit(), cal, Vec::new(), &[], gone)
    });
    host.landed(cal);
    WriteOutcome::Gone
}

/// The new part PM made is gone from Google: its rows, if a sync wrote any meanwhile, go too, no
/// older than the copies of it PM saw (C9a-3b critiques L2, L3). A new part that doesn't repeat is a
/// row of its own (C11).
fn land_tail_removed(host: &impl Host, cal: &str, facts: &SplitFacts, seen: &[Option<&Value>]) {
    let moments: Vec<String> = seen.iter().filter_map(|copy| updated_of(*copy)).collect();
    let floor = diff::floor_of(moments.iter().map(String::as_str));
    let _ = host.with_conn(|conn| {
        let mut rows = calendar::series_rows(conn, cal, &[facts.tail.as_str()])?;
        rows.extend(calendar::event_by_id(
            conn,
            &format!("{cal}:{}", facts.tail),
        )?);
        let mut gone = Removes::Series {
            series: facts.tail.clone(),
        }
        .rows(cal, &rows, facts.zone, floor.as_deref());
        gone.extend(
            Removes::Occurrence {
                event: facts.tail.clone(),
            }
            .rows(cal, &rows, facts.zone, floor.as_deref()),
        );
        record(conn, host.edit(), cal, Vec::new(), &[], gone)
    });
    host.landed(cal);
}

/// What a "this and following" run came to, in the mirror and in words. `token`: the partial a
/// Finish ran from, held again under it if this run is partial too.
async fn land_split<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    mirror: &MirrorFacts,
    plan: SeriesPlan,
    facts: SplitFacts,
    ran: Ran,
    snapshot: Vec<CalendarEvent>,
    token: Option<String>,
) -> WriteOutcome {
    let (host, cal) = (ctx.host, ctx.target.calendar_id.as_str());
    if let Some(kind) = cut_kind(facts.insert, &ran) {
        return look_and_hold(ctx, plan, facts, ran, kind, &snapshot, token).await;
    }
    match &ran {
        Ran::Done { .. } => {
            let saved = land_split_done(ctx, mirror, &plan, &facts, &ran, snapshot).await;
            with_moves(ctx, &plan, &facts, &ran, saved).await
        }
        Ran::NotDone {
            step,
            why,
            rollback,
            ..
        } => {
            // Proved not done: the cut, its rollback having deleted the new part (`Done`), or the
            // insert itself (`Nothing`). The new part never reached the mirror but through a sync,
            // whose rows go too.
            let split = *rollback == Rollback::Done;
            if split {
                land_tail_removed(host, cal, &facts, &[]);
            }
            if *why == Why::Gone && *step == facts.insert + 1 {
                return land_series_gone(
                    host,
                    cal,
                    &facts.master,
                    &snapshot,
                    facts.zone,
                    &facts.moments,
                );
            }
            not_done_outcome(why.clone(), ctx.editing, split)
        }
        // Every partial shape is `cut_kind`'s, and cleanup never ends a run unknown.
        Ran::Unknown { .. } => {
            drop_hash(host, cal);
            WriteOutcome::Unconfirmed
        }
    }
}

/// A split the run left partial: one look at Google first (reads only), then hold it with what that
/// look allows, or say how it settled.
async fn look_and_hold<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    plan: SeriesPlan,
    facts: SplitFacts,
    ran: Ran,
    kind: CutKind,
    snapshot: &[CalendarEvent],
    token: Option<String>,
) -> WriteOutcome {
    let (host, cal) = (ctx.host, ctx.target.calendar_id.as_str());
    let Some(mut look) = Settle::new(plan.clone(), &ran, SettleAction::Check) else {
        drop_hash(host, cal);
        return WriteOutcome::Unconfirmed;
    };
    let settled = drive(ctx.wire, &mut look).await;
    // Only the insert's own answer stands for the new part as PM made it: a look's copy may already
    // hold a change made in Google since (review round 1).
    let tail_seen = inserted_copy(&ran, facts.insert).cloned();
    match offers_for(kind, &settled, false) {
        // A rollback's delete that couldn't be proved turns out done: the run's own result.
        Offers::Removed => {
            if let Some(gone) = land_removed(host, cal, &facts, &ran, &[tail_seen.as_ref()]) {
                return gone;
            }
            match ran {
                Ran::NotDone { why, .. } => not_done_outcome(why, ctx.editing, true),
                _ => WriteOutcome::SplitRemoved,
            }
        }
        Offers::OriginalGone => {
            land_series_gone(
                host,
                cal,
                &facts.master,
                snapshot,
                facts.zone,
                &facts.moments,
            );
            WriteOutcome::Failed {
                message: original_gone_words(&facts.from),
            }
        }
        Offers::Hold(offers) => {
            // Google may hold part of it: the next sync rewrites the rows.
            drop_hash(host, cal);
            host.landed(cal);
            let token = token.unwrap_or_else(new_split_ref);
            let from = facts.from.clone();
            host.edit().hold_partial(
                token.clone(),
                HeldSplit {
                    target: ctx.target.clone(),
                    plan,
                    ran,
                    facts,
                    offers: offers.clone(),
                    tail_seen,
                    tail_changed: false,
                },
            );
            WriteOutcome::PartialSplit {
                token,
                offers,
                from,
                note: None,
            }
        }
    }
}

/// A split that ran to its end: both series re-fetched, then landed in one mirror write.
async fn land_split_done<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    mirror: &MirrorFacts,
    plan: &SeriesPlan,
    facts: &SplitFacts,
    ran: &Ran,
    snapshot: Vec<CalendarEvent>,
) -> WriteOutcome {
    let (host, cal) = (ctx.host, ctx.target.calendar_id.as_str());
    let remote = ctx.target.remote_calendar.as_str();
    let skipped = ran.skipped();
    // The cut's answer, never the insert's: the new part's `updated` would floor the original's rows.
    let floor = match ran.ends().get(facts.insert + 1) {
        Some(End::Done { reply, .. }) => updated_of(reply.as_ref()),
        _ => None,
    };
    let left_behind: Vec<String> = plan
        .left_behind(&skipped)
        .iter()
        .map(|p| p.day_in(facts.zone).format("%Y-%m-%d").to_string())
        .collect();
    let expect = plan.expect_after(&skipped);
    let removal = Removal {
        plan,
        skipped: &skipped,
        snapshot: &snapshot,
        zone: facts.zone,
        floor: floor.clone(),
    };
    let Some(window) = mirror.instants() else {
        return stale_saved(host, cal, removal, left_behind);
    };
    // A split that stops repeating made one event in place of a new part: it lands from its insert's
    // own answer, never a listing, since what Google lists for a one-off's occurrences is unproven
    // (C11 §9.7). One outside the months PM mirrors isn't landed (C10 §6.2).
    let one_event = plan
        .expect
        .iter()
        .any(|e| matches!(e, Expect::Event { .. }));
    let event_row = if one_event {
        match inserted_copy(ran, facts.insert).and_then(|copy| calendar::parse_event(cal, copy)) {
            Some(row) => Some(row),
            None => return stale_saved(host, cal, removal, left_behind),
        }
    } else {
        None
    };
    let outside = event_row
        .as_ref()
        .is_some_and(|row| in_mirror(row, Some(window)) == Some(false));
    let (mut original, mut tail) = (Listing::default(), Listing::default());
    let mut stale = true;
    for pause in [None, Some(REFETCH_AGAIN[0]), Some(REFETCH_AGAIN[1])] {
        if let Some(pause) = pause {
            ctx.wire.wait(pause).await;
        }
        // Neither series is meant to be gone, so a 404 is never an empty listing (review M7).
        original = refetch(ctx.wire, remote, &facts.master, &mirror.window, false).await;
        if !one_event {
            tail = refetch(ctx.wire, remote, &facts.tail, &mirror.window, false).await;
        }
        let probe = diff::diff_series(
            cal,
            &snapshot,
            &split_refetched(facts, one_event, &original, &tail),
            window,
            &expect,
            facts.zone,
            floor.as_deref(),
        );
        // Every row of the new part is new: half of it would land the original's removals with
        // only some of their replacements (review M8).
        stale = probe.stale || !original.complete || (!one_event && !tail.complete);
        if !stale {
            break;
        }
    }
    if stale {
        return stale_saved(host, cal, removal, left_behind);
    }
    let landed = host.with_conn(|conn| {
        let now = calendar::series_rows(conn, cal, &[facts.master.as_str(), facts.tail.as_str()])?;
        let before = rows_union(&snapshot, now);
        let diff = diff::diff_series(
            cal,
            &before,
            &split_refetched(facts, one_event, &original, &tail),
            window,
            &expect,
            facts.zone,
            floor.as_deref(),
        );
        let gone =
            plan.removes_for(&diff, &skipped)
                .rows(cal, &before, facts.zone, floor.as_deref());
        // The snapshot holds none of the new part's rows, so every one of them is created: a list
        // that lags behind can't take them off the screen.
        let mut created = diff.created_since(&snapshot);
        let mut deletes = diff.deletes;
        for row in gone {
            if !deletes.iter().any(|d| d.row_id == row.row_id) {
                deletes.push(row);
            }
        }
        let mut upserts = diff.upserts;
        if let Some(row) = event_row.clone().filter(|_| !outside) {
            created.push(row.id.clone());
            upserts.push(row);
        }
        deletes.retain(|d| !upserts.iter().any(|u| u.id == d.row_id));
        record(conn, host.edit(), cal, upserts, &created, deletes)
    });
    let outside = outside.then_some(WriteWarning::OutsideMirror);
    match landed {
        Ok(()) => {
            host.landed(cal);
            series_saved(outside.into_iter().collect(), left_behind)
        }
        Err(_) => {
            drop_hash(host, cal);
            series_saved(vec![WriteWarning::MirrorRefreshPending], left_behind)
        }
    }
}

/// The series a landed split re-fetched: the original, and the new part unless it's one event
/// (`one_event`), which lands from its insert's answer instead.
fn split_refetched<'a>(
    facts: &'a SplitFacts,
    one_event: bool,
    original: &'a Listing,
    tail: &'a Listing,
) -> Vec<Refetch<'a>> {
    let mut out = vec![Refetch {
        series: facts.master.as_str(),
        listing: original,
    }];
    if !one_event {
        out.push(Refetch {
            series: facts.tail.as_str(),
            listing: tail,
        });
    }
    out
}

/// The new part PM made, as Google holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TailCheck {
    /// There, this plan's, and as PM made it, with no occurrence changed on its own, at `etag`: the
    /// only version Remove may then delete.
    Unchanged {
        etag: String,
    },
    /// Changed in Google since PM made it, or an occurrence of it was.
    Changed,
    /// Deleted, or not this plan's.
    Gone,
    Unread,
}

/// Look at the new part again before Remove or Finish: a change made to it in Google since is the
/// user's, never PM's to delete or carry over (C9a-3b critiques M5, M6, M11). Its exceptions need a
/// whole listing to say there are none.
async fn tail_check<W: Wire>(wire: &W, held: &HeldSplit) -> TailCheck {
    let (remote, insert) = (held.target.remote_calendar.as_str(), held.facts.insert);
    let copy = match get(wire, remote, &held.facts.tail).await {
        Fetched::Event(copy) => copy,
        Fetched::Gone => return TailCheck::Gone,
        _ => return TailCheck::Unread,
    };
    if held.plan.verdict(insert, Some(&copy)) != Applied::Yes {
        return TailCheck::Gone;
    }
    let Some(etag) = copy.get("etag").and_then(Value::as_str).map(str::to_string) else {
        return TailCheck::Unread;
    };
    // At the etag of the insert's own answer it's as PM made it, however Google wrote it out;
    // otherwise it must still hold what the insert sent.
    let same_etag = held
        .tail_seen
        .as_ref()
        .and_then(|seen| seen.get("etag"))
        .and_then(Value::as_str)
        == Some(etag.as_str());
    if !same_etag && !held.plan.tail_as_inserted(insert, &copy) {
        return TailCheck::Changed;
    }
    let Some(ical) = copy.get("iCalUID").and_then(Value::as_str) else {
        return TailCheck::Unread;
    };
    let listed = list_pages(
        wire,
        |page: Option<&str>| plan::list_by_ical(remote, ical, page),
        EXCEPTION_PAGES,
    )
    .await;
    if !listed.complete {
        return TailCheck::Unread;
    }
    // The original's deleted occurrences went over as EXDATEs, which Google may list as cancelled.
    let left_out = held.plan.tail_exdates(insert);
    let changed = listed.items.iter().any(|item| {
        item.get("recurringEventId").and_then(Value::as_str) == Some(held.facts.tail.as_str())
            && !(patch::is_gone(item) && original_of(item).is_some_and(|p| left_out.contains(&p)))
    });
    if changed {
        TailCheck::Changed
    } else {
        TailCheck::Unchanged { etag }
    }
}

/// The settle command past its shell: Google through `wire`; the held split, the claims and the
/// mirror through `host`. A settle that leaves the change held says so, whatever stopped it.
async fn settle_split<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    token: &str,
    action: SplitAction,
) -> WriteOutcome {
    let outcome = settle_split_once(wire, host, token, action).await;
    still_held(host.edit(), token, outcome)
}

/// A settle's `outcome`, as the change still held when PM holds it under `token` yet: its offers
/// now and the outcome's words as the note, so the notice keeps its buttons (C9b-1 critiques 1 H2,
/// 2 H1). A result that ends the change, or a token that's gone (`SETTLING`), stays as it is.
fn still_held(edit: &CalendarEditState, token: &str, outcome: WriteOutcome) -> WriteOutcome {
    let note = match &outcome {
        WriteOutcome::Failed { message } => message.clone(),
        // PM's own claim: an 8-second held delete on this repeating event, or a save on it.
        WriteOutcome::Busy => {
            "PM is still changing this repeating event; try again in a moment.".to_string()
        }
        WriteOutcome::ReadOnly { reason } => read_only_message(*reason).to_string(),
        WriteOutcome::Unconfirmed => "PM lost touch with Google; check again.".to_string(),
        WriteOutcome::Reauth => {
            "Google's sign-in for this account needs renewing. Reconnect it, then try again."
                .to_string()
        }
        WriteOutcome::Conflict { .. } | WriteOutcome::NeedsChoice => {
            "This repeating event changed in Google meanwhile; check again.".to_string()
        }
        WriteOutcome::PartialSplit { .. }
        | WriteOutcome::Saved { .. }
        | WriteOutcome::NoChange
        | WriteOutcome::Gone
        | WriteOutcome::IgnoredByGoogle { .. }
        | WriteOutcome::SplitRemoved
        // Only creating an event answers this; a settle never does.
        | WriteOutcome::Ambiguous { .. } => return outcome,
    };
    let Some((offers, from)) = edit.held_split(token) else {
        return outcome;
    };
    WriteOutcome::PartialSplit {
        token: token.to_string(),
        offers,
        from,
        note: Some(note),
    }
}

/// [`settle_split`] up to its answer.
async fn settle_split_once<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    token: &str,
    action: SplitAction,
) -> WriteOutcome {
    let edit = host.edit();
    let settling = || WriteOutcome::Failed {
        message: SETTLING.into(),
    };
    let Some((peeked, keys)) = edit.partial_claim(token) else {
        return settling();
    };
    match host.settle_allowed(&peeked.remote_calendar) {
        Ok(Ok(())) => {}
        Ok(Err(message)) => {
            return WriteOutcome::Failed {
                message: message.into(),
            }
        }
        Err(e) => {
            return WriteOutcome::Failed {
                message: e.to_string(),
            }
        }
    }
    // Both series claimed, then the split taken out: a second settle, or a save on either part,
    // meets Busy or finds nothing to settle (review H2; C9a-3b critique M1).
    let Some(_flight) = InFlight::claim_all(edit, &keys) else {
        return WriteOutcome::Busy;
    };
    // Marked running before it leaves `partials` (the second key is its new part's).
    let [_, tail_key] = keys;
    let _splitting = Splitting::mark(edit, tail_key);
    let Some((taken, held)) = TakenSplit::take(edit, token) else {
        return settling();
    };
    let remote = held.target.remote_calendar.clone();
    let (master, tail) = (held.facts.master.clone(), held.facts.tail.clone());
    let outcome = settle_taken(wire, host, token, action, taken, held).await;
    // The claims still held, and the split held again unless it's settled: a settled one's record
    // goes (review H3). An unconfirmed end keeps it, for Google to be read again.
    if !matches!(outcome, WriteOutcome::Unconfirmed) && edit.held_on(&remote, &master).is_none() {
        forget(host, &remote, &master, &tail);
    }
    outcome
}

/// A settle once the split is taken out (`taken`, a copy in `held`): it goes back when `taken` does,
/// unless settled.
async fn settle_taken<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    token: &str,
    action: SplitAction,
    mut taken: TakenSplit<'_>,
    mut held: HeldSplit,
) -> WriteOutcome {
    let editing = match editing_now(host, &held.target) {
        Ok(editing) => editing,
        Err(outcome) => return outcome,
    };
    let Some(kind) = cut_kind(held.facts.insert, &held.ran) else {
        return WriteOutcome::Unconfirmed;
    };
    let target = held.target.clone();
    let cal = target.calendar_id.as_str();
    let ctx = SeriesWrite {
        wire,
        host,
        target: &target,
        editing,
        missing_ok: false,
        // A settle never plans anew: it runs the plan it holds.
        new_series_id: plan::client_event_id,
    };
    // Look first, every time: Google's state now decides what may happen.
    let Some(mut look) = Settle::new(held.plan.clone(), &held.ran, SettleAction::Check) else {
        return WriteOutcome::Unconfirmed;
    };
    let settled = drive(wire, &mut look).await;
    if held.tail_changed
        && matches!(
            tail_check(wire, &held).await,
            TailCheck::Unchanged { .. } | TailCheck::Gone
        )
    {
        // Changed back, or deleted in Google since: it no longer stops anything.
        held.tail_changed = false;
    }
    match offers_for(kind, &settled, held.tail_changed) {
        Offers::Removed => {
            taken.settled();
            let seen = [inserted_copy(&held.ran, held.facts.insert)];
            return land_removed(host, cal, &held.facts, &held.ran, &seen)
                .unwrap_or(WriteOutcome::SplitRemoved);
        }
        Offers::OriginalGone => {
            taken.settled();
            let facts = &held.facts;
            land_series_gone(host, cal, &facts.master, &[], facts.zone, &facts.moments);
            return WriteOutcome::Failed {
                message: original_gone_words(&facts.from),
            };
        }
        Offers::Hold(offers) => held.offers = offers,
    }
    taken.keep(&held);
    if action == SplitAction::Check || !held.offers.contains(&action) {
        return partial_outcome(token, &held);
    }
    if action == SplitAction::Remove {
        remove_split(&ctx, &mut taken, held, kind, token).await
    } else {
        finish_split(&ctx, &mut taken, held, settled, token).await
    }
}

/// Remove the new part, once Google shows the original wasn't cut and the new part as PM made it.
async fn remove_split<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    taken: &mut TakenSplit<'_>,
    mut held: HeldSplit,
    kind: CutKind,
    token: &str,
) -> WriteOutcome {
    let checked = match tail_check(ctx.wire, &held).await {
        TailCheck::Changed => {
            held.tail_changed = true;
            held.offers = vec![SplitAction::Check];
            taken.keep(&held);
            return WriteOutcome::Failed {
                message: tail_changed_words(&held.facts.from),
            };
        }
        TailCheck::Unread => return partial_outcome(token, &held),
        // Deleted only at the version just checked (review round 1).
        TailCheck::Unchanged { etag } => Some(etag),
        // Gone: the machine finds it so, and says removed.
        TailCheck::Gone => None,
    };
    let Some(remove) = Settle::new(held.plan.clone(), &held.ran, SettleAction::Remove) else {
        return WriteOutcome::Unconfirmed;
    };
    let mut remove = match checked {
        Some(etag) => remove.at_tail_etag(etag),
        None => remove,
    };
    let settled = drive(ctx.wire, &mut remove).await;
    match offers_for(kind, &settled, held.tail_changed) {
        Offers::Removed => {
            taken.settled();
            let seen = [inserted_copy(&held.ran, held.facts.insert)];
            land_removed(
                ctx.host,
                &ctx.target.calendar_id,
                &held.facts,
                &held.ran,
                &seen,
            )
            .unwrap_or(WriteOutcome::SplitRemoved)
        }
        Offers::Hold(offers) => {
            held.offers = offers;
            taken.keep(&held);
            partial_outcome(token, &held)
        }
        // Only a look reads the original gone.
        Offers::OriginalGone => partial_outcome(token, &held),
    }
}

/// Finish the change from where it stopped, once Google shows the part before it landed: never on
/// a new part or occurrences changed in Google since (review M5).
async fn finish_split<W: Wire, H: Host>(
    ctx: &SeriesWrite<'_, W, H>,
    taken: &mut TakenSplit<'_>,
    mut held: HeldSplit,
    settled: Settled,
    token: &str,
) -> WriteOutcome {
    let (host, cal) = (ctx.host, ctx.target.calendar_id.as_str());
    let remote = ctx.target.remote_calendar.as_str();
    let cut_done = matches!(settled, Settled::CutDone { .. });
    let unread = || WriteOutcome::Failed {
        message: FINISH_UNREAD.into(),
    };
    let tail = tail_check(ctx.wire, &held).await;
    match tail {
        TailCheck::Unread => return unread(),
        TailCheck::Gone if cut_done => {
            // Cut, and the new part gone: nothing PM can finish or undo.
            taken.settled();
            drop_hash(host, cal);
            host.landed(cal);
            return WriteOutcome::Failed {
                message: TAIL_GONE.into(),
            };
        }
        TailCheck::Gone => {
            held.offers = vec![SplitAction::Check];
            taken.keep(&held);
            return partial_outcome(token, &held);
        }
        TailCheck::Changed if !cut_done => {
            held.tail_changed = true;
            held.offers = vec![SplitAction::Check];
            taken.keep(&held);
            return WriteOutcome::Failed {
                message: tail_changed_words(&held.facts.from),
            };
        }
        TailCheck::Changed | TailCheck::Unchanged { .. } => {}
    }
    // The original's exceptions from the split on, listed again: as the plan saw them, or no Finish.
    let listed = list_pages(
        ctx.wire,
        |page: Option<&str>| plan::list_by_ical(remote, &held.facts.ical, page),
        EXCEPTION_PAGES,
    )
    .await;
    if !listed.complete {
        return unread();
    }
    // The original deleted in Google while the split was held: never cut it now, whose rollback
    // would delete the new part, then the only copy of what follows. Said, landed, nothing sent
    // (review round 4).
    let original_there = listed.items.iter().any(|item| {
        item.get("id").and_then(Value::as_str) == Some(held.facts.master.as_str())
            && !patch::is_gone(item)
    });
    if !cut_done && !original_there {
        taken.settled();
        let facts = &held.facts;
        land_series_gone(host, cal, &facts.master, &[], facts.zone, &facts.moments);
        return WriteOutcome::Failed {
            message: original_gone_words(&facts.from),
        };
    }
    let facts = &held.facts;
    let changed = tail == TailCheck::Changed
        || marks_from(&listed.items, &facts.master, facts.at, facts.zone) != facts.marks;
    let mirror = match host.mirror(cal) {
        Ok(mirror) => mirror,
        Err(e) => {
            return WriteOutcome::Failed {
                message: e.to_string(),
            }
        }
    };
    let snapshot =
        match host.with_conn(|conn| calendar::series_rows(conn, cal, &[facts.master.as_str()])) {
            Ok(rows) => rows,
            Err(e) => {
                return WriteOutcome::Failed {
                    message: e.to_string(),
                }
            }
        };
    if changed {
        let Settled::CutDone { original } = settled else {
            // Not cut: nothing stands yet, and Remove still undoes it.
            held.offers = vec![SplitAction::Remove];
            taken.keep(&held);
            return WriteOutcome::Failed {
                message: FINISH_CHANGED.into(),
            };
        };
        // Cut, and what the change would move changed since: what Google holds stands, and the
        // occurrences it would have moved are said to be left behind. Nothing is sent (Q2).
        let cut = held.facts.insert + 1;
        let mut ends = held.ran.ends().to_vec();
        for (i, end) in ends.iter_mut().enumerate().skip(cut) {
            *end = if i == cut {
                End::Done {
                    reply: Some(original.clone()),
                    date: None,
                }
            } else {
                End::Skipped
            };
        }
        taken.settled();
        return land_split(
            ctx,
            &mirror,
            held.plan,
            held.facts,
            Ran::Done { ends },
            snapshot,
            None,
        )
        .await;
    }
    let mut run = match Run::finish(held.plan.clone(), &held.ran, &settled) {
        Ok(run) => run,
        Err(Unfit) => {
            return WriteOutcome::Failed {
                message: UNFIT.into(),
            }
        }
    };
    // From here the run decides, held again or settled: the old record never comes back, since its
    // reading of the cut no longer holds (C9a-3b critique M8).
    taken.settled();
    let ran = drive(ctx.wire, &mut run).await;
    let plan = run.plan().clone();
    land_split(
        ctx,
        &mirror,
        plan,
        held.facts,
        ran,
        snapshot,
        Some(token.to_string()),
    )
    .await
}

// --- the journal: a "this and following" change that outlives PM (C9a-3b-2) ---

/// What PM keeps of a split before its first write (review H3): Google's ids, the plan's note, the
/// original's rule before and after the cut, and its start. `None` for a plan not shaped as one.
fn journal_entry(
    target: &Target,
    plan: &SeriesPlan,
    facts: &SplitFacts,
    start: &Value,
) -> Option<Entry> {
    let Shows::Tail { plan: digest, .. } = &plan.steps.get(facts.insert)?.shows else {
        return None;
    };
    let Shows::Fields { sent, old } = &plan.steps.get(facts.insert + 1)?.shows else {
        return None;
    };
    let lines = |v: Option<&Value>| -> Option<Vec<String>> {
        v?.as_array()?
            .iter()
            .map(|line| line.as_str().map(str::to_string))
            .collect()
    };
    Some(Entry {
        calendar: target.remote_calendar.clone(),
        series: facts.master.clone(),
        tail: facts.tail.clone(),
        plan: digest.clone(),
        from: facts.from.clone(),
        at: journal::point_text(facts.at),
        zone: facts.zone.name().to_string(),
        start: start.clone(),
        before: lines(old.get("recurrence"))?,
        cut: lines(sent.get("recurrence"))?,
        cleanup: plan.steps.len() > facts.insert + 2,
    })
}

fn stored_journal(conn: &Connection) -> Result<journal::Journal> {
    Ok(journal::read(
        crate::db::get_setting(conn, journal::SETTINGS_KEY)?.as_deref(),
    ))
}

fn store_journal(conn: &Connection, all: &journal::Journal) -> Result<()> {
    match journal::write(all) {
        Some(json) => crate::db::set_setting(conn, journal::SETTINGS_KEY, &json),
        None => crate::db::delete_setting(conn, journal::SETTINGS_KEY),
    }
}

/// Add `entry`, under the DB lock already held.
fn journal_add(conn: &Connection, entry: &Entry) -> Result<()> {
    let mut all = stored_journal(conn)?;
    all.insert(journal::key(&entry.calendar, &entry.series), entry.clone());
    store_journal(conn, &all)
}

/// Take the split of `series` into `tail` out of the journal, best effort: a change that began since
/// keeps its own entry (its new part has another id). One that stays is read from Google next time,
/// so it heals itself.
fn forget(host: &impl Host, calendar: &str, series: &str, tail: &str) {
    let key = journal::key(calendar, series);
    let _ = host.with_conn(|conn| {
        let mut all = stored_journal(conn)?;
        if all.get(&key).is_none_or(|e| e.tail != tail) {
            return Ok(());
        }
        all.remove(&key);
        store_journal(conn, &all)
    });
}

/// A journal entry on a series that no split held in memory stands for: one left from before PM
/// restarted (or a run that ended unconfirmed), as Google's copies read now.
struct Orphaned {
    entry: Entry,
    orphan: Orphan,
}

impl Orphaned {
    /// Whether it reaches `scope` on an occurrence of `series` at `original`, as a held split would.
    fn reaches(&self, series: &str, original: Option<Point>, scope: Scope) -> bool {
        let e = &self.entry;
        let at = e.point().zip(e.tz());
        reaches(&e.series, &e.tail, at, series, original, scope)
    }

    /// What to say of it. After a restart PM only refuses and says what Google shows; it deletes
    /// nothing and offers no delete, even of a proven duplicate (DECISIONS 18-10, Q4 as narrowed by
    /// C9a-3b critique C1-H1): the way out is in Google Calendar.
    fn words(&self) -> String {
        let day = day_words(&self.entry.from);
        match self.orphan {
            Orphan::Twice => format!(
                "PM closed while it was changing this repeating event, so it shows twice from {day}. \
                 In Google Calendar, delete the newer copy (Delete › All events on it), then try \
                 again."
            ),
            Orphan::CutLanded => format!(
                "PM closed before it finished changing this repeating event from {day}: \
                 occurrences you changed on their own may not have moved to the new part. Check \
                 them in Google Calendar."
            ),
            Orphan::Changed => format!(
                "PM closed while it was changing this repeating event from {day}, and its repeat \
                 has changed in Google since, so PM can't tell how that change ended. Check it in \
                 Google Calendar."
            ),
            Orphan::Unknown | Orphan::Clear => format!(
                "PM closed while it was changing this repeating event from {day}, and can't check \
                 it now. Try again in a moment."
            ),
        }
    }
}

/// A GET as a look at one series.
fn look(fetched: &Fetched) -> Look<'_> {
    match fetched {
        Fetched::Event(copy) => Look::Copy(copy),
        Fetched::Gone => Look::Gone,
        _ => Look::Unread,
    }
}

/// Every journal entry on `planning`'s series, as its original or its new part, that no held split
/// stands for, read against Google's copies now: the planned series' own copy, fetched once, and a
/// GET of the other. Reads only.
async fn orphaned<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    remote: &str,
    mirror: &MirrorFacts,
    planning: &Planning,
) -> Result<Vec<Orphaned>> {
    let series = planning.series.master.id.as_str();
    let this = planning.series.master_fresh.json();
    let edit = host.edit();
    let mut found = Vec::new();
    for entry in host.with_conn(stored_journal)?.into_values() {
        if entry.calendar != remote || (entry.series != series && entry.tail != series) {
            continue;
        }
        // A split held in memory stands for it, or the send or settle running it now: either way
        // it's no orphan. Claims alone can't tell, since two unrelated writes may hold both series.
        let running = edit.splitting(&series_key(remote, &entry.tail));
        if running || edit.held_on(remote, &entry.series).is_some() {
            continue;
        }
        let fetch_other = |id: &str| {
            let id = id.to_string();
            async move {
                if is_plain_segment(&id) {
                    get(wire, remote, &id).await
                } else {
                    Fetched::Failed(String::new())
                }
            }
        };
        let (tail, original) = if entry.tail == series {
            (None, Some(fetch_other(&entry.series).await))
        } else {
            (Some(fetch_other(&entry.tail).await), None)
        };
        let mut orphan = journal::orphan(
            &entry,
            tail.as_ref().map_or(Look::Copy(this), look),
            original.as_ref().map_or(Look::Copy(this), look),
        );
        if orphan == Orphan::Twice {
            // Shown twice only while the original still repeats from the split on: read off the
            // occurrences, as the backstop reads them (review round 1).
            let overlap = match &original {
                // Its exceptions read whole, as the new part's side requires (review round 3).
                None if !planning.series.exceptions.complete => Parts::Unread,
                None => planning.series.parts,
                Some(Fetched::Event(copy)) => {
                    original_overlap(wire, remote, mirror, copy, this).await
                }
                Some(_) => Parts::Unread,
            };
            orphan = journal::backed(orphan, overlap);
        }
        found.push(Orphaned { entry, orphan });
    }
    Ok(found)
}

/// Whether the original of `entry` still repeats past the new part `tail` (Google's copy of it), read
/// off its occurrences in the mirror's window.
async fn original_overlap<W: Wire>(
    wire: &W,
    remote: &str,
    mirror: &MirrorFacts,
    original: &Value,
    tail: &Value,
) -> Parts {
    // Read as gather reads a series: its exceptions listed whole and cross-checked, so one changed
    // on its own and left on it doesn't count as the rule repeating (review round 2).
    let original = FreshEvent::from_google(original.clone());
    let Ok(master) = Master::read(&original) else {
        return Parts::Unread;
    };
    let Some(ical) = original.json().get("iCalUID").and_then(Value::as_str) else {
        return Parts::Unread;
    };
    if !is_plain_segment(&master.id) {
        return Parts::Unread;
    }
    let listed = list_pages(
        wire,
        |page: Option<&str>| plan::list_by_ical(remote, ical, page),
        EXCEPTION_PAGES,
    )
    .await;
    let (time_min, time_max) = (mirror.window.0.as_str(), mirror.window.1.as_str());
    let window = list_pages(
        wire,
        |page: Option<&str>| {
            let mut query = vec![
                ("timeMin", time_min),
                ("timeMax", time_max),
                ("maxResults", "250"),
                ("fields", WINDOW_FIELDS),
            ];
            query.extend(page.map(|p| ("pageToken", p)));
            plan::instances(remote, &master.id, &query)
        },
        OCCURRENCE_PAGES,
    )
    .await;
    let window = Listing {
        items: window.items,
        complete: window.complete,
    };
    let exceptions = Exceptions::sorted(listed.items, listed.complete).cross_checked(
        original.json(),
        &master,
        &window,
    );
    if !exceptions.complete {
        return Parts::Unread;
    }
    let parts = Listing {
        items: vec![tail.clone()],
        complete: true,
    };
    parts_overlap(&parts, &master, &window, &exceptions)
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
    /// A settle reaches more than one occurrence (a whole new part, a cut series), so it fails closed
    /// with the variable unset, as "this and following" and "all events" do.
    pub(super) const SETTLE_SERIES_WIDE: bool = true;

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
async fn after_lost_answer<W: Wire, H: Host>(
    wire: &W,
    host: &H,
    target: &Target,
    session: &str,
    body: &serde_json::Map<String, Value>,
) -> WriteOutcome {
    match get(wire, &target.remote_calendar, &target.remote_event).await {
        Fetched::Event(now) if patch::already_applied(&now, body) => {
            land_saved(host, target, session, now, body)
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
async fn still_there<W: Wire>(wire: &W, target: &Target) -> Option<bool> {
    match get(wire, &target.remote_calendar, &target.remote_event).await {
        Fetched::Gone => Some(false),
        Fetched::Event(now) => Some(!patch::is_gone(&now)),
        _ => None,
    }
}

/// What a request that never reached Google means for the user: nothing changed.
fn unsent_outcome(why: Unsent) -> WriteOutcome {
    not_done_outcome(Why::Unsent(why), EditingStatus::On, false)
}

/// What a settled verdict means for the user.
fn outcome_for(verdict: Verdict, editing: EditingStatus) -> WriteOutcome {
    match verdict {
        Verdict::Ok => WriteOutcome::Saved {
            warnings: Vec::new(),
            left_behind: Vec::new(),
            milestone_moves: None,
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
    // The editor's own reason lines (`reasonText`), so a row of the choices refused for one says
    // why, as the editor would (C9b-1 critique 2 M4).
    match reason {
        ReadOnlyReason::NotGoogle => {
            "This event comes from a calendar PM can only read (a subscription or Outlook)."
        }
        ReadOnlyReason::EditingOff => {
            "Editing isn't on for this Google account. Turn it on in Settings › Connectors › \
             Google Calendar."
        }
        ReadOnlyReason::CalendarReadOnly => {
            "This calendar is shared with you to view only, so its events can't be changed here."
        }
        ReadOnlyReason::PrivateEvent => {
            "This is a private event on a calendar shared with you, and you can't change private \
             events there."
        }
        ReadOnlyReason::NotOrganizer => {
            "Someone else organises this event, so only they can change it."
        }
        ReadOnlyReason::BecameRepeating => {
            "This became a repeating event in Google. Refresh, then open one of its occurrences."
        }
        ReadOnlyReason::HasGuests => {
            "This event has guests, and PM can't change events with guests yet."
        }
        ReadOnlyReason::SpecialType => {
            "Birthdays, events from Gmail, focus time, out of office and working locations can't \
             be changed here."
        }
        ReadOnlyReason::Locked => {
            "Google has locked this event's title, time, place and description."
        }
        ReadOnlyReason::HtmlDescription => {
            "This description has formatting, so PM shows its text here and can't change it yet."
        }
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
    /// The account's editing status now: its stored choice, then its token's scope (a keychain read,
    /// with the DB lock let go first; the tests set it instead).
    fn editing(&self, target: &Target) -> Result<EditingStatus> {
        let choice = self.with_conn(|conn| calendar_editing::choice(conn, &target.email))?;
        Ok(editing_status(choice, &target.token_key))
    }
    /// Whether a settle may write to the Google calendar `remote`: a dev build's allow-list, failing
    /// closed when unset, since a settle reaches a whole new part and a cut series (C9a-3a critique
    /// 9). Release builds settle; dev builds check the allow-list (fails closed when unset).
    fn settle_allowed(&self, remote: &str) -> Result<std::result::Result<(), &'static str>> {
        #[cfg(debug_assertions)]
        {
            self.with_conn(|conn| dev_allow::check(conn, remote, dev_allow::SETTLE_SERIES_WIDE))
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = remote;
            Ok(Ok(()))
        }
    }
    /// Today in `zone`, `YYYY-MM-DD`: the day a split's linked milestones are resolved on (C9b-2;
    /// the tests fix it).
    fn today_in(&self, zone: chrono_tz::Tz) -> String {
        crate::clock::today_sql_in(zone)
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

/// A delete met an event Google changed since the user saw it: show what Google holds now, so the
/// row that comes back is the changed one and a second delete compares against it rather than
/// conflicting again. The one-event delete and a repeating event's (its choices and its send) alike.
fn land_changed(
    host: &impl Host,
    target: &Target,
    fresh: &Value,
    fields: Vec<String>,
) -> WriteOutcome {
    if let Some(row) = calendar::parse_event(&target.calendar_id, fresh) {
        let _ = land(host, target, vec![row], Vec::new());
    }
    WriteOutcome::Conflict { fields }
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
        milestone_moves: None,
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
            milestone_moves: None,
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
            milestone_moves: None,
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
        release_claims(self.edit, &self.keys);
    }
}

/// Let go of `keys`, then, after the landing, forget the words of every series among them, so a
/// popover opened from now on reads Google again. Both kinds of claim end here. The claims' lock goes
/// before the words' is taken, each alone ([`CalendarEditState`]): words an ask keeps in between go
/// with the rest, and an ask that keeps after sees the writes moved ([`keep_summary`]).
fn release_claims(edit: &CalendarEditState, keys: &[String]) {
    if let Ok(mut held) = edit.in_flight.lock() {
        for key in keys {
            held.remove(key);
        }
    }
    series_written(&edit.summaries, keys);
}

/// A claim that outlives the command that took it ([`InFlight::hand_off`]); released on drop.
struct HeldClaim {
    app: AppHandle,
    keys: Vec<String>,
}

impl Drop for HeldClaim {
    fn drop(&mut self) {
        release_claims(
            &self.app.state::<AppState>().inner().calendar_edit,
            &self.keys,
        );
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
        assert!(f.organizer_self && !f.has_guests && f.html_description && !f.series_master);
        let invite = json!({
            "organizer": { "email": "boss@x.com" }, "recurringEventId": "s1",
            "attendees": [{ "email": "me@x.com", "self": true }, { "email": "boss@x.com" }]
        });
        let f = facts_from_fresh(&invite, &facts, EditingStatus::On);
        assert!(!f.organizer_self && f.has_guests && !f.series_master);
        assert_eq!(gate::edit_block(&f), Some(ReadOnlyReason::NotOrganizer));
        // An occurrence, a one-off, and a one-off Google has since given a rule: only the last is a
        // series itself, with its own reason (C9 design critique 2 M6).
        let mine = || json!({ "organizer": { "self": true }, "eventType": "default" });
        let mut occurrence = mine();
        occurrence["recurringEventId"] = json!("s1");
        let mut turned = mine();
        turned["recurrence"] = json!(["RRULE:FREQ=WEEKLY"]);
        let masters: Vec<bool> = [occurrence.clone(), mine(), turned.clone()]
            .iter()
            .map(|fresh| facts_from_fresh(fresh, &facts, EditingStatus::On).series_master)
            .collect();
        assert_eq!(masters, [false, false, true]);
        let f = facts_from_fresh(&turned, &facts, EditingStatus::On);
        assert_eq!(gate::edit_block(&f), Some(ReadOnlyReason::BecameRepeating));
        // An occurrence you organise edits like a one-off: every field and the delete are open, so
        // nothing in the gate stands between it and the dialog; which occurrences is asked at the
        // write (R9; C9b-1 critiques 1 L5, 2 L11).
        let f = facts_from_fresh(&occurrence, &facts, EditingStatus::On);
        let open = FieldPermissions {
            summary: true,
            time: true,
            location: true,
            description: true,
            show_as: true,
            visibility: true,
            delete: true,
            reasons: Vec::new(),
        };
        assert!(
            gate::edit_rights(&f) == open,
            "an occurrence edits like a one-off"
        );
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
            series_master: false,
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
        // How it repeats moves its times: a locked time locks it too (C11).
        let stop = EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
            ..Default::default()
        };
        assert_eq!(refused_field(&locked, &stop), Some(ReadOnlyReason::Locked));
        let open = FieldPermissions {
            time: true,
            ..locked.clone()
        };
        assert_eq!(refused_field(&open, &stop), None);
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
        // An occurrence of a repeating event you organise gets Edit and Delete: the dialog asks
        // which occurrences (R9).
        let occurrence = CalendarEvent {
            id: format!("{cal}:c_20261012T090000Z"),
            series_id: Some("c".into()),
            recurring: true,
            ..mine.clone()
        };
        assert_eq!(
            edit_blocks(&conn, &[mine, invite, orphan, occurrence]).unwrap(),
            vec![
                None,
                Some(ReadOnlyReason::NotOrganizer),
                Some(ReadOnlyReason::NotGoogle),
                None
            ]
        );
    }

    /// scope-L8: a delete of an occurrence with no choice is refused before it's held. The test is
    /// `row_repeats`, the same "is it repeating" as the webview's `isRepeating` (recurring || series
    /// id).
    #[test]
    fn a_series_row_needs_a_choice_before_its_delete_is_held() {
        let (_d, conn) = store();
        let cal = "gcal:me@x.com:me@x.com";
        add_calendar(&conn, "gcal:me@x.com", "google", cal, Some("me@x.com"));
        let one_off = CalendarEvent {
            id: format!("{cal}:a"),
            calendar_id: cal.into(),
            summary: "Mine".into(),
            start: "2026-10-12T09:00:00Z".into(),
            organizer_self: true,
            ..Default::default()
        };
        let occurrence = CalendarEvent {
            id: format!("{cal}:s_20261012T090000Z"),
            series_id: Some("s".into()),
            recurring: true,
            ..one_off.clone()
        };
        // A row read before series ids existed repeats by its flag alone.
        let flagged = CalendarEvent {
            id: format!("{cal}:f_20261012T090000Z"),
            recurring: true,
            ..one_off.clone()
        };
        let rows = [one_off.clone(), occurrence.clone(), flagged.clone()];
        calendar::replace_events(&conn, cal, &rows, true).unwrap();
        assert!(needs_scope(&conn, &occurrence.id).unwrap());
        assert!(needs_scope(&conn, &flagged.id).unwrap());
        assert!(!needs_scope(&conn, &one_off.id).unwrap());
        assert!(!needs_scope(&conn, &format!("{cal}:missing")).unwrap());
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

    /// The series, its listing by iCal UID, its occurrences in the mirror's window, and the parts PM
    /// split off it (none).
    fn series_reads(series: &Value, listed: Vec<Value>, window: Vec<Value>) -> Vec<Step> {
        vec![
            Step::get("/events/abc", "", series.clone()),
            Step::get("/events", "iCalUID=abc%40google.com", page(listed, None)),
            window_read(window),
            parts_read(Vec::new()),
        ]
    }

    /// The first request the script was sent that wasn't a read.
    fn first_write(script: &Script) -> RequestPlan {
        script
            .sent()
            .into_iter()
            .find(|r| r.method != Method::Get)
            .expect("a write went out")
    }

    /// The listing of the parts PM split off series "abc" (scope-M4).
    fn parts_read(parts: Vec<Value>) -> Step {
        Step::get(
            "/events",
            "privateExtendedProperty=pmSplitOf%3Dabc",
            page(parts, None),
        )
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
        let got = ask_on(
            &TestHost::holding(&[]),
            script,
            instance,
            request,
            opened,
            series,
        )
        .await;
        match got {
            SeriesChoices::Ready {
                this,
                following,
                all,
                notice: None,
            } => (this, following, all),
            other => panic!("{other:?}"),
        }
    }

    /// [`ask`] against `host`, its journal and its held splits: the whole answer.
    async fn ask_on(
        host: &TestHost,
        script: &Script,
        instance: &Value,
        request: ChoiceRequest,
        opened: Option<(&Value, Option<&Value>)>,
        series: &[&str],
    ) -> SeriesChoices {
        let got = choices(
            script,
            host,
            &target(instance),
            EditingStatus::On,
            &mirror(series),
            &request,
            opened,
        )
        .await;
        script.finished();
        got
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
            &*TestHost::holding(&[]),
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
        assert!(
            series_info(&instance, &series).is_some_and(|info| !info.first
                && info.repeats == "Weekly on Monday"
                && info.rule.is_some())
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

    /// C11 §11: the editor learns the series' rule as one of its own, the day it starts and the zone
    /// it repeats in, so the Repeat row starts from it.
    #[test]
    fn the_editor_gets_the_rule_its_first_day_and_zone() {
        use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern};
        let series = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"]);
        let info = series_info(&monday_19(), &series).unwrap();
        let rule = RepeatRule {
            pattern: RepeatPattern::Weekly {
                days: vec![DayCode::Mon],
            },
            interval: 1,
            ends: RepeatEnds::After { count: 10 },
        };
        assert!(info.rule == Some(rule), "the series' own rule");
        assert_eq!(info.starts, "2026-10-05");
        assert_eq!(info.zone.as_deref(), Some("Europe/London"));
        assert_eq!(info.repeats, "Weekly on Monday, 10 times");
        // Its start in its own zone, whatever offset Google wrote it with: 08:00 on the 6th in Tokyo
        // is still the 5th in New York and in UTC.
        let late = with(
            master(&["RRULE:FREQ=DAILY"]),
            "start",
            json!({ "dateTime": "2026-10-05T19:00:00-04:00", "timeZone": "Asia/Tokyo" }),
        );
        let late = with(
            late,
            "end",
            json!({ "dateTime": "2026-10-05T20:00:00-04:00", "timeZone": "Asia/Tokyo" }),
        );
        let info = series_info(&monday_19(), &late).unwrap();
        assert_eq!(info.starts, "2026-10-06");
        assert_eq!(info.zone.as_deref(), Some("Asia/Tokyo"));
    }

    /// A rule PM can only show gives its words and no rule: it can only be replaced.
    #[test]
    fn a_custom_rule_gives_its_words_and_no_rule() {
        let series = master(&["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"]);
        let info = series_info(&monday_19(), &series).unwrap();
        assert!(info.rule.is_none());
        assert_eq!(info.repeats, crate::calendar_recur::spec::CUSTOM_RULE);
    }

    /// An all-day series repeats by days, in no zone.
    #[test]
    fn an_all_day_series_names_no_zone() {
        let series = json!({
            "id": "abc", "etag": "\"m1\"", "recurrence": ["RRULE:FREQ=YEARLY"],
            "start": { "date": "2026-03-01" }, "end": { "date": "2026-03-02" },
            "organizer": { "self": true },
        });
        let occurrence = json!({
            "id": "abc_20270301", "recurringEventId": "abc",
            "originalStartTime": { "date": "2027-03-01" },
            "start": { "date": "2027-03-01" }, "end": { "date": "2027-03-02" },
        });
        let info = series_info(&occurrence, &series).unwrap();
        assert!(info.zone.is_none());
        assert_eq!(info.starts, "2026-03-01");
        assert!(info
            .rule
            .is_some_and(|r| r.pattern == crate::calendar_recur::wire::RepeatPattern::YearlyDate));
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
                parts_read(Vec::new()),
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
            &*TestHost::holding(&[]),
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
            &*TestHost::holding(&[]),
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
            &*TestHost::holding(&[]),
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
                &*TestHost::holding(&[]),
                &target(&event),
                EditingStatus::On,
                &mirror(&[]),
                &edit(title("Retro")),
                Some((&event, None)),
            )
            .await
        };
        assert_eq!(ask_one(one_off.clone()).await, SeriesChoices::NotRepeating);
        // A one-off turned into a series in Google: the gate refuses it, with its own reason, before
        // anything else is read.
        let now_a_series = with(one_off, "recurrence", json!(["RRULE:FREQ=DAILY"]));
        assert!(matches!(
            ask_one(now_a_series).await,
            SeriesChoices::Refused {
                result: WriteOutcome::ReadOnly {
                    reason: ReadOnlyReason::BecameRepeating
                }
            }
        ));
    }

    /// The C9a-3a hand-off: a delete's choices that meet an occurrence changed in Google land
    /// Google's copy, as the one-event delete does, so the next ask compares against it instead of
    /// conflicting again until a sync.
    #[tokio::test]
    async fn a_delete_choice_on_a_changed_occurrence_shows_googles_copy() {
        let (series, instance) = (weekly(), monday_19());
        let host = TestHost::holding(&october());
        let renamed = with(instance.clone(), "summary", json!("Standup moved"));
        let delete = |seen: SeenSummary| ChoiceRequest::Delete {
            event_id: "cal:abc_20261019T080000Z".into(),
            seen,
        };
        let script = Script::new(vec![instance_read(&renamed)]);
        let got = choices(
            &script,
            &*host,
            &target(&instance),
            EditingStatus::On,
            &mirror(&["abc"]),
            &delete(seen_by_user(&fresh_event(instance.clone()))),
            None,
        )
        .await;
        script.finished();
        assert!(matches!(
            &got,
            SeriesChoices::Refused { result: WriteOutcome::Conflict { fields } }
                if fields == &["summary"]
        ));
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:abc_20261019T080000Z"))
            .unwrap()
            .unwrap();
        assert_eq!(row.summary, "Standup moved");
        assert_eq!(host.told.lock().unwrap().len(), 1);
        // Asked again on what the user now sees (an occurrence changed on its own, so listed with
        // its series): every row offered.
        let script = Script::new(reads(
            &renamed,
            &series,
            vec![series.clone(), renamed.clone()],
            vec![renamed.clone()],
        ));
        let (this, following, all) = ask(
            &script,
            &renamed,
            delete(seen_by_user(&fresh_event(renamed.clone()))),
            None,
            &["abc"],
        )
        .await;
        for row in [this, following, all] {
            assert!(matches!(row, ScopeRow::Offered { .. }));
        }
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
            parts_read(Vec::new()),
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
            &*TestHost::holding(&[]),
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
                 guests yet. Choose “This event” to change only this one."
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
            Step::get(
                "/events",
                "privateExtendedProperty=pmSplitOf%3Dabc_R20261019T080000",
                page(Vec::new(), None),
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

    use crate::calendar_write::series::fixtures::{point, Case, TAIL};
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
        /// The account's editing status, never read from the keychain here.
        editing: Mutex<EditingStatus>,
        /// What a dev build's allow-list says of a settle.
        allowed: Mutex<std::result::Result<(), &'static str>>,
        /// The instant linked milestones are resolved at, its day read in the zone asked for.
        now: Mutex<chrono::DateTime<chrono::Utc>>,
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
                editing: Mutex::new(EditingStatus::On),
                allowed: Mutex::new(Ok(())),
                now: Mutex::new(noon("2026-10-12")),
                _dir: dir,
            })
        }

        /// Today is `day` (`YYYY-MM-DD`) in every zone these tests use (UTC, London, Honolulu).
        fn today_is(&self, day: &str) {
            *self.now.lock().unwrap() = noon(day);
        }

        /// It's `at` (RFC 3339): a test can put the user's day and the series' apart.
        fn now_is(&self, at: &str) {
            *self.now.lock().unwrap() = at.parse().unwrap();
        }

        /// The series' rows, in time order.
        fn rows(&self) -> Vec<CalendarEvent> {
            self.with_conn(|conn| calendar::series_rows(conn, "cal", &["abc"]))
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

        /// What the journal holds.
        fn journal(&self) -> journal::Journal {
            self.with_conn(stored_journal).unwrap()
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

        fn editing(&self, _target: &Target) -> Result<EditingStatus> {
            Ok(*self.editing.lock().unwrap())
        }

        fn settle_allowed(&self, _remote: &str) -> Result<std::result::Result<(), &'static str>> {
            Ok(*self.allowed.lock().unwrap())
        }

        fn today_in(&self, zone: chrono_tz::Tz) -> String {
            crate::clock::today_sql_in_at(zone, *self.now.lock().unwrap())
        }
    }

    /// Noon UTC on `day`: that day from Honolulu (02:00) to London (13:00).
    fn noon(day: &str) -> chrono::DateTime<chrono::Utc> {
        format!("{day}T12:00:00Z").parse().unwrap()
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
            milestone_moves: None,
        }
    }

    fn pending() -> WriteOutcome {
        WriteOutcome::Saved {
            warnings: vec![WriteWarning::MirrorRefreshPending],
            left_behind: Vec::new(),
            milestone_moves: None,
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
            new_series_id: || TAIL.to_string(),
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
        assert_eq!(
            first_write(&script).body,
            Some(json!({ "summary": "Retro" }))
        );
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
        assert_eq!(first_write(&script).body.unwrap()["status"], "cancelled");
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
        assert_eq!(
            first_write(&script).body,
            Some(json!({ "summary": "Retro" }))
        );
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
                milestone_moves: None,
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

    /// Scope-L2: "This event" edited, Google answering 2xx with the old title kept, says so as
    /// "All events" does; the row is as it was.
    #[tokio::test]
    async fn a_this_event_edit_google_ignored_says_so() {
        let host = TestHost::holding(&october());
        let instance = monday_19();
        let mut steps = october_reads();
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            instance["etag"].as_str().unwrap(),
            Scripted::Json(written(&instance, &[], "\"i2\"")),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &weekly(),
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert!(matches!(
            outcome,
            WriteOutcome::IgnoredByGoogle { split: false }
        ));
        assert!(host.rows().iter().all(|r| r.summary == "Standup"));
        assert!(host.nothing_recorded());
    }

    /// Scope-L2: "This event" deleted, Google answering 2xx to the cancel with the occurrence still
    /// confirmed, says so; the row stays.
    #[tokio::test]
    async fn a_this_event_delete_google_ignored_says_so() {
        let host = TestHost::holding(&october());
        let instance = monday_19();
        let mut steps = october_reads();
        steps.extend(october_reads());
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            instance["etag"].as_str().unwrap(),
            Scripted::Json(written(&instance, &[], "\"i2\"")),
        ));
        let script = Script::new(steps);
        let outcome = held_delete(&host, &script, &instance, Scope::This).await;
        script.asked_all();
        assert!(matches!(
            outcome,
            WriteOutcome::IgnoredByGoogle { split: false }
        ));
        assert!(host.ids().contains(&"cal:abc_20261019T080000Z".to_string()));
        assert!(host.nothing_recorded());
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

    // --- changing how it repeats (C11c) ---

    fn every_day() -> RepeatRule {
        use crate::calendar_recur::wire::{RepeatEnds, RepeatPattern};
        RepeatRule {
            pattern: RepeatPattern::Daily,
            interval: 1,
            ends: RepeatEnds::Never,
        }
    }

    /// A draft that makes it repeat by `rule`.
    fn repeating(rule: RepeatRule) -> EventPatchDraft {
        EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Rule { rule }),
            ..Default::default()
        }
    }

    fn on_tuesdays() -> EventPatchDraft {
        use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern};
        repeating(RepeatRule {
            pattern: RepeatPattern::Weekly {
                days: vec![DayCode::Tue],
            },
            interval: 1,
            ends: RepeatEnds::Never,
        })
    }

    /// "Does not repeat".
    fn stop_repeating() -> EventPatchDraft {
        EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
            ..Default::default()
        }
    }

    /// C11 §9.2: a new rule refuses "This event" in words and offers the rest, each with what it
    /// does: "this and following" starts again on Tuesday 20-10, every occurrence from Tuesday 06-10.
    #[tokio::test]
    async fn choices_for_a_rule_change_refuse_this_event_and_offer_the_rest() {
        let (series, instance) = (weekly(), monday_19());
        let script = Script::new(october_reads());
        let opened = Some((&instance, Some(&series)));
        let (this, following, all) =
            ask(&script, &instance, edit(on_tuesdays()), opened, &["abc"]).await;
        assert!(
            this == ScopeRow::Refused {
                message: "Changing how it repeats can't apply to just this occurrence.".into()
            },
            "\"This event\" is refused in words"
        );
        assert!(
            warnings(&following)
                == [
                    SeriesWarningDto::StartMoved {
                        from: "2026-10-19".into(),
                        to: "2026-10-20".into()
                    },
                    SeriesWarningDto::NewIdentity
                ],
            "the new part starts on Tuesday"
        );
        assert!(
            warnings(&all)
                == [SeriesWarningDto::StartMoved {
                    from: "2026-10-05".into(),
                    to: "2026-10-06".into()
                }],
            "every occurrence moves to Tuesdays"
        );
        // "Does not repeat": the first occurrence stays, and from this one on one event does.
        let script = Script::new(october_reads());
        let (this, following, all) =
            ask(&script, &instance, edit(stop_repeating()), opened, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Refused { .. }));
        assert!(
            warnings(&all)
                == [SeriesWarningDto::BecomesOneEvent {
                    day: "2026-10-05".into()
                }],
            "only the first stays"
        );
        assert!(
            warnings(&following)
                == [SeriesWarningDto::BecomesOneEvent {
                    day: "2026-10-19".into()
                }],
            "this one becomes the one event"
        );
    }

    /// C11 critique 1 H2, through the reads: a part Google split off from 02-11 (its id says so)
    /// stops a rule that would repeat past it.
    #[tokio::test]
    async fn a_new_rule_stops_short_of_a_later_part_google_made() {
        let (series, instance) = (weekly(), monday_19());
        let script = Script::new(october_reads());
        let opened = Some((&instance, Some(&series)));
        let (_, following, all) = ask(
            &script,
            &instance,
            edit(repeating(every_day())),
            opened,
            &["abc", "abc_R20261102T090000"],
        )
        .await;
        let past = ScopeRow::Refused {
            message: "PM can't make it repeat past 02-11-2026, where a later part of it starts. \
                      Pick an end date before then."
                .into(),
        };
        assert!(following == past && all == past, "both refused in words");
    }

    /// Made daily with this occurrence moved a day on, every occurrence says where it now starts,
    /// and nothing asks about Mondays becoming Tuesdays: a daily rule names no day.
    #[tokio::test]
    async fn a_daily_rule_with_a_moved_day_asks_nothing_of_its_days() {
        let (series, instance) = (weekly(), monday_19());
        let script = Script::new(october_reads());
        let daily_from_tuesday = EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Rule { rule: every_day() }),
            ..at("09:00", "2026-10-20")
        };
        let (_, _, all) = ask(
            &script,
            &instance,
            edit(daily_from_tuesday),
            Some((&instance, Some(&series))),
            &["abc"],
        )
        .await;
        assert!(
            warnings(&all)
                == [SeriesWarningDto::StartMoved {
                    from: "2026-10-05".into(),
                    to: "2026-10-06".into()
                }],
            "where it starts, and no question"
        );
    }

    /// The choice's token binds the rule: chosen for one, saved for another (whose plan is
    /// otherwise the same), it's asked again, with nothing but reads sent.
    #[tokio::test]
    async fn the_choice_token_binds_the_rule() {
        use crate::calendar_recur::wire::{RepeatEnds, RepeatPattern};
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let (session, open) = open_in(&host, &instance, &series);
        let request = ChoiceRequest::Edit {
            session: session.clone(),
            draft: repeating(every_day()),
        };
        let script = Script::new(october_reads());
        let (_, _, all) = ask(&script, &instance, request, Some(open.opened()), &["abc"]).await;
        let every_other_day = repeating(RepeatRule {
            pattern: RepeatPattern::Daily,
            interval: 2,
            ends: RepeatEnds::Never,
        });
        let script = Script::new(october_reads());
        let target = target(&instance);
        let outcome = save_series(
            &writer(&script, &host, &target),
            &session,
            open,
            every_other_day,
            Scope::All,
            Some(token(&all)),
        )
        .await;
        script.finished();
        assert!(matches!(outcome, WriteOutcome::NeedsChoice), "asked again");
    }

    /// C11 §9.7, critiques 1 M2 and 2 M1: "Does not repeat" for every occurrence lands Google's answer
    /// as the one event, recorded as created, and removes the old rows but those Google still holds
    /// live under the iCal UID; no instances listing goes out.
    #[tokio::test]
    async fn does_not_repeat_on_all_lands_one_event_and_removes_the_rest() {
        let host = TestHost::holding(&october());
        let (series, instance) = (weekly(), monday_19());
        let mut one_event = written(&series, &[], "\"m2\"");
        one_event.as_object_mut().unwrap().remove("recurrence");
        let planning = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Planning"));
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(one_event.clone())));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com showDeleted=true",
            Scripted::Json(page(
                vec![one_event, planning, cancelled("2026-10-12T09:00:00+01:00")],
                None,
            )),
        ));
        let script = Script::new(steps);
        let before = Instant::now();
        let (outcome, session) = save(
            &host,
            &script,
            &instance,
            &series,
            stop_repeating(),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert!(outcome == saved(), "saved, nothing pending");
        assert_eq!(
            first_write(&script).body,
            Some(json!({ "recurrence": null }))
        );
        // The one event is a row of its own; the old ones go but the one Google still shows.
        assert_eq!(host.row("abc").start, "2026-10-05T08:00:00Z");
        assert_eq!(host.ids(), ["cal:abc_20261026T090000Z"]);
        assert_eq!(host.told(), ["cal"]);
        assert!(open_session(&host.edit, &session).unwrap().is_none());
        // A whole sync that lags behind it, begun before the save or after, keeps the one event and
        // hides the rows that went.
        for started in [before, Instant::now()] {
            let shown = ids_of(host.synced(&october(), started));
            assert!(shown.contains(&"cal:abc".to_string()));
            assert!(!shown.contains(&"cal:abc_20261019T080000Z".to_string()));
            assert!(!shown.contains(&"cal:abc_20261012T080000Z".to_string()));
        }
    }

    /// The one event outside the months PM mirrors isn't landed, and the old rows still go; a
    /// listing that didn't run to its end removes none and leaves the rest to the next sync.
    #[tokio::test]
    async fn does_not_repeat_on_all_outside_the_mirror_or_unlisted() {
        let (series, instance) = (weekly(), monday_19());
        let mut long_ago = written(&series, &[], "\"m2\"");
        long_ago.as_object_mut().unwrap().remove("recurrence");
        long_ago["start"] =
            json!({ "dateTime": "2026-08-03T09:00:00+01:00", "timeZone": "Europe/London" });
        long_ago["end"] =
            json!({ "dateTime": "2026-08-03T10:00:00+01:00", "timeZone": "Europe/London" });
        let host = TestHost::holding(&october());
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(long_ago.clone())));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![long_ago.clone()], None)),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &series,
            stop_repeating(),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, .. }
                if *warnings == [WriteWarning::OutsideMirror]),
            "saved, outside the mirror"
        );
        assert!(host.ids().is_empty());
        assert!(host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:abc"))
            .unwrap()
            .is_none());
        // The listing cut short: nothing removed, the calendar to catch up.
        let host = TestHost::holding(&october());
        let mut one_event = written(&series, &[], "\"m2\"");
        one_event.as_object_mut().unwrap().remove("recurrence");
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(one_event.clone())));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![one_event], Some("next"))),
        ));
        steps.push(ical_listing("pageToken=next", Scripted::Status(500)));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &series,
            stop_repeating(),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert!(outcome == pending(), "saved, the calendar to catch up");
        assert_eq!(host.ids().len(), 3);
        assert_eq!(host.row("abc").start, "2026-10-05T08:00:00Z");
    }

    /// C11 §9.7: "Does not repeat" from this occurrence on lands the one event from the insert's own
    /// answer, never an instances listing of it, as created, beside the cut original.
    #[tokio::test]
    async fn does_not_repeat_on_following_lands_the_new_event_from_its_reply() {
        let host = TestHost::holding(&october());
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::Following, stop_repeating())
            .unwrap();
        let before = Instant::now();
        let mut steps = october_reads();
        steps.extend([
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
            refetch_read(Scripted::Json(page(vec![october()[0].clone()], None))),
        ]);
        let script = Script::new(steps);
        let (outcome, session) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            stop_repeating(),
            Scope::Following,
        )
        .await;
        script.asked_all();
        assert!(outcome == saved(), "saved");
        let inserted = first_write(&script).body.unwrap();
        assert!(inserted.get("recurrence").is_none());
        let (original, tail) = split_rows(&host);
        assert_eq!(original, ["cal:abc_20261012T080000Z"]);
        assert!(tail.is_empty(), "one event is no series' rows");
        assert_eq!(host.row(TAIL).start, "2026-10-19T08:00:00Z");
        assert!(open_session(&host.edit, &session).unwrap().is_none());
        assert_eq!(held_count(&host), 0);
        for started in [before, Instant::now()] {
            let lagging = ids_of(host.synced(&october(), started));
            assert!(lagging.contains(&format!("cal:{TAIL}")));
            assert!(!lagging.contains(&"cal:abc_20261026T090000Z".to_string()));
        }
    }

    /// The one event put past the months PM mirrors isn't landed, and says so; the original is cut
    /// as ever.
    #[tokio::test]
    async fn does_not_repeat_on_following_outside_the_mirror_says_so() {
        let host = TestHost::holding(&october());
        let far = EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
            ..at("09:00", "2027-12-06")
        };
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::Following, far.clone())
            .unwrap();
        let mut steps = october_reads();
        steps.extend([
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
            refetch_read(Scripted::Json(page(vec![october()[0].clone()], None))),
        ]);
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            far,
            Scope::Following,
        )
        .await;
        script.asked_all();
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, .. }
                if *warnings == [WriteWarning::OutsideMirror]),
            "saved, outside the mirror"
        );
        assert!(host
            .with_conn(|conn| calendar::event_by_id(conn, &format!("cal:{TAIL}")))
            .unwrap()
            .is_none());
        assert_eq!(split_rows(&host).0, ["cal:abc_20261012T080000Z"]);
    }

    /// C11 §9.7: a new rule for every occurrence gives each a new id, so the whole series is
    /// listed again and the old rows can't linger.
    #[tokio::test]
    async fn a_new_rule_on_all_relists_the_whole_series() {
        let host = TestHost::holding(&october());
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::All, on_tuesdays())
            .unwrap();
        let Op::Patch { body, .. } = &plan.steps[0].op else {
            panic!("not a patch");
        };
        let fields: Vec<(&str, Value)> =
            body.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let tuesdays = written(&weekly(), &fields, "\"m2\"");
        let occurrences: Vec<Value> = [
            "2026-10-13T09:00:00+01:00",
            "2026-10-20T09:00:00+01:00",
            "2026-10-27T09:00:00Z",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, start)| written(&plain(start), &[], &format!("\"n{i}\"")))
        .collect();
        let mut steps = october_reads();
        steps.push(series_patch(Scripted::Json(tuesdays)));
        steps.push(refetch_read(Scripted::Json(page(
            occurrences.clone(),
            None,
        ))));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            on_tuesdays(),
            Scope::All,
        )
        .await;
        script.asked_all();
        assert!(outcome == saved(), "saved");
        assert_eq!(host.ids(), ids_of(rows_of(&occurrences)));
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
                Step::get(
                    "/events",
                    "privateExtendedProperty=pmSplitOf%3Dxyz",
                    page(Vec::new(), None),
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

    /// The C9a-3a hand-off, for a scoped delete: before its hold and at its send, an occurrence
    /// changed in Google since the user saw it is a conflict that lands Google's copy, with nothing
    /// written.
    #[tokio::test]
    async fn a_held_series_delete_meeting_a_changed_occurrence_lands_it() {
        let instance = monday_19();
        let renamed = with(instance.clone(), "summary", json!("Standup moved"));
        let target = target(&instance);
        let seen = seen_by_user(&fresh_event(instance.clone()));
        let title_now = |host: &TestHost| {
            host.with_conn(|conn| calendar::event_by_id(conn, &target.row_id))
                .unwrap()
                .unwrap()
                .summary
        };
        let conflict = |outcome: &WriteOutcome| matches!(outcome, WriteOutcome::Conflict { fields } if fields == &["summary"]);
        // Before the hold.
        let host = TestHost::holding(&october());
        let script = Script::new(vec![instance_read(&renamed)]);
        let planned = plan_held_delete(
            &writer(&script, &host, &target),
            &target.row_id,
            &seen,
            Scope::All,
            None,
        )
        .await;
        script.finished();
        assert!(planned.err().as_ref().is_some_and(conflict));
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert_eq!(title_now(&host), "Standup moved");
        // At the send, after a hold planned on what the user saw.
        let host = TestHost::holding(&october());
        let script = Script::new(october_reads());
        let Ok((_claim, held)) = plan_held_delete(
            &writer(&script, &host, &target),
            &target.row_id,
            &seen,
            Scope::All,
            None,
        )
        .await
        else {
            panic!("the delete wasn't held");
        };
        script.finished();
        let script = Script::new(vec![instance_read(&renamed)]);
        let sent = send_held_series_delete(
            &writer(&script, &host, &target),
            &target.row_id,
            &seen,
            Scope::All,
            &held,
        )
        .await;
        script.finished();
        assert!(conflict(&sent));
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert_eq!(title_now(&host), "Standup moved");
    }

    // --- "this and following" and settling (C9a-3b) ---

    const TAIL_ICAL: &str = "tail@google.com";

    /// "This and following" retitled "Retro" from Monday 19-10, the series holding `exceptions`:
    /// the plan the send makes, its new series under [`TAIL`].
    fn split_plan(exceptions: Vec<Value>) -> SeriesPlan {
        Case::new(weekly(), monday_19())
            .exceptions(exceptions)
            .edit(Scope::Following, title("Retro"))
            .unwrap()
    }

    /// Google's copy of the new series `plan` inserts, at `etag`, updated at `updated`.
    fn tail_copy(plan: &SeriesPlan, etag: &str, updated: &str) -> Value {
        let Op::Insert { body, .. } = &plan.steps[0].op else {
            panic!("not a split");
        };
        let mut v = Value::Object(body.clone());
        v["kind"] = json!("calendar#event");
        v["id"] = json!(TAIL);
        v["etag"] = json!(etag);
        v["status"] = json!("confirmed");
        v["iCalUID"] = json!(TAIL_ICAL);
        v["updated"] = json!(updated);
        v
    }

    /// Google's series after `plan`'s cut, at "m2".
    fn cut_copy(plan: &SeriesPlan) -> Value {
        let Op::Patch { body, .. } = &plan.steps[1].op else {
            panic!("not a cut");
        };
        written(
            &weekly(),
            &[("recurrence", body["recurrence"].clone())],
            "\"m2\"",
        )
    }

    /// An occurrence of the new series at `start`, as Google lists it.
    fn tail_occurrence(start: &str, etag: &str) -> Value {
        let mut v = written(&plain(start), &[("summary", json!("Retro"))], etag);
        let id = v["id"].as_str().unwrap().replacen("abc", TAIL, 1);
        v["id"] = json!(id);
        v["recurringEventId"] = json!(TAIL);
        v
    }

    /// The new series' occurrences in the mirror's window, as the re-fetch after a split lists them.
    fn tail_october() -> Vec<Value> {
        vec![
            tail_occurrence("2026-10-19T09:00:00+01:00", "\"u0\""),
            tail_occurrence("2026-10-26T09:00:00Z", "\"u1\""),
        ]
    }

    /// The new series' POST: PM's id, nobody emailed, no Meet of Google's own.
    fn insert_step(answer: Scripted) -> Step {
        Step::new(
            Method::Insert,
            "/events",
            "sendUpdates=none conferenceDataVersion=1",
            answer,
        )
    }

    /// A GET of one event, answered `answer`.
    fn event_read(event: &str, answer: Scripted) -> Step {
        Step::new(Method::Get, &format!("/events/{event}"), "", answer)
    }

    fn tail_refetch(answer: Scripted) -> Step {
        Step::new(
            Method::Get,
            &format!("/events/{TAIL}/instances"),
            "timeMin=2026-09-01T00%3A00%3A00Z timeMax=2027-11-01T00%3A00%3A00Z maxResults=50",
            answer,
        )
    }

    /// A listing by iCal UID, answered `answer`.
    fn ical_listing(ical: &'static str, answer: Scripted) -> Step {
        Step::new(Method::Get, "/events", ical, answer)
    }

    fn lost() -> Scripted {
        Scripted::Lost { retried: false }
    }

    /// The original's rows, and the new series'.
    fn split_rows(host: &TestHost) -> (Vec<String>, Vec<String>) {
        let of = |series: &str| {
            ids_of(
                host.with_conn(|conn| calendar::series_rows(conn, "cal", &[series]))
                    .unwrap(),
            )
        };
        (of("abc"), of(TAIL))
    }

    /// Save the split from Monday 19-10, Google answering as `steps` say after the reads.
    async fn split_save(host: &TestHost, steps: Vec<Step>) -> (WriteOutcome, Script, String) {
        let mut all = october_reads();
        all.extend(steps);
        let script = Script::new(all);
        let (outcome, session) = save(
            host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.asked_all();
        (outcome, script, session)
    }

    /// The token of a held split, from the outcome that held it.
    fn held_token(outcome: &WriteOutcome) -> String {
        match outcome {
            WriteOutcome::PartialSplit { token, .. } => token.clone(),
            _ => panic!("not held"),
        }
    }

    fn offers_of(outcome: &WriteOutcome) -> Vec<SplitAction> {
        match outcome {
            WriteOutcome::PartialSplit { offers, .. } => offers.clone(),
            _ => Vec::new(),
        }
    }

    /// A settle's words while the change is still held: its note, `None` for any other answer.
    fn note_of(outcome: &WriteOutcome) -> Option<&str> {
        match outcome {
            WriteOutcome::PartialSplit { note, .. } => note.as_deref(),
            _ => None,
        }
    }

    /// The answer is the change still held under `token`, with what PM holds now and `words` as
    /// its note: the notice keeps its buttons (C9b-1 critiques 1 H2, 2 H1).
    fn still_held_with(host: &TestHost, outcome: &WriteOutcome, token: &str, words: &str) {
        let held = host.edit.held_split(token);
        assert!(held.is_some(), "the change is still held");
        let (offers, from) = held.unwrap();
        assert!(
            matches!(outcome, WriteOutcome::PartialSplit { token: t, offers: o, from: f, note: Some(n) }
                if t == token && *o == offers && *f == from && n == words),
            "the answer is the held change with the outcome's words"
        );
    }

    /// A split whose cut's answer was lost (and lost again on the look the run makes), then held
    /// with what `look`, the first look at the original, allows.
    async fn lost_cut(host: &TestHost, look: Scripted) -> (WriteOutcome, Script) {
        let plan = split_plan(Vec::new());
        let (outcome, script, _) = split_save(
            host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Lost { retried: true }),
                event_read("abc", lost()),
                event_read("abc", look),
            ],
        )
        .await;
        (outcome, script)
    }

    /// The settle command through `steps`.
    async fn settle(
        host: &TestHost,
        token: &str,
        action: SplitAction,
        steps: Vec<Step>,
    ) -> (WriteOutcome, Script) {
        let script = Script::new(steps);
        let outcome = settle_split(&script, host, token, action).await;
        script.asked_all();
        (outcome, script)
    }

    fn held_count(host: &TestHost) -> usize {
        host.edit.partials.lock().unwrap().len()
    }

    /// The new part, its listing by iCal UID holding `exceptions`: what Remove and Finish read of it.
    fn tail_reads(tail: Value, exceptions: Vec<Value>) -> Vec<Step> {
        let mut listed = vec![tail.clone()];
        listed.extend(exceptions);
        vec![
            event_read(TAIL, Scripted::Json(tail)),
            ical_listing(
                "iCalUID=tail%40google.com",
                Scripted::Json(page(listed, None)),
            ),
        ]
    }

    /// §5 (design `a_split_lands_tail_and_removals_in_one_write`): the new series goes in first,
    /// under PM's id, nobody emailed and no Meet; the cut next; both series are re-fetched and
    /// land in one write, the new rows recorded as created; the editor's session ends.
    #[tokio::test]
    async fn a_following_edit_inserts_cuts_and_lands_both_series() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let before = Instant::now();
        let (outcome, script, session) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Json(cut_copy(&plan))),
                refetch_read(Scripted::Json(page(vec![october()[0].clone()], None))),
                tail_refetch(Scripted::Json(page(tail_october(), None))),
            ],
        )
        .await;
        assert_eq!(outcome, saved());
        let insert = first_write(&script);
        let body = insert.body.as_ref().unwrap();
        assert_eq!(body["extendedProperties"]["private"]["pmSplitOf"], "abc");
        assert!(body["extendedProperties"]["private"]["pmSplitPlan"].is_string());
        assert!(body.get("conferenceData").is_none());
        assert_eq!(body["id"], TAIL);
        let (original, tail) = split_rows(&host);
        assert_eq!(original, ["cal:abc_20261012T080000Z"]);
        assert_eq!(tail, ids_of(rows_of(&tail_october())));
        assert!(open_session(&host.edit, &session).unwrap().is_none());
        assert_eq!(held_count(&host), 0);
        // A whole sync that lags behind the split keeps the new part and hides the cut-off rows.
        let lagging = sorted(ids_of(host.synced(&october(), before)));
        let mut want = vec!["cal:abc_20261012T080000Z".to_string()];
        want.extend(ids_of(rows_of(&tail_october())));
        assert_eq!(lagging, sorted(want));
    }

    /// The design's floors: the original's removed rows are floored at the cut's answer, never the
    /// insert's, so a row changed in Google after the cut comes back.
    #[tokio::test]
    async fn a_split_floors_the_originals_rows_at_the_cuts_reply() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(
                    &plan,
                    "\"t1\"",
                    "2026-10-17T12:00:05.000Z",
                ))),
                series_patch(Scripted::Json(cut_copy(&plan))),
                refetch_read(Scripted::Json(page(vec![october()[0].clone()], None))),
                tail_refetch(Scripted::Json(page(tail_october(), None))),
            ],
        )
        .await;
        assert_eq!(outcome, saved());
        let after_the_cut = with(
            written(&october()[2], &[], "\"o9\""),
            "updated",
            json!("2026-10-17T12:00:03.000Z"),
        );
        let old = "cal:abc_20261026T090000Z".to_string();
        assert!(ids_of(host.synced(&[after_the_cut], Instant::now())).contains(&old));
    }

    /// A14 for a split: a new part the re-fetch doesn't show yet is looked for after 1 s and 3 s;
    /// still not, the original's rows stay (DS18), nothing is recorded, and the save says so.
    #[tokio::test]
    async fn a_stale_split_keeps_the_originals_rows() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let mut steps = vec![
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
        ];
        for _ in 0..3 {
            steps.push(refetch_read(Scripted::Json(page(
                vec![october()[0].clone()],
                None,
            ))));
            steps.push(tail_refetch(Scripted::Json(page(Vec::new(), None))));
        }
        let (outcome, script, _) = split_save(&host, steps).await;
        assert_eq!(outcome, pending());
        assert_eq!(script.waits(), REFETCH_AGAIN.to_vec());
        assert_eq!(split_rows(&host).0, ids_of(rows_of(&october())));
        assert!(host.nothing_recorded());
        assert!(host.hash_dropped());
    }

    /// Review M8 for a split: a new part whose listing stops short is pending, never half landed.
    #[tokio::test]
    async fn an_incomplete_new_part_listing_is_pending_never_half_landed() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let mut steps = vec![
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
        ];
        for _ in 0..3 {
            steps.push(refetch_read(Scripted::Json(page(
                vec![october()[0].clone()],
                None,
            ))));
            steps.push(tail_refetch(Scripted::Json(page(
                tail_october(),
                Some("p2"),
            ))));
            steps.push(tail_refetch(Scripted::Status(500)));
        }
        let (outcome, _, _) = split_save(&host, steps).await;
        assert_eq!(outcome, pending());
        assert_eq!(split_rows(&host), (ids_of(rows_of(&october())), Vec::new()));
        assert!(host.nothing_recorded());
    }

    /// L-R5: Google answers the cut 2xx and keeps the old rule. The new part is deleted, on its
    /// etag, emailing nobody, and the save says Google ignored it; nothing is held, and the new
    /// part's rows a sync wrote meanwhile go too (critique L2).
    #[tokio::test]
    async fn a_cut_google_ignored_removes_the_new_part_and_says_so() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let syncing = Arc::clone(&host);
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Json(written(&weekly(), &[], "\"m2\""))).then(move || {
                    let mut rows = rows_of(&october());
                    rows.extend(rows_of(&tail_october()));
                    syncing
                        .with_conn(|conn| calendar::replace_events(conn, "cal", &rows, true))
                        .unwrap();
                }),
                Step::write(Method::Delete, &format!("/events/{TAIL}"), "\"t1\"", {
                    Scripted::Status(204)
                }),
            ],
        )
        .await;
        assert_eq!(outcome, WriteOutcome::IgnoredByGoogle { split: true });
        assert_eq!(split_rows(&host), (ids_of(rows_of(&october())), Vec::new()));
        assert_eq!(held_count(&host), 0);
    }

    /// C11: the one event "Does not repeat" from this occurrence on made is a row of its own; a
    /// sync that wrote it meanwhile doesn't leave it behind when the change is undone.
    #[tokio::test]
    async fn a_removed_one_event_takes_the_row_a_sync_wrote() {
        let host = TestHost::holding(&october());
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::Following, stop_repeating())
            .unwrap();
        let one_event = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let syncing = Arc::clone(&host);
        let synced = one_event.clone();
        let mut steps = october_reads();
        steps.extend([
            insert_step(Scripted::Json(one_event)),
            series_patch(Scripted::Json(written(&weekly(), &[], "\"m2\""))).then(move || {
                let mut rows = rows_of(&october());
                rows.extend(rows_of(std::slice::from_ref(&synced)));
                syncing
                    .with_conn(|conn| calendar::replace_events(conn, "cal", &rows, true))
                    .unwrap();
            }),
            Step::write(Method::Delete, &format!("/events/{TAIL}"), "\"t1\"", {
                Scripted::Status(204)
            }),
        ]);
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            stop_repeating(),
            Scope::Following,
        )
        .await;
        script.asked_all();
        assert!(
            matches!(outcome, WriteOutcome::IgnoredByGoogle { split: true }),
            "the cut was ignored, the one event removed"
        );
        assert!(host
            .with_conn(|conn| calendar::event_by_id(conn, &format!("cal:{TAIL}")))
            .unwrap()
            .is_none());
        assert_eq!(host.ids(), ids_of(rows_of(&october())));
    }

    /// C9a-3b critique M9: the new series is claimed once planned, so a write to it meanwhile (or
    /// this split, when one already holds it) is Busy, with only reads sent.
    #[tokio::test]
    async fn a_split_claims_its_new_series_too() {
        let host = TestHost::holding(&october());
        let _other = InFlight::claim(&host.edit, &series_key(CALENDAR, TAIL)).unwrap();
        let (outcome, script, _) = split_save(&host, Vec::new()).await;
        script.finished();
        assert_eq!(outcome, WriteOutcome::Busy);
    }

    /// C9a-3a critique L6: a 404 to the first POST is the calendar, said so; nothing else goes.
    #[tokio::test]
    async fn a_404_to_the_insert_says_the_calendar_isnt_there() {
        let host = TestHost::holding(&october());
        let (outcome, script, _) =
            split_save(&host, vec![insert_step(Scripted::Status(404))]).await;
        assert_eq!(
            outcome,
            WriteOutcome::Failed {
                message: NO_CALENDAR.into()
            }
        );
        assert_eq!(script.sent().len(), october_reads().len() + 1);
        assert_eq!(held_count(&host), 0);
    }

    #[tokio::test]
    async fn an_unsent_insert_sends_nothing_more() {
        for (why, want) in [
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
            let (outcome, _, _) = split_save(&host, vec![insert_step(Scripted::Unsent(why))]).await;
            assert_eq!(outcome, want);
            assert_eq!(held_count(&host), 0);
        }
    }

    /// §5.3: a cut whose answer is lost is looked at once more, then held with what Google's copy
    /// allows: done → Finish only; uncut at a newer etag → Remove; uncut at the cut's own etag →
    /// Remove or Check; no answer → Check only. The hash is dropped and the editor's session ends.
    #[tokio::test]
    async fn a_lost_cut_is_looked_at_and_held_with_what_google_allows() {
        use SplitAction::{Check, Finish, Remove};
        let plan = split_plan(Vec::new());
        for (look, want) in [
            (Scripted::Json(cut_copy(&plan)), vec![Finish]),
            (
                Scripted::Json(written(&weekly(), &[], "\"m7\"")),
                vec![Remove],
            ),
            (Scripted::Json(weekly()), vec![Remove, Check]),
            (lost(), vec![Check]),
        ] {
            let host = TestHost::holding(&october());
            let (outcome, _) = lost_cut(&host, look).await;
            assert_eq!(offers_of(&outcome), want);
            assert!(
                matches!(&outcome, WriteOutcome::PartialSplit { from, .. } if from == "2026-10-19")
            );
            assert_eq!(held_count(&host), 1);
            assert!(host.hash_dropped());
            assert!(host.edit.sessions.lock().unwrap().is_empty());
        }
    }

    /// C9a-3b critique L5: the look finds the original deleted in Google: said so, its rows go,
    /// nothing is held.
    #[tokio::test]
    async fn a_lost_cut_whose_original_is_gone_says_so() {
        // A 404, or the cancelled copy Google keeps of a deleted event (review round 1).
        let deleted = written(&with(weekly(), "status", json!("cancelled")), &[], "\"m8\"");
        for look in [Scripted::Status(404), Scripted::Json(deleted)] {
            let host = TestHost::holding(&october());
            let (outcome, _) = lost_cut(&host, look).await;
            assert!(
                matches!(&outcome, WriteOutcome::Failed { message } if message.contains("deleted in Google"))
            );
            assert_eq!(held_count(&host), 0);
            assert!(split_rows(&host).0.is_empty());
        }
    }

    /// Review round 1: a cut answered 404 (the series deleted in Google) whose rollback couldn't be
    /// proved, the look then finding the new part gone too: said gone, and the deleted series' rows
    /// leave the mirror.
    #[tokio::test]
    async fn a_cut_meeting_its_series_deleted_lands_it_once_the_new_part_is_gone() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Status(404)),
                Step::write(
                    Method::Delete,
                    &format!("/events/{TAIL}"),
                    "\"t1\"",
                    Scripted::Status(403),
                ),
                event_read(TAIL, Scripted::Status(404)),
            ],
        )
        .await;
        assert_eq!(outcome, WriteOutcome::Gone);
        assert_eq!(held_count(&host), 0);
        assert!(split_rows(&host).0.is_empty());
    }

    /// Review round 1: a split with an occurrence changed on its own after it carries that change
    /// onto the new part's same occurrence, then cancels the old copy; both series land, the old
    /// copy's row goes.
    #[tokio::test]
    async fn a_split_carries_and_cancels_what_changed_on_its_own_and_lands_both() {
        let (events, mut steps) = with_off_site();
        let host = TestHost::holding(&events);
        let plan = split_plan(vec![off_site()]);
        let moved = tail_occurrence("2026-11-02T09:00:00Z", "\"q1\"");
        let carried = with(written(&moved, &[], "\"q2\""), "summary", json!("Off-site"));
        let cancelled_copy = written(
            &with(off_site(), "status", json!("cancelled")),
            &[],
            "\"c2\"",
        );
        let mut listed = tail_october();
        listed.push(carried.clone());
        steps.extend([
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
            Step::get(
                &format!("/events/{TAIL}/instances"),
                "timeMin=2026-11-01T09%3A00%3A00Z timeMax=2026-11-03T09%3A00%3A00Z maxResults=10",
                page(vec![moved], None),
            ),
            Step::write(
                Method::Patch,
                &format!("/events/{TAIL}_20261102T090000Z"),
                "\"q1\"",
                Scripted::Json(carried.clone()),
            ),
            Step::write(
                Method::Patch,
                "/events/abc_20261102T090000Z",
                off_site()["etag"].as_str().unwrap(),
                Scripted::Json(cancelled_copy.clone()),
            ),
            refetch_read(Scripted::Json(page(
                vec![october()[0].clone(), cancelled_copy],
                None,
            ))),
            tail_refetch(Scripted::Json(page(listed.clone(), None))),
        ]);
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
        let sent = script.sent();
        let carry = sent
            .iter()
            .find(|r| r.url.contains("_20261102T090000Z?") && r.url.contains(TAIL))
            .unwrap();
        assert_eq!(carry.body, Some(json!({ "summary": "Off-site" })));
        let (original, tail) = split_rows(&host);
        assert_eq!(original, ["cal:abc_20261012T080000Z"]);
        assert_eq!(sorted(tail), sorted(ids_of(rows_of(&listed))));
    }

    /// M4, corner c: a sync that mirrors the new part while the split is on its way; its rows are
    /// still recorded as created (none were in the rows read before the first send), so a sync that
    /// lags behind the split keeps them.
    #[tokio::test]
    async fn a_sync_mid_split_still_leaves_the_new_part_created() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let syncing = Arc::clone(&host);
        let before = Instant::now();
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Json(cut_copy(&plan))).then(move || {
                    let mut rows = rows_of(&october());
                    rows.extend(rows_of(&tail_october()));
                    // A row of the new part Google no longer lists by the time PM re-fetches it.
                    rows.extend(rows_of(&[stale_tail_row()]));
                    syncing
                        .with_conn(|conn| calendar::replace_events(conn, "cal", &rows, true))
                        .unwrap();
                }),
                refetch_read(Scripted::Json(page(vec![october()[0].clone()], None))),
                tail_refetch(Scripted::Json(page(tail_october(), None))),
            ],
        )
        .await;
        assert_eq!(outcome, saved());
        let lagging = ids_of(host.synced(&october(), before));
        for id in ids_of(rows_of(&tail_october())) {
            assert!(lagging.contains(&id), "{id}");
        }
        // The landing read the new part's rows too, so the one Google no longer lists went.
        assert_eq!(split_rows(&host).1, ids_of(rows_of(&tail_october())));
    }

    /// A row of the new part that a sync wrote and the re-fetch no longer lists.
    fn stale_tail_row() -> Value {
        tail_occurrence("2026-10-28T09:00:00Z", "\"u9\"")
    }

    /// Review round 2: the claims last the whole send (the new part's too) and the whole settle
    /// (both series), not just their first statement.
    #[tokio::test]
    async fn claims_last_the_whole_send_and_settle() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let held_then = |host: &Arc<TestHost>| {
            let host = Arc::clone(host);
            let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = Arc::clone(&seen);
            let check = move || {
                let flight = host.edit.in_flight.lock().unwrap();
                let both = [series_key(CALENDAR, "abc"), series_key(CALENDAR, TAIL)];
                flag.store(
                    both.iter().all(|k| flight.contains(k)),
                    std::sync::atomic::Ordering::SeqCst,
                );
            };
            (seen, check)
        };
        let (during_send, check) = held_then(&host);
        let (held, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Lost { retried: true }).then(check),
                event_read("abc", lost()),
                event_read("abc", lost()),
            ],
        )
        .await;
        assert!(during_send.load(std::sync::atomic::Ordering::SeqCst));
        let (during_settle, check) = held_then(&host);
        let (_, script) = settle(
            &host,
            &held_token(&held),
            SplitAction::Check,
            vec![event_read("abc", lost()).then(check)],
        )
        .await;
        script.finished();
        assert!(during_settle.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Review round 2: a cut answered 404 whose rollback deleted the new part: gone, and the deleted
    /// series' rows leave the mirror at once.
    #[tokio::test]
    async fn a_cut_meeting_its_series_deleted_lands_it() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Status(404)),
                Step::write(
                    Method::Delete,
                    &format!("/events/{TAIL}"),
                    "\"t1\"",
                    Scripted::Status(204),
                ),
            ],
        )
        .await;
        assert_eq!(outcome, WriteOutcome::Gone);
        assert!(split_rows(&host).0.is_empty());
    }

    /// Review round 2: settling a split whose cut met its series deleted: Remove takes the new part,
    /// and the answer is that the event is gone, its rows with it, never "as it was".
    #[tokio::test]
    async fn removing_the_new_part_of_a_deleted_series_says_gone() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (held, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail.clone())),
                series_patch(Scripted::Status(404)),
                Step::write(
                    Method::Delete,
                    &format!("/events/{TAIL}"),
                    "\"t1\"",
                    Scripted::Status(403),
                ),
                event_read(TAIL, Scripted::Json(tail.clone())),
            ],
        )
        .await;
        assert_eq!(offers_of(&held), [SplitAction::Remove]);
        let mut steps = vec![event_read(TAIL, Scripted::Json(tail.clone()))];
        steps.extend(tail_reads(tail.clone(), Vec::new()));
        steps.push(event_read(TAIL, Scripted::Json(tail)));
        steps.push(Step::write(
            Method::Delete,
            &format!("/events/{TAIL}"),
            "\"t1\"",
            Scripted::Status(204),
        ));
        let (outcome, _) = settle(&host, &held_token(&held), SplitAction::Remove, steps).await;
        assert_eq!(outcome, WriteOutcome::Gone);
        assert_eq!(split_rows(&host), (Vec::new(), Vec::new()));
        assert_eq!(held_count(&host), 0);
    }

    /// Review round 2: a settle's look that finds the original deleted in Google says so, lands it,
    /// and holds nothing.
    #[tokio::test]
    async fn a_settle_that_finds_the_original_gone_says_so() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let (outcome, _) = settle(
            &host,
            &held_token(&held),
            SplitAction::Check,
            vec![event_read("abc", Scripted::Status(404))],
        )
        .await;
        assert!(
            matches!(&outcome, WriteOutcome::Failed { message } if message.contains("deleted in Google"))
        );
        assert!(split_rows(&host).0.is_empty());
        assert_eq!(held_count(&host), 0);
    }

    /// Review rounds 1 and 2: after a lost insert, a new part renamed in Google, or given a place it
    /// never had, before PM's first look, is never taken for "as PM made it": neither Remove nor
    /// Finish acts on it.
    #[tokio::test]
    async fn a_new_part_changed_before_the_first_look_is_never_acted_on() {
        let plan = split_plan(Vec::new());
        let changes = [
            with(
                tail_copy(&plan, "\"t9\"", SAVED_AT),
                "summary",
                json!("Renamed in Google"),
            ),
            with(
                tail_copy(&plan, "\"t9\"", SAVED_AT),
                "location",
                json!("Room 4"),
            ),
        ];
        for changed in changes {
            for action in [SplitAction::Remove, SplitAction::Finish] {
                let host = TestHost::holding(&october());
                let (held, _, _) = split_save(
                    &host,
                    vec![
                        insert_step(lost()),
                        event_read(TAIL, lost()),
                        event_read(TAIL, Scripted::Json(changed.clone())),
                    ],
                )
                .await;
                assert_eq!(offers_of(&held), [SplitAction::Finish, SplitAction::Remove]);
                let (outcome, script) = settle(
                    &host,
                    &held_token(&held),
                    action,
                    vec![
                        event_read(TAIL, Scripted::Json(changed.clone())),
                        event_read(TAIL, Scripted::Json(changed.clone())),
                    ],
                )
                .await;
                script.finished();
                // Still held, so the answer keeps its buttons (Check) and says why.
                assert!(
                    matches!(&outcome, WriteOutcome::PartialSplit { offers, note: Some(message), .. }
                        if message.contains("changed in Google") && offers == &[SplitAction::Check]),
                    "{action:?}"
                );
                assert_eq!(
                    host.edit.held_on(CALENDAR, "abc").unwrap().offers,
                    [SplitAction::Check]
                );
                // Still changed on the next look: Check only, whichever action found it (M7).
                let (again, _) = settle(
                    &host,
                    &held_token(&held),
                    SplitAction::Check,
                    vec![
                        event_read(TAIL, Scripted::Json(changed.clone())),
                        event_read(TAIL, Scripted::Json(changed.clone())),
                    ],
                )
                .await;
                assert_eq!(offers_of(&again), [SplitAction::Check], "{action:?}");
            }
        }
    }

    /// Review round 3: a split held after its rollback couldn't delete the new part, then the new
    /// part deleted in Google: a settle's first look settles it. The original cut (the cut ignored)
    /// says the part was removed; the original deleted in Google (the cut answered 404) says gone and
    /// lands it. Either way the new part's rows go and nothing stays held.
    #[tokio::test]
    async fn a_settle_whose_look_finds_the_new_part_gone_settles_it() {
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        for (cut, want, original_rows) in [
            (
                Scripted::Json(written(&weekly(), &[], "\"m2\"")),
                WriteOutcome::SplitRemoved,
                ids_of(rows_of(&october())),
            ),
            (Scripted::Status(404), WriteOutcome::Gone, Vec::new()),
        ] {
            for action in [SplitAction::Check, SplitAction::Remove] {
                let host = TestHost::holding(&october());
                let (held, _, _) = split_save(
                    &host,
                    vec![
                        insert_step(Scripted::Json(tail.clone())),
                        series_patch(match &cut {
                            Scripted::Json(v) => Scripted::Json(v.clone()),
                            _ => Scripted::Status(404),
                        }),
                        Step::write(
                            Method::Delete,
                            &format!("/events/{TAIL}"),
                            "\"t1\"",
                            Scripted::Status(403),
                        ),
                        event_read(TAIL, Scripted::Json(tail.clone())),
                    ],
                )
                .await;
                assert_eq!(offers_of(&held), [SplitAction::Remove]);
                // A sync mirrored the new part meanwhile.
                host.with_conn(|conn| {
                    let mut rows = rows_of(&october());
                    rows.extend(rows_of(&tail_october()));
                    calendar::replace_events(conn, "cal", &rows, true)
                })
                .unwrap();
                let (outcome, script) = settle(
                    &host,
                    &held_token(&held),
                    action,
                    vec![event_read(TAIL, Scripted::Status(404))],
                )
                .await;
                script.finished();
                assert_eq!(outcome, want, "{action:?}");
                assert_eq!(split_rows(&host), (original_rows.clone(), Vec::new()));
                assert_eq!(held_count(&host), 0);
            }
        }
    }

    /// Review round 3: what Remove and Finish can't read leaves the split as it was, held, with
    /// nothing sent: the new part unread, or the original's occurrences listed short.
    #[tokio::test]
    async fn what_remove_or_finish_cant_read_sends_nothing() {
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        // Remove, the new part unread.
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, Scripted::Json(written(&weekly(), &[], "\"m7\""))).await;
        let (outcome, script) = settle(
            &host,
            &held_token(&held),
            SplitAction::Remove,
            vec![
                event_read("abc", Scripted::Json(written(&weekly(), &[], "\"m7\""))),
                event_read(TAIL, Scripted::Status(500)),
            ],
        )
        .await;
        script.finished();
        assert_eq!(offers_of(&outcome), [SplitAction::Remove]);
        assert_eq!(held_count(&host), 1);
        // Finish, the new part unread; then the original's occurrences listed short. Still held,
        // with Finish's words, so its buttons stay.
        let mut short = tail_reads(tail.clone(), Vec::new());
        short.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![cut_copy(&plan)], Some("p2"))),
        ));
        short.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Status(500),
        ));
        for reads in [vec![event_read(TAIL, Scripted::Status(500))], short] {
            let host = TestHost::holding(&october());
            let (held, _) = lost_cut(&host, lost()).await;
            let mut steps = vec![event_read("abc", Scripted::Json(cut_copy(&plan)))];
            steps.extend(reads);
            let (outcome, script) =
                settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
            script.finished();
            still_held_with(&host, &outcome, &held_token(&held), FINISH_UNREAD);
            assert_eq!(held_count(&host), 1);
        }
    }

    /// Review round 2: a new part found changed, then deleted in Google by the user: Remove comes
    /// back, and finds it gone.
    #[tokio::test]
    async fn a_changed_new_part_deleted_in_google_gives_remove_back() {
        let plan = split_plan(Vec::new());
        let renamed = with(
            tail_copy(&plan, "\"t2\"", SAVED_AT),
            "summary",
            json!("Renamed"),
        );
        let uncut = || event_read("abc", Scripted::Json(written(&weekly(), &[], "\"m7\"")));
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, Scripted::Json(written(&weekly(), &[], "\"m7\""))).await;
        let token = held_token(&held);
        let (refused, _) = settle(
            &host,
            &token,
            SplitAction::Remove,
            vec![uncut(), event_read(TAIL, Scripted::Json(renamed))],
        )
        .await;
        let from = host.edit.held_split(&token).map(|(_, from)| from);
        let words = tail_changed_words(from.as_deref().unwrap_or_default());
        still_held_with(&host, &refused, &token, &words);
        assert_eq!(offers_of(&refused), [SplitAction::Check]);
        let (back, _) = settle(
            &host,
            &token,
            SplitAction::Check,
            vec![uncut(), event_read(TAIL, Scripted::Status(404))],
        )
        .await;
        assert_eq!(offers_of(&back), [SplitAction::Remove]);
    }

    /// Review round 1 (high): after a lost insert no copy of the new part stands for "as PM made
    /// it" but the insert's own answer. A later look that finds it renamed in Google is never taken
    /// for it, so Remove refuses and deletes nothing.
    #[tokio::test]
    async fn a_new_part_renamed_after_a_lost_insert_is_never_removed() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let mut steps = vec![insert_step(lost()), event_read(TAIL, lost())];
        steps.extend((0..3).map(|_| event_read(TAIL, Scripted::Status(404))));
        let (held, _, _) = split_save(&host, steps).await;
        assert_eq!(offers_of(&held), [SplitAction::Remove, SplitAction::Check]);
        let renamed = with(
            tail_copy(&plan, "\"t9\"", SAVED_AT),
            "summary",
            json!("Renamed in Google"),
        );
        let (outcome, script) = settle(
            &host,
            &held_token(&held),
            SplitAction::Remove,
            vec![
                event_read(TAIL, Scripted::Json(renamed.clone())),
                event_read(TAIL, Scripted::Json(renamed)),
            ],
        )
        .await;
        script.finished();
        assert!(note_of(&outcome).is_some_and(|n| n.contains("changed in Google")));
        assert_eq!(offers_of(&outcome), [SplitAction::Check]);
        assert_eq!(held_count(&host), 1);
    }

    /// The guard: a split taken out to settle goes back under its token when the settle stops,
    /// unless it was settled.
    #[test]
    fn a_taken_split_goes_back_unless_settled() {
        let edit = CalendarEditState::default();
        let plan = split_plan(Vec::new());
        let held = HeldSplit {
            target: target(&monday_19()),
            plan: plan.clone(),
            ran: Ran::Unknown {
                step: 0,
                sent_on: Vec::new(),
                ends: vec![End::NotRun, End::NotRun],
            },
            facts: SplitFacts {
                master: "abc".into(),
                tail: TAIL.into(),
                insert: 0,
                at: point("2026-10-19T09:00:00+01:00"),
                zone: chrono_tz::Europe::London,
                from: "2026-10-19".into(),
                moments: Vec::new(),
                marks: Vec::new(),
                ical: "abc@google.com".into(),
                movable: None,
            },
            offers: vec![SplitAction::Check],
            tail_seen: None,
            tail_changed: false,
        };
        edit.hold_partial("t".into(), held);
        {
            let (mut taken, mut copy) = TakenSplit::take(&edit, "t").unwrap();
            assert!(edit.held_on(CALENDAR, "abc").is_none());
            assert!(TakenSplit::take(&edit, "t").is_none());
            copy.offers = vec![SplitAction::Remove];
            taken.keep(&copy);
        }
        assert_eq!(
            edit.held_on(CALENDAR, "abc").unwrap().offers,
            [SplitAction::Remove]
        );
        {
            let (mut taken, _) = TakenSplit::take(&edit, "t").unwrap();
            taken.settled();
        }
        assert!(edit.held_on(CALENDAR, "abc").is_none());
    }

    /// §5.3: a lost insert is held with Finish or Remove when the new part is there, and with
    /// Remove or Check when three looks don't find it.
    #[tokio::test]
    async fn a_lost_insert_is_held_with_finish_or_remove_when_the_new_part_is_there() {
        use SplitAction::{Check, Finish, Remove};
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        // The run: POST lost, GET lost (unknown); then the look.
        let run = || vec![insert_step(lost()), event_read(TAIL, lost())];
        let host = TestHost::holding(&october());
        let mut steps = run();
        steps.push(event_read(TAIL, Scripted::Json(tail.clone())));
        let (outcome, _, _) = split_save(&host, steps).await;
        assert_eq!(offers_of(&outcome), [Finish, Remove]);

        let host = TestHost::holding(&october());
        let mut steps = run();
        steps.extend((0..3).map(|_| event_read(TAIL, Scripted::Status(404))));
        let (outcome, script, _) = split_save(&host, steps).await;
        assert_eq!(offers_of(&outcome), [Remove, Check]);
        assert_eq!(script.waits(), REFETCH_AGAIN.to_vec());
    }

    /// Deviation 3: a cut proved not done whose rollback couldn't delete the new part is looked at
    /// first: gone after all, it's the run's own result and nothing is held; still there, Remove.
    #[tokio::test]
    async fn a_rollback_that_couldnt_delete_the_new_part_is_looked_at_first() {
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let run = || {
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Json(written(&weekly(), &[], "\"m2\""))),
                // The rollback's delete refused: unknown.
                Step::write(
                    Method::Delete,
                    &format!("/events/{TAIL}"),
                    "\"t1\"",
                    Scripted::Status(403),
                ),
            ]
        };
        let host = TestHost::holding(&october());
        let mut steps = run();
        steps.push(event_read(TAIL, Scripted::Status(404)));
        let (outcome, _, _) = split_save(&host, steps).await;
        assert_eq!(outcome, WriteOutcome::IgnoredByGoogle { split: true });
        assert_eq!(held_count(&host), 0);

        let host = TestHost::holding(&october());
        let mut steps = run();
        steps.push(event_read(TAIL, Scripted::Json(tail)));
        let (outcome, _, _) = split_save(&host, steps).await;
        assert_eq!(offers_of(&outcome), [SplitAction::Remove]);
    }

    /// Review M1: while a split is held, every scope on the original but "this event" before the
    /// split answers it, with only reads sent; "this event" before the split goes ahead.
    #[tokio::test]
    async fn a_held_split_blocks_this_and_following_and_all_but_not_this_before_it() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let token = held_token(&held);
        for scope in [Scope::Following, Scope::All, Scope::This] {
            let script = Script::new(october_reads());
            let (outcome, _) = save(
                &host,
                &script,
                &monday_19(),
                &weekly(),
                title("Again"),
                scope,
            )
            .await;
            script.finished();
            assert_eq!(held_token(&outcome), token, "{scope:?}");
        }
        // The 12th, before the split: "this event" plans and saves.
        let twelfth = october()[0].clone();
        let mut steps = reads(&twelfth, &weekly(), vec![weekly()], october());
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261012T080000Z",
            twelfth["etag"].as_str().unwrap(),
            Scripted::Json(retitled(&twelfth, "\"i2\"")),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &twelfth,
            &weekly(),
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
    }

    /// Review M1, for the new part and a series delete: blocked before the hold.
    #[tokio::test]
    async fn a_held_split_blocks_a_series_delete_too() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let script = Script::new(october_reads());
        let outcome = held_delete(&host, &script, &monday_19(), Scope::All).await;
        script.finished();
        assert_eq!(held_token(&outcome), held_token(&held));
    }

    /// §6.3: the choices show a held split's rows as pending, and "this event" before it as offered.
    #[tokio::test]
    async fn choices_show_pending_rows_while_a_split_is_held() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let token = held_token(&held);
        let ask_on = |instance: Value| {
            let host = Arc::clone(&host);
            async move {
                let script = Script::new(reads(&instance, &weekly(), vec![weekly()], october()));
                let got = choices(
                    &script,
                    &*host,
                    &target(&instance),
                    EditingStatus::On,
                    &mirror(&["abc"]),
                    &edit(title("Again")),
                    Some((&instance, Some(&weekly()))),
                )
                .await;
                script.finished();
                match got {
                    SeriesChoices::Ready {
                        this,
                        following,
                        all,
                        notice: None,
                    } => (this, following, all),
                    _ => panic!("no rows"),
                }
            }
        };
        let pending =
            |row: &ScopeRow| matches!(row, ScopeRow::Pending { token: t, .. } if *t == token);
        let (this, following, all) = ask_on(monday_19()).await;
        assert!(pending(&this) && pending(&following) && pending(&all));
        // The new part is held under the same split.
        assert_eq!(
            host.edit.held_on(CALENDAR, TAIL).map(|v| v.token),
            Some(token.clone())
        );
        let (this, following, all) = ask_on(october()[0].clone()).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert!(pending(&following) && pending(&all));
    }

    /// H2: the split is taken before anything is sent, so a token already taken (or settled, or
    /// from before a restart) finds nothing to settle, with no request at all.
    #[tokio::test]
    async fn a_second_settle_finds_nothing_to_settle() {
        let host = TestHost::holding(&october());
        let (outcome, _) = settle(&host, "no-such-split", SplitAction::Check, Vec::new()).await;
        assert_eq!(
            outcome,
            WriteOutcome::Failed {
                message: SETTLING.into()
            }
        );
    }

    /// H2, C9a-3b critique M1: both series are claimed before the split is taken; either claimed
    /// already is Busy, with nothing sent and the split still held.
    #[tokio::test]
    async fn settle_claims_both_series_before_taking_the_split() {
        for series in ["abc", TAIL] {
            let host = TestHost::holding(&october());
            let (held, _) = lost_cut(&host, lost()).await;
            let token = held_token(&held);
            let _other = InFlight::claim(&host.edit, &series_key(CALENDAR, series)).unwrap();
            let (outcome, _) = settle(&host, &token, SplitAction::Check, Vec::new()).await;
            // PM's own claim, worded as such (C9b-1 critique 1 L9), with the buttons kept.
            still_held_with(
                &host,
                &outcome,
                &token,
                "PM is still changing this repeating event; try again in a moment.",
            );
            assert_eq!(held_count(&host), 1);
            // A save meanwhile still meets the held split.
            assert!(host.edit.held_on(CALENDAR, "abc").is_some());
        }
    }

    /// C9a-3b critique L7: a dev build's allow-list refusing the calendar stops a settle before
    /// anything is sent.
    #[tokio::test]
    async fn a_settle_the_allow_list_refuses_sends_nothing() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        *host.allowed.lock().unwrap() = Err("not this calendar");
        let (outcome, _) = settle(&host, &held_token(&held), SplitAction::Check, Vec::new()).await;
        still_held_with(&host, &outcome, &held_token(&held), "not this calendar");
        assert_eq!(held_count(&host), 1);
    }

    /// Editing turned off before a settle: refused after the take, nothing sent, and the change
    /// still held with its buttons and the reason.
    #[tokio::test]
    async fn a_settle_with_editing_off_keeps_the_change_held() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let token = held_token(&held);
        *host.editing.lock().unwrap() = EditingStatus::Paused;
        let (outcome, script) = settle(&host, &token, SplitAction::Finish, Vec::new()).await;
        script.finished();
        still_held_with(
            &host,
            &outcome,
            &token,
            read_only_message(ReadOnlyReason::EditingOff),
        );
        assert_eq!(held_count(&host), 1);
        // A token that's gone stays a plain answer: there's nothing to keep buttons for.
        let (gone, _) = settle(&host, "no-such-split", SplitAction::Check, Vec::new()).await;
        assert!(note_of(&gone).is_none());
        assert!(matches!(gone, WriteOutcome::Failed { .. }));
    }

    /// Check reads only, and holds the split again under its token with the new offers.
    #[tokio::test]
    async fn settle_check_looks_again_and_keeps_the_token() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let token = held_token(&held);
        let plan = split_plan(Vec::new());
        let (outcome, script) = settle(
            &host,
            &token,
            SplitAction::Check,
            vec![event_read("abc", Scripted::Json(cut_copy(&plan)))],
        )
        .await;
        script.finished();
        assert_eq!(held_token(&outcome), token);
        assert_eq!(offers_of(&outcome), [SplitAction::Finish]);
        assert_eq!(held_count(&host), 1);
        // Held still, so its record stays for a restart.
        assert_eq!(host.journal().len(), 1);
        // Held again with the new offers, so a blocked save says them too.
        assert_eq!(
            host.edit.held_on(CALENDAR, "abc").unwrap().offers,
            [SplitAction::Finish]
        );
    }

    /// Remove and Finish not among the offers Google allows now: nothing is sent, the offers redraw.
    #[tokio::test]
    async fn an_action_google_no_longer_allows_sends_nothing() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let plan = split_plan(Vec::new());
        let (outcome, script) = settle(
            &host,
            &held_token(&held),
            SplitAction::Remove,
            vec![event_read("abc", Scripted::Json(cut_copy(&plan)))],
        )
        .await;
        script.finished();
        assert_eq!(offers_of(&outcome), [SplitAction::Finish]);
    }

    /// §7.4, L8: Remove, once the look proves the original uncut and the new part is as PM made it,
    /// deletes only the new part, on its etag, and takes its rows out of the mirror.
    #[tokio::test]
    async fn remove_deletes_only_the_new_part_and_its_rows() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (held, _) = lost_cut(&host, Scripted::Json(written(&weekly(), &[], "\"m7\""))).await;
        // A sync mirrored the new part meanwhile.
        host.with_conn(|conn| {
            let mut rows = rows_of(&october());
            rows.extend(rows_of(&tail_october()));
            calendar::replace_events(conn, "cal", &rows, true)
        })
        .unwrap();
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let mut steps = vec![event_read(
            "abc",
            Scripted::Json(written(&weekly(), &[], "\"m7\"")),
        )];
        steps.extend(tail_reads(tail.clone(), Vec::new()));
        // Settle(Remove): the original, uncut away from the cut's etag; the new part; its delete.
        steps.push(event_read(
            "abc",
            Scripted::Json(written(&weekly(), &[], "\"m7\"")),
        ));
        steps.push(event_read(TAIL, Scripted::Json(tail)));
        steps.push(Step::write(
            Method::Delete,
            &format!("/events/{TAIL}"),
            "\"t1\"",
            Scripted::Status(204),
        ));
        let (outcome, _) = settle(&host, &held_token(&held), SplitAction::Remove, steps).await;
        assert_eq!(outcome, WriteOutcome::SplitRemoved);
        assert_eq!(split_rows(&host), (ids_of(rows_of(&october())), Vec::new()));
        assert_eq!(held_count(&host), 0);
        assert!(host.journal().is_empty());
    }

    /// C9a-3b critiques M6, M7: a new part renamed in Google since, or holding an occurrence changed
    /// on its own, isn't removed; the split stays held with Check only, and says the way out.
    #[tokio::test]
    async fn remove_refuses_when_the_new_part_changed_in_google() {
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let renamed = with(
            with(tail.clone(), "summary", json!("Renamed")),
            "etag",
            json!("\"t2\""),
        );
        let moved = with(
            tail_occurrence("2026-11-02T09:00:00Z", "\"x1\""),
            "summary",
            json!("Moved"),
        );
        // A renamed new part is read once; its occurrences are listed only when it's as PM made it.
        let reads = [
            vec![event_read(TAIL, Scripted::Json(renamed))],
            tail_reads(tail.clone(), vec![moved]),
        ];
        for tail_steps in reads {
            let host = TestHost::holding(&october());
            let (held, _) =
                lost_cut(&host, Scripted::Json(written(&weekly(), &[], "\"m7\""))).await;
            let token = held_token(&held);
            let mut steps = vec![event_read(
                "abc",
                Scripted::Json(written(&weekly(), &[], "\"m7\"")),
            )];
            steps.extend(tail_steps);
            let (outcome, script) = settle(&host, &token, SplitAction::Remove, steps).await;
            script.finished();
            // The tail-changed words, with Check left as the one button.
            assert!(note_of(&outcome).is_some_and(|n| n.contains("changed in Google")));
            assert_eq!(offers_of(&outcome), [SplitAction::Check]);
            let back = host.edit.held_on(CALENDAR, "abc").unwrap();
            assert_eq!(back.offers, [SplitAction::Check]);
        }
    }

    /// Review round 1: Remove deletes the new part only at the etag it just checked; changed in
    /// Google between that look and the delete's own, nothing is deleted and the split stays held.
    #[tokio::test]
    async fn remove_deletes_the_new_part_only_at_the_version_it_checked() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (held, _) = lost_cut(&host, Scripted::Json(written(&weekly(), &[], "\"m7\""))).await;
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let uncut = || event_read("abc", Scripted::Json(written(&weekly(), &[], "\"m7\"")));
        let mut steps = vec![uncut()];
        steps.extend(tail_reads(tail.clone(), Vec::new()));
        steps.push(uncut());
        steps.push(event_read(
            TAIL,
            Scripted::Json(with(tail, "etag", json!("\"t2\""))),
        ));
        let (outcome, script) = settle(&host, &held_token(&held), SplitAction::Remove, steps).await;
        script.finished();
        assert!(matches!(outcome, WriteOutcome::PartialSplit { .. }));
        assert_eq!(held_count(&host), 1);
    }

    /// C9a-3b critique M7: a new part found changed keeps Remove off on the next look while it stays
    /// changed, and gives it back once Google shows it as PM made it.
    #[tokio::test]
    async fn a_changed_new_part_keeps_remove_off_until_it_is_as_made() {
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let renamed = with(
            with(tail.clone(), "summary", json!("Renamed")),
            "etag",
            json!("\"t2\""),
        );
        let uncut = || event_read("abc", Scripted::Json(written(&weekly(), &[], "\"m7\"")));
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, Scripted::Json(written(&weekly(), &[], "\"m7\""))).await;
        let token = held_token(&held);
        let (refused, _) = settle(
            &host,
            &token,
            SplitAction::Remove,
            vec![uncut(), event_read(TAIL, Scripted::Json(renamed.clone()))],
        )
        .await;
        assert!(note_of(&refused).is_some_and(|n| n.contains("changed in Google")));
        // Still renamed: Check only.
        let (still, _) = settle(
            &host,
            &token,
            SplitAction::Check,
            vec![uncut(), event_read(TAIL, Scripted::Json(renamed))],
        )
        .await;
        assert_eq!(offers_of(&still), [SplitAction::Check]);
        // Renamed back: Remove again.
        let mut steps = vec![uncut()];
        steps.extend(tail_reads(tail, Vec::new()));
        let (back, _) = settle(&host, &token, SplitAction::Check, steps).await;
        assert_eq!(offers_of(&back), [SplitAction::Remove]);
    }

    /// Q2 with something to carry: Finish after a landed cut meets the occurrence changed since the
    /// save; nothing is sent, what Google holds lands, and that occurrence is said left behind, its
    /// row kept.
    #[tokio::test]
    async fn finish_after_a_landed_cut_leaves_a_changed_occurrence_behind() {
        let (events, mut steps) = with_off_site();
        let host = TestHost::holding(&events);
        let plan = split_plan(vec![off_site()]);
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        steps.extend([
            insert_step(Scripted::Json(tail.clone())),
            series_patch(Scripted::Lost { retried: true }),
            event_read("abc", lost()),
            event_read("abc", lost()),
        ]);
        let script = Script::new(steps);
        let (held, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.asked_all();
        let changed_since = with(off_site(), "summary", json!("Changed since"));
        let mut steps = vec![event_read("abc", Scripted::Json(cut_copy(&plan)))];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![cut_copy(&plan), changed_since.clone()], None)),
        ));
        steps.push(refetch_read(Scripted::Json(page(
            vec![october()[0].clone(), changed_since],
            None,
        ))));
        steps.push(tail_refetch(Scripted::Json(page(tail_october(), None))));
        let (outcome, script) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert_eq!(
            outcome,
            WriteOutcome::Saved {
                warnings: vec![WriteWarning::LeftOnOldSeries],
                left_behind: vec!["2026-11-02".into()],
                milestone_moves: None,
            }
        );
        assert!(split_rows(&host)
            .0
            .contains(&"cal:abc_20261102T090000Z".to_string()));
        assert_eq!(held_count(&host), 0);
    }

    /// §7.4: Remove never deletes after the cut landed: the look says Finish.
    #[tokio::test]
    async fn remove_never_deletes_after_the_cut_landed() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (held, _) = lost_cut(&host, Scripted::Json(weekly())).await;
        assert!(offers_of(&held).contains(&SplitAction::Remove));
        let (outcome, script) = settle(
            &host,
            &held_token(&held),
            SplitAction::Remove,
            vec![event_read("abc", Scripted::Json(cut_copy(&plan)))],
        )
        .await;
        script.finished();
        assert_eq!(offers_of(&outcome), [SplitAction::Finish]);
    }

    /// §7.5: Finish after a lost cut that landed: the new part and both listings read again, then the
    /// rest of the same plan, and both series land; nothing is held after.
    #[tokio::test]
    async fn finish_after_a_lost_cut_runs_the_rest_and_lands() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (held, _) = lost_cut(&host, lost()).await;
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        // The settle marks its split running from before it's taken out of `partials`.
        let at_look = Arc::clone(&host);
        let mut steps = vec![
            event_read("abc", Scripted::Json(cut_copy(&plan))).then(move || {
                assert!(at_look.edit.splitting(&series_key(CALENDAR, TAIL)));
            }),
        ];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![cut_copy(&plan)], None)),
        ));
        steps.push(refetch_read(Scripted::Json(page(
            vec![october()[0].clone()],
            None,
        ))));
        steps.push(tail_refetch(Scripted::Json(page(tail_october(), None))));
        assert_eq!(host.journal().len(), 1);
        let (outcome, script) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert_eq!(outcome, saved());
        assert_eq!(split_rows(&host).1, ids_of(rows_of(&tail_october())));
        assert_eq!(held_count(&host), 0);
        // Finished: its record goes, and so does the mark.
        assert!(host.journal().is_empty());
        assert!(!host.edit.splitting(&series_key(CALENDAR, TAIL)));
    }

    // --- the journal, and the overlap backstop (C9a-3b-2) ---

    /// The record `split_plan(..)` keeps, as if PM closed with it running: `cleanup` when the
    /// change had occurrences to move.
    fn orphan_entry(plan: &SeriesPlan, cleanup: bool) -> Entry {
        let Shows::Tail { plan: digest, .. } = &plan.steps[0].shows else {
            panic!("not a split");
        };
        let Shows::Fields { sent, old } = &plan.steps[1].shows else {
            panic!("not a cut");
        };
        let lines = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_string())
                .collect()
        };
        Entry {
            calendar: CALENDAR.into(),
            series: "abc".into(),
            tail: TAIL.into(),
            plan: digest.clone(),
            from: "2026-10-19".into(),
            at: "2026-10-19T08:00:00+00:00".into(),
            zone: "Europe/London".into(),
            start: weekly()["start"].clone(),
            before: lines(&old["recurrence"]),
            cut: lines(&sent["recurrence"]),
            cleanup,
        }
    }

    /// A host whose journal holds `entry` and no split in memory: PM restarted since.
    fn restarted_with(entry: &Entry) -> Arc<TestHost> {
        let host = TestHost::holding(&october());
        host.with_conn(|conn| journal_add(conn, entry)).unwrap();
        host
    }

    fn refused_with(message: &str) -> ScopeRow {
        ScopeRow::Refused {
            message: message.into(),
        }
    }

    fn rows_of_choices(got: SeriesChoices) -> (ScopeRow, ScopeRow, ScopeRow, Option<String>) {
        match got {
            SeriesChoices::Ready {
                this,
                following,
                all,
                notice,
            } => (this, following, all, notice),
            _ => panic!("no rows"),
        }
    }

    /// H3: the record goes in before the first write, holding the ids, the plan's note and the
    /// original's rule before and after the cut, and goes once the split has landed.
    #[tokio::test]
    async fn a_split_is_recorded_before_its_first_write_and_forgotten_once_landed() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let at_insert = Arc::clone(&host);
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))).then(move || {
                    let journal = at_insert.journal();
                    let entry = journal.get(&journal::key(CALENDAR, "abc")).unwrap();
                    let want = orphan_entry(&split_plan(Vec::new()), false);
                    assert!(*entry == want, "the record isn't the split's");
                    // And marked as this run's, so no look reads it as left over.
                    assert!(at_insert.edit.splitting(&series_key(CALENDAR, TAIL)));
                }),
                series_patch(Scripted::Json(cut_copy(&plan))),
                refetch_read(Scripted::Json(page(vec![october()[0].clone()], None))),
                tail_refetch(Scripted::Json(page(tail_october(), None))),
            ],
        )
        .await;
        assert_eq!(outcome, saved());
        assert!(host.journal().is_empty());
        assert!(!host.edit.splitting(&series_key(CALENDAR, TAIL)));
    }

    /// A split whose record can't be kept sends nothing: a quit mid-change would leave no trace.
    #[tokio::test]
    async fn a_split_whose_record_cant_be_kept_sends_nothing() {
        let host = TestHost::holding(&october());
        host.with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER no_journal BEFORE INSERT ON settings \
                 BEGIN SELECT RAISE(ABORT, 'full'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        let (outcome, script, _) = split_save(&host, Vec::new()).await;
        script.finished();
        assert!(matches!(outcome, WriteOutcome::Failed { .. }));
        assert_eq!(held_count(&host), 0);
    }

    /// A split held in memory keeps its record (a restart then finds it); one whose rollback proved
    /// nothing stands forgets it.
    #[tokio::test]
    async fn a_held_split_keeps_its_record_and_a_rolled_back_one_forgets_it() {
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        assert!(matches!(held, WriteOutcome::PartialSplit { .. }));
        assert_eq!(host.journal().len(), 1);
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (outcome, _, _) = split_save(
            &host,
            vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Json(written(&weekly(), &[], "\"m2\""))),
                Step::write(
                    Method::Delete,
                    &format!("/events/{TAIL}"),
                    "\"t1\"",
                    Scripted::Status(204),
                ),
            ],
        )
        .await;
        assert_eq!(
            outcome,
            WriteOutcome::IgnoredByGoogle { split: true },
            "Google kept the old rule"
        );
        assert!(host.journal().is_empty());
    }

    /// C1-H1: after a restart, a new part that's there while the original provably still holds
    /// its rule as before shows twice: every change it reaches is refused with what to do, the
    /// occurrences before the split stay open to "this event", and nothing is deleted.
    #[tokio::test]
    async fn after_a_restart_a_proven_duplicate_refuses_what_it_reaches() {
        let plan = split_plan(Vec::new());
        let entry = orphan_entry(&plan, false);
        let host = restarted_with(&entry);
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let twice = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::Twice,
        }
        .words();
        assert!(twice.contains("shows twice from 19-10-2026"));
        let ask_at = |instance: Value| {
            let (host, tail) = (Arc::clone(&host), tail.clone());
            async move {
                let mut steps = reads_with_part(&instance, &tail);
                steps.push(event_read(TAIL, Scripted::Json(tail)));
                let script = Script::new(steps);
                let opened = Some((&instance, Some(&weekly())));
                rows_of_choices(
                    ask_on(
                        &host,
                        &script,
                        &instance,
                        edit(title("Again")),
                        opened,
                        &["abc"],
                    )
                    .await,
                )
            }
        };
        let (this, following, all, notice) = ask_at(monday_19()).await;
        assert_eq!(notice, None);
        for row in [this, following, all] {
            assert_eq!(row, refused_with(&twice));
        }
        let (this, following, all, _) = ask_at(october()[0].clone()).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!(
            (following, all),
            (refused_with(&twice), refused_with(&twice))
        );
        // The send refuses alike, reading the same; only reads went out.
        let mut steps = reads_with_part(&monday_19(), &tail);
        steps.push(event_read(TAIL, Scripted::Json(tail.clone())));
        let script = Script::new(steps);
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
        assert_eq!(outcome, WriteOutcome::Failed { message: twice });
        assert_eq!(host.journal().len(), 1);
        // "This event" before the split isn't reached, and saves.
        let twelfth = october()[0].clone();
        let mut steps = reads_with_part(&twelfth, &tail);
        steps.push(event_read(TAIL, Scripted::Json(tail.clone())));
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261012T080000Z",
            twelfth["etag"].as_str().unwrap(),
            Scripted::Json(retitled(&twelfth, "\"i2\"")),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &twelfth,
            &weekly(),
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert_eq!(host.journal().len(), 1);
        // Its exceptions not read whole: PM can't say it shows twice, so it can't check it now.
        let unknown = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::Unknown,
        }
        .words();
        let steps = vec![
            instance_read(&monday_19()),
            Step::get("/events/abc", "", weekly()),
            Step::get(
                "/events",
                "iCalUID=abc%40google.com",
                page(vec![weekly()], Some("p2")),
            ),
            Step::status("/events", "pageToken=p2", 500),
            window_read(october()),
            parts_read(vec![tail.clone()]),
            event_read(TAIL, Scripted::Json(tail.clone())),
        ];
        let script = Script::new(steps);
        let opened = Some((&monday_19(), Some(&weekly())));
        let (this, _, _, notice) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &monday_19(),
                edit(title("Again")),
                opened,
                &["abc"],
            )
            .await,
        );
        assert_eq!((this, notice), (refused_with(&unknown), None));
        assert_eq!(host.journal().len(), 1);
        // The original's occurrences from the split deleted one by one in Google: its rule is as
        // before, but nothing shows twice. Said once as changed, then the backstop guards it.
        let mut cleared = october();
        cleared[1] = cancelled("2026-10-19T09:00:00+01:00");
        cleared[2] = cancelled("2026-10-26T09:00:00Z");
        // Listed whole: the series and its two cancelled occurrences.
        let mut listed = vec![weekly()];
        listed.extend(cleared[1..].iter().cloned());
        let mut steps = reads(&twelfth, &weekly(), listed, cleared);
        steps.pop();
        steps.push(parts_read(vec![tail.clone()]));
        steps.push(event_read(TAIL, Scripted::Json(tail)));
        let script = Script::new(steps);
        let opened = Some((&twelfth, Some(&weekly())));
        let (_, _, _, notice) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &twelfth,
                edit(title("Again")),
                opened,
                &["abc"],
            )
            .await,
        );
        let changed = Orphaned {
            entry,
            orphan: Orphan::Changed,
        }
        .words();
        assert_eq!(notice, Some(changed));
        assert!(host.journal().is_empty());
    }

    /// The October reads of `instance`, the parts listing holding the new part `tail`.
    fn reads_with_part(instance: &Value, tail: &Value) -> Vec<Step> {
        let mut steps = reads(instance, &weekly(), vec![weekly()], october());
        steps.pop();
        steps.push(parts_read(vec![tail.clone()]));
        steps
    }

    /// Q4 as narrowed by critique C1-H1: after a restart PM offers no delete, not even of a proven
    /// duplicate's new part; the words point to Google Calendar. Once Google shows the original cut,
    /// the record clears and the new part is free again.
    #[tokio::test]
    async fn a_proven_duplicates_new_part_is_refused_every_change_delete_included() {
        let plan = split_plan(Vec::new());
        let entry = orphan_entry(&plan, false);
        let host = restarted_with(&entry);
        // As Google answers a GET of it: the organizer and the type filled in.
        let mut tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        tail["organizer"] = weekly()["organizer"].clone();
        tail["eventType"] = json!("default");
        let occurrence = tail_occurrence("2026-10-26T09:00:00Z", "\"u1\"");
        // The new part's reads, then the original, and its exceptions and occurrences when an uncut
        // rule needs them to say it shows twice.
        let tail_series_reads = |original: Value, occurrences: Option<(Vec<Value>, Vec<Value>)>| {
            let mut steps = vec![
                instance_read(&occurrence),
                event_read(TAIL, Scripted::Json(tail.clone())),
                ical_listing(
                    "iCalUID=tail%40google.com",
                    Scripted::Json(page(vec![tail.clone()], None)),
                ),
                Step::get(
                    &format!("/events/{TAIL}/instances"),
                    "timeMin=2026-09-01",
                    page(vec![masked(&occurrence)], None),
                ),
                Step::get(
                    "/events",
                    "privateExtendedProperty=pmSplitOf%3D0123456789abcdef0123456789abcdef",
                    page(Vec::new(), None),
                ),
                event_read("abc", Scripted::Json(original)),
            ];
            if let Some((listed, window)) = occurrences {
                steps.push(ical_listing(
                    "iCalUID=abc%40google.com",
                    Scripted::Json(page(listed, None)),
                ));
                steps.push(window_read(window));
            }
            steps
        };
        let uncut = || Some((vec![weekly()], october()));
        let delete = ChoiceRequest::Delete {
            event_id: format!("cal:{}", occurrence["id"].as_str().unwrap()),
            seen: seen_by_user(&fresh_event(occurrence.clone())),
        };
        let script = Script::new(tail_series_reads(weekly(), uncut()));
        let (this, following, all, _) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &occurrence,
                delete.clone(),
                None,
                &["abc", TAIL],
            )
            .await,
        );
        let twice = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::Twice,
        }
        .words();
        for row in [this, following, all] {
            assert_eq!(row, refused_with(&twice));
        }
        let script = Script::new(tail_series_reads(weekly(), uncut()));
        let opened = Some((&occurrence, Some(&tail)));
        let (_, _, all, _) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &occurrence,
                edit(title("Again")),
                opened,
                &["abc", TAIL],
            )
            .await,
        );
        assert_eq!(all, refused_with(&twice));
        // The original's exceptions unread: unknown, never twice, and the record stays.
        let mut steps = tail_series_reads(weekly(), None);
        steps.push(Step::status("/events", "iCalUID=abc%40google.com", 500));
        steps.push(window_read(october()));
        let script = Script::new(steps);
        let (_, _, all, _) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &occurrence,
                delete.clone(),
                None,
                &["abc", TAIL],
            )
            .await,
        );
        let unknown = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::Unknown,
        }
        .words();
        assert_eq!(all, refused_with(&unknown));
        assert_eq!(host.journal().len(), 1);
        // Its rule as before, but from the split on only an occurrence changed on its own is left
        // on it: the rule no longer repeats there, so nothing shows twice. Said once as changed.
        let other = restarted_with(&entry);
        let moved_26 = moved(
            "2026-10-26T09:00:00Z",
            "2026-10-26T11:00:00Z",
            "2026-10-26T12:00:00Z",
        );
        let cancelled_19 = cancelled("2026-10-19T09:00:00+01:00");
        let left = Some((
            vec![weekly(), cancelled_19.clone(), moved_26.clone()],
            vec![october()[0].clone(), cancelled_19, moved_26],
        ));
        let script = Script::new(tail_series_reads(weekly(), left));
        let (_, _, all, notice) = rows_of_choices(
            ask_on(
                &other,
                &script,
                &occurrence,
                delete.clone(),
                None,
                &["abc", TAIL],
            )
            .await,
        );
        let changed = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::Changed,
        }
        .words();
        assert_eq!(notice, Some(changed));
        assert!(matches!(all, ScopeRow::Offered { .. }));
        // The original cut since (in Google): nothing shows twice, and nothing was left to move.
        let script = Script::new(tail_series_reads(cut_copy(&plan), None));
        let (_, _, all, notice) = rows_of_choices(
            ask_on(&host, &script, &occurrence, delete, None, &["abc", TAIL]).await,
        );
        assert_eq!(notice, None, "nothing to move, so nothing to say");
        assert!(host.journal().is_empty());
        assert!(matches!(all, ScopeRow::Offered { .. }));
    }

    /// A record whose change Google shows landed, or whose rule changed in Google since, is said
    /// once beside the rows and forgotten; the rows are planned as ever.
    #[tokio::test]
    async fn a_landed_or_changed_record_is_said_once_then_forgotten() {
        let plan = split_plan(Vec::new());
        let cut = cut_copy(&plan);
        let changed = written(
            &weekly(),
            &[(
                "recurrence",
                json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261201T000000Z"]),
            )],
            "\"m5\"",
        );
        for (original, orphan) in [(cut, Orphan::CutLanded), (changed, Orphan::Changed)] {
            let entry = orphan_entry(&plan, true);
            let host = restarted_with(&entry);
            let twelfth = october()[0].clone();
            let ask = |with_look: bool| {
                let mut steps = reads(
                    &twelfth,
                    &original,
                    vec![original.clone()],
                    vec![twelfth.clone()],
                );
                if with_look {
                    steps.push(event_read(
                        TAIL,
                        Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT)),
                    ));
                }
                Script::new(steps)
            };
            let opened = Some((&twelfth, Some(&original)));
            let script = ask(true);
            let (this, _, _, notice) = rows_of_choices(
                ask_on(
                    &host,
                    &script,
                    &twelfth,
                    edit(title("Again")),
                    opened,
                    &["abc"],
                )
                .await,
            );
            let words = Orphaned {
                entry: entry.clone(),
                orphan,
            }
            .words();
            assert_eq!(notice.as_deref(), Some(words.as_str()), "{orphan:?}");
            assert!(matches!(this, ScopeRow::Offered { .. }));
            assert!(host.journal().is_empty());
            // Said: the next look reads nothing more and says nothing.
            let script = ask(false);
            let (_, _, _, notice) = rows_of_choices(
                ask_on(
                    &host,
                    &script,
                    &twelfth,
                    edit(title("Again")),
                    opened,
                    &["abc"],
                )
                .await,
            );
            assert_eq!(notice, None);
        }
    }

    /// Two records on one series, one as its original (abc → the new part) and one as its new part
    /// (an earlier xyz → abc), both said and forgotten at once: each is said, since neither can be
    /// said again (C9b-1 spec §1.5, critique 1 L4).
    #[tokio::test]
    async fn two_orphans_both_say_their_words_once() {
        let plan = split_plan(Vec::new());
        let as_original = orphan_entry(&plan, true);
        // abc is itself the new part of an earlier change to xyz, whose start PM can't read back.
        let as_tail = Entry {
            series: "xyz".into(),
            tail: "abc".into(),
            plan: "earlier-plan".into(),
            from: "2026-10-05".into(),
            start: Value::Null,
            ..as_original.clone()
        };
        let host = restarted_with(&as_original);
        host.with_conn(|conn| journal_add(conn, &as_tail)).unwrap();
        // abc's repeat changed in Google since; it carries the note of the change that made it.
        let mut abc = written(
            &weekly(),
            &[(
                "recurrence",
                json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261201T000000Z"]),
            )],
            "\"m5\"",
        );
        abc["extendedProperties"] =
            json!({ "private": { "pmSplitOf": "xyz", "pmSplitPlan": "earlier-plan" } });
        let xyz = with(weekly(), "id", json!("xyz"));
        let twelfth = october()[0].clone();
        let ask = |with_looks: bool| {
            let mut steps = reads(&twelfth, &abc, vec![abc.clone()], vec![twelfth.clone()]);
            if with_looks {
                // In the journal's order: abc's own record looks at its new part, then xyz's at xyz.
                steps.push(event_read(
                    TAIL,
                    Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT)),
                ));
                steps.push(event_read("xyz", Scripted::Json(xyz.clone())));
            }
            Script::new(steps)
        };
        let opened = Some((&twelfth, Some(&abc)));
        let script = ask(true);
        let (this, _, _, notice) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &twelfth,
                edit(title("Again")),
                opened,
                &["abc"],
            )
            .await,
        );
        script.finished();
        let words = |entry: &Entry| {
            Orphaned {
                entry: entry.clone(),
                orphan: Orphan::Changed,
            }
            .words()
        };
        let both = format!("{} {}", words(&as_original), words(&as_tail));
        assert_eq!(notice.as_deref(), Some(both.as_str()));
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert!(host.journal().is_empty());
        // Both said: the next ask reads nothing more for them and says nothing.
        let script = ask(false);
        let (_, _, _, notice) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &twelfth,
                edit(title("Again")),
                opened,
                &["abc"],
            )
            .await,
        );
        script.finished();
        assert_eq!(notice, None);
    }

    /// A record whose new part is gone is forgotten by the next send, which goes ahead; one PM
    /// can't check refuses what it reaches, and stays.
    #[tokio::test]
    async fn a_gone_new_part_clears_the_record_and_an_unread_one_refuses() {
        let plan = split_plan(Vec::new());
        let entry = orphan_entry(&plan, true);
        let host = restarted_with(&entry);
        let instance = monday_19();
        let mut steps = october_reads();
        steps.push(event_read(TAIL, Scripted::Status(500)));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &weekly(),
            title("Retro"),
            Scope::This,
        )
        .await;
        script.finished();
        let unknown = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::Unknown,
        }
        .words();
        assert_eq!(outcome, WriteOutcome::Failed { message: unknown });
        assert_eq!(host.journal().len(), 1);
        let mut steps = october_reads();
        steps.push(event_read(TAIL, Scripted::Status(404)));
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            instance["etag"].as_str().unwrap(),
            Scripted::Json(retitled(&instance, "\"i2\"")),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &weekly(),
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert!(host.journal().is_empty());
    }

    /// A landed change's record met by a send (no dialog asked first) is said by refusing once;
    /// the next send goes ahead. Forgetting a split leaves a later one's record under the same key.
    #[tokio::test]
    async fn a_landed_record_met_by_a_send_refuses_once() {
        let plan = split_plan(Vec::new());
        let entry = orphan_entry(&plan, true);
        let host = restarted_with(&entry);
        let (cut, twelfth) = (cut_copy(&plan), october()[0].clone());
        let mut steps = reads(&twelfth, &cut, vec![cut.clone()], vec![twelfth.clone()]);
        steps.push(event_read(
            TAIL,
            Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT)),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(&host, &script, &twelfth, &cut, title("Retro"), Scope::All).await;
        script.finished();
        let words = Orphaned {
            entry: entry.clone(),
            orphan: Orphan::CutLanded,
        }
        .words();
        assert_eq!(outcome, WriteOutcome::Failed { message: words });
        assert!(host.journal().is_empty());
        let host = restarted_with(&entry);
        forget(&*host, CALENDAR, "abc", "another-tail");
        assert_eq!(host.journal().len(), 1);
        forget(&*host, CALENDAR, "abc", TAIL);
        assert!(host.journal().is_empty());
    }

    /// While the split or settle a record stands for runs, the record is no orphan: the choices read
    /// nothing for it and block nothing by it. Claims alone don't say so: two unrelated writes (a
    /// held delete on the original, a save on the new part) may hold both series (review round 2).
    #[tokio::test]
    async fn a_running_changes_record_is_left_to_it() {
        let plan = split_plan(Vec::new());
        let entry = orphan_entry(&plan, false);
        let host = restarted_with(&entry);
        let _claims = (
            InFlight::claim(&host.edit, &series_key(CALENDAR, "abc")).unwrap(),
            InFlight::claim(&host.edit, &series_key(CALENDAR, TAIL)).unwrap(),
        );
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let mut steps = reads_with_part(&monday_19(), &tail);
        steps.push(event_read(TAIL, Scripted::Json(tail.clone())));
        let script = Script::new(steps);
        let opened = Some((&monday_19(), Some(&weekly())));
        let (this, _, _, _) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &monday_19(),
                edit(title("Again")),
                opened,
                &["abc"],
            )
            .await,
        );
        let twice = Orphaned {
            entry,
            orphan: Orphan::Twice,
        }
        .words();
        assert_eq!(this, refused_with(&twice));
        let _running = Splitting::mark(&host.edit, series_key(CALENDAR, TAIL));
        let instance = monday_19();
        let script = Script::new(reads(&instance, &weekly(), vec![weekly()], october()));
        let opened = Some((&instance, Some(&weekly())));
        let (this, following, _, notice) = rows_of_choices(
            ask_on(
                &host,
                &script,
                &instance,
                edit(title("Again")),
                opened,
                &["abc"],
            )
            .await,
        );
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert!(matches!(following, ScopeRow::Offered { .. }));
        assert_eq!((notice, host.journal().len()), (None, 1));
    }

    /// scope-M4: a part PM split off that still overlaps the series refuses every change past this
    /// occurrence, in the choices and the send alike, whatever the journal holds; a parts listing
    /// PM couldn't read refuses the same way.
    #[tokio::test]
    async fn an_overlapping_part_refuses_past_this_occurrence() {
        let plan = split_plan(Vec::new());
        let part = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let instance = monday_19();
        let with_parts = |parts: Step| {
            let mut steps = vec![instance_read(&instance)];
            steps.extend(series_reads(&weekly(), vec![weekly()], october()));
            steps.pop();
            steps.push(parts);
            steps
        };
        let overlap =
            SeriesRefusal::SplitOverlap(chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap())
                .to_string();
        assert!(overlap.contains("from 19-10-2026"));
        let opened = Some((&instance, Some(&weekly())));
        let script = Script::new(with_parts(parts_read(vec![part.clone()])));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Again")), opened, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!(
            (following, all),
            (refused_with(&overlap), refused_with(&overlap))
        );
        let unread = SeriesRefusal::PartsUnread.to_string();
        let script = Script::new(with_parts(Step::status(
            "/events",
            "privateExtendedProperty=pmSplitOf%3Dabc",
            500,
        )));
        let (this, following, all) =
            ask(&script, &instance, edit(title("Again")), opened, &["abc"]).await;
        assert!(matches!(this, ScopeRow::Offered { .. }));
        assert_eq!(
            (following, all),
            (refused_with(&unread), refused_with(&unread))
        );
        let host = TestHost::holding(&october());
        let script = Script::new(with_parts(parts_read(vec![part])));
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &weekly(),
            title("Retro"),
            Scope::All,
        )
        .await;
        script.finished();
        assert_eq!(outcome, WriteOutcome::Failed { message: overlap });
    }

    /// §7.5: Finish after a lost insert whose new part is there sends the cut on its own etag, then
    /// lands.
    #[tokio::test]
    async fn finish_after_a_lost_insert_sends_the_cut_onward() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (held, _, _) = split_save(
            &host,
            vec![
                insert_step(lost()),
                event_read(TAIL, lost()),
                event_read(TAIL, Scripted::Json(tail.clone())),
            ],
        )
        .await;
        let mut steps = vec![event_read(TAIL, Scripted::Json(tail.clone()))];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![weekly()], None)),
        ));
        steps.push(series_patch(Scripted::Json(cut_copy(&plan))));
        steps.push(refetch_read(Scripted::Json(page(
            vec![october()[0].clone()],
            None,
        ))));
        steps.push(tail_refetch(Scripted::Json(page(tail_october(), None))));
        let (outcome, _) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        assert_eq!(outcome, saved());
        assert_eq!(held_count(&host), 0);
    }

    /// C9a-3b critique M8: a Finish whose cut is lost again is held again under the same token, as
    /// a cut sent: Remove then needs the original proved uncut, never just the new part there.
    #[tokio::test]
    async fn a_finish_that_ends_unknown_at_the_cut_reholds_the_new_run() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (held, _, _) = split_save(
            &host,
            vec![
                insert_step(lost()),
                event_read(TAIL, lost()),
                event_read(TAIL, Scripted::Json(tail.clone())),
            ],
        )
        .await;
        let token = held_token(&held);
        let mut steps = vec![event_read(TAIL, Scripted::Json(tail.clone()))];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![weekly()], None)),
        ));
        steps.push(series_patch(Scripted::Lost { retried: true }));
        steps.push(event_read("abc", lost()));
        // The look at the original after the lost cut.
        steps.push(event_read("abc", lost()));
        let (outcome, _) = settle(&host, &token, SplitAction::Finish, steps).await;
        assert_eq!(held_token(&outcome), token);
        assert_eq!(offers_of(&outcome), [SplitAction::Check]);
        let partials = host.edit.partials.lock().unwrap();
        let again = partials.get(&token).unwrap();
        assert_eq!(
            cut_kind(again.facts.insert, &again.ran),
            Some(CutKind::Sent)
        );
    }

    /// Review M5, Q2: Finish meets an occurrence of the original changed since the save. Not cut yet
    /// (the insert was lost): refused, nothing sent, Remove offered. Cut already: what Google holds
    /// stands, the occurrences the change would have moved are said left behind; nothing sent.
    #[tokio::test]
    async fn finish_refuses_when_an_occurrence_changed_since_the_save() {
        let changed_since = with(off_site(), "summary", json!("Changed since"));
        // Not cut: a lost insert, the new part there.
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (held, _, _) = split_save(
            &host,
            vec![
                insert_step(lost()),
                event_read(TAIL, lost()),
                event_read(TAIL, Scripted::Json(tail.clone())),
            ],
        )
        .await;
        let mut steps = vec![event_read(TAIL, Scripted::Json(tail.clone()))];
        steps.extend(tail_reads(tail.clone(), Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![weekly(), changed_since.clone()], None)),
        ));
        let (outcome, script) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        script.finished();
        // Still held, with Remove as the button its words name.
        still_held_with(&host, &outcome, &held_token(&held), FINISH_CHANGED);
        assert_eq!(offers_of(&outcome), [SplitAction::Remove]);
        assert_eq!(
            host.edit.held_on(CALENDAR, "abc").unwrap().offers,
            [SplitAction::Remove]
        );

        // Cut: a lost cut that landed.
        let host = TestHost::holding(&october());
        let (held, _) = lost_cut(&host, lost()).await;
        let mut steps = vec![event_read("abc", Scripted::Json(cut_copy(&plan)))];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![cut_copy(&plan), changed_since], None)),
        ));
        steps.push(refetch_read(Scripted::Json(page(
            vec![october()[0].clone()],
            None,
        ))));
        steps.push(tail_refetch(Scripted::Json(page(tail_october(), None))));
        let (outcome, script) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert_eq!(outcome, saved());
        assert_eq!(held_count(&host), 0);
    }

    /// Review round 4: Finish after a lost insert, the original deleted in Google meanwhile (its
    /// cancelled copy listed, or none): the cut isn't sent, so no rollback deletes the new part, now
    /// the only copy of what follows. Said, the original's rows go, nothing held.
    #[tokio::test]
    async fn finish_never_cuts_an_original_deleted_in_google() {
        let plan = split_plan(Vec::new());
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let deleted = written(&with(weekly(), "status", json!("cancelled")), &[], "\"m8\"");
        for listed in [vec![deleted], Vec::new()] {
            let host = TestHost::holding(&october());
            let (held, _, _) = split_save(
                &host,
                vec![
                    insert_step(lost()),
                    event_read(TAIL, lost()),
                    event_read(TAIL, Scripted::Json(tail.clone())),
                ],
            )
            .await;
            let mut steps = vec![event_read(TAIL, Scripted::Json(tail.clone()))];
            steps.extend(tail_reads(tail.clone(), Vec::new()));
            steps.push(ical_listing(
                "iCalUID=abc%40google.com",
                Scripted::Json(page(listed, None)),
            ));
            let (outcome, script) =
                settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
            script.finished();
            assert!(
                matches!(&outcome, WriteOutcome::Failed { message } if message.contains("deleted in Google"))
            );
            assert!(split_rows(&host).0.is_empty());
            assert_eq!(held_count(&host), 0);
        }
    }

    /// §7.5: the cut landed and the new part is gone: nothing PM can finish or undo; said, and
    /// nothing held.
    #[tokio::test]
    async fn finish_on_a_cut_whose_new_part_is_gone_says_so() {
        let host = TestHost::holding(&october());
        let plan = split_plan(Vec::new());
        let (held, _) = lost_cut(&host, lost()).await;
        let (outcome, _) = settle(
            &host,
            &held_token(&held),
            SplitAction::Finish,
            vec![
                event_read("abc", Scripted::Json(cut_copy(&plan))),
                event_read(TAIL, Scripted::Status(404)),
            ],
        )
        .await;
        assert_eq!(
            outcome,
            WriteOutcome::Failed {
                message: TAIL_GONE.into()
            }
        );
        assert_eq!(held_count(&host), 0);
    }

    /// The offers follow what Google shows (§5.3), and a new part changed in Google drops Remove
    /// and a Finish that would send the cut, keeping Check.
    #[test]
    fn offers_follow_what_google_shows() {
        use SplitAction::{Check, Finish, Remove};
        let copy = json!({});
        let hold = |o: &[SplitAction]| Offers::Hold(o.to_vec());
        let cases = [
            (CutKind::Sent, Settled::Removed, false, Offers::Removed),
            (
                CutKind::Sent,
                Settled::OriginalGone,
                false,
                Offers::OriginalGone,
            ),
            (
                CutKind::Sent,
                Settled::CutDone {
                    original: copy.clone(),
                },
                false,
                hold(&[Finish]),
            ),
            (
                CutKind::NotSent,
                Settled::TailThere { tail: copy.clone() },
                false,
                hold(&[Finish, Remove]),
            ),
            (
                CutKind::NotSent,
                Settled::TailAbsent,
                false,
                hold(&[Remove, Check]),
            ),
            (
                CutKind::NotSent,
                Settled::Unknown,
                false,
                hold(&[Remove, Check]),
            ),
            (CutKind::Sent, Settled::CutNotDone, false, hold(&[Remove])),
            (
                CutKind::Sent,
                Settled::CutUnproved,
                false,
                hold(&[Remove, Check]),
            ),
            (CutKind::Sent, Settled::Unknown, false, hold(&[Check])),
            (CutKind::Proved, Settled::CutNotDone, false, hold(&[Remove])),
            (
                CutKind::Proved,
                Settled::Unknown,
                false,
                hold(&[Remove, Check]),
            ),
            (CutKind::Proved, Settled::TailAbsent, false, hold(&[Check])),
            (
                CutKind::NotSent,
                Settled::TailThere { tail: copy.clone() },
                true,
                hold(&[Check]),
            ),
            (CutKind::Sent, Settled::CutNotDone, true, hold(&[Check])),
            (
                CutKind::Sent,
                Settled::CutDone { original: copy },
                true,
                hold(&[Finish, Check]),
            ),
        ];
        for (cut, settled, changed, want) in cases {
            assert!(offers_for(cut, &settled, changed) == want, "{cut:?}");
        }
    }

    /// Review M1's rule: every scope on the new part; on the original, all but "this event" before
    /// the split, read where the occurrence sits in its series (a moved one counts where it was).
    #[test]
    fn blocks_every_scope_on_the_new_part_and_this_from_the_split_on() {
        let view = HeldView {
            token: "t".into(),
            offers: Vec::new(),
            from: "2026-10-19".into(),
            master: "abc".into(),
            tail: TAIL.into(),
            at: point("2026-10-19T08:00:00Z"),
            zone: chrono_tz::Europe::London,
        };
        let before = Some(point("2026-10-12T08:00:00Z"));
        let on = Some(point("2026-10-19T08:00:00Z"));
        for scope in [Scope::This, Scope::Following, Scope::All] {
            assert!(blocks(&view, TAIL, before, scope));
            assert!(!blocks(&view, "other", on, scope));
        }
        assert!(!blocks(&view, "abc", before, Scope::This));
        assert!(blocks(&view, "abc", on, Scope::This));
        assert!(blocks(&view, "abc", None, Scope::This));
        assert!(blocks(&view, "abc", before, Scope::Following));
        assert!(blocks(&view, "abc", before, Scope::All));
    }

    /// C9a-3b critiques M5, M6, M11: the new part as Google holds it. A copy at the etag PM saw, or
    /// one only Google's bookkeeping moved, is as PM made it; a cancelled occurrence the original's
    /// deleted one went over as (an EXDATE) is no change, any other occurrence of its own is; and
    /// "none" needs the whole listing.
    #[tokio::test]
    async fn the_new_part_is_checked_against_what_pm_made() {
        let unchanged = |etag: &str| TailCheck::Unchanged { etag: etag.into() };
        let gone = cancelled("2026-11-02T09:00:00Z");
        let plan = split_plan(vec![gone.clone()]);
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let at = point("2026-10-19T09:00:00+01:00");
        let held = HeldSplit {
            target: target(&monday_19()),
            plan: plan.clone(),
            ran: Ran::Unknown {
                step: 1,
                sent_on: vec!["\"m1\"".into()],
                ends: vec![
                    End::Done {
                        reply: Some(tail.clone()),
                        date: None,
                    },
                    End::NotRun,
                ],
            },
            facts: SplitFacts {
                master: "abc".into(),
                tail: TAIL.into(),
                insert: 0,
                at,
                zone: chrono_tz::Europe::London,
                from: "2026-10-19".into(),
                moments: Vec::new(),
                marks: Vec::new(),
                ical: "abc@google.com".into(),
                movable: None,
            },
            offers: Vec::new(),
            tail_seen: Some(tail.clone()),
            tail_changed: false,
        };
        let of_tail = |mut v: Value| {
            let id = v["id"].as_str().unwrap().replacen("abc", TAIL, 1);
            v["id"] = json!(id);
            v["recurringEventId"] = json!(TAIL);
            v
        };
        let left_out = of_tail(gone);
        let deleted_since = of_tail(cancelled("2026-11-09T09:00:00Z"));
        let bookkeeping = with(tail.clone(), "etag", json!("\"t3\""));
        // At the etag PM saw, Google's own way of writing a field isn't a change.
        let as_seen = with(tail.clone(), "description", json!("<p>Daily notes</p>"));
        for (copy, listed, want) in [
            (tail.clone(), vec![left_out.clone()], unchanged("\"t1\"")),
            (bookkeeping, Vec::new(), unchanged("\"t3\"")),
            (as_seen, Vec::new(), unchanged("\"t1\"")),
            (
                tail.clone(),
                vec![left_out, deleted_since],
                TailCheck::Changed,
            ),
        ] {
            let script = Script::new(tail_reads(copy, listed));
            assert_eq!(tail_check(&script, &held).await, want);
            script.asked_all();
        }
        // A listing that stops short says nothing.
        let script = Script::new(vec![
            event_read(TAIL, Scripted::Json(tail.clone())),
            ical_listing(
                "iCalUID=tail%40google.com",
                Scripted::Json(page(vec![tail.clone()], Some("p2"))),
            ),
            ical_listing("iCalUID=tail%40google.com", Scripted::Status(500)),
        ]);
        assert_eq!(tail_check(&script, &held).await, TailCheck::Unread);
        // Gone, or another plan's under the same id.
        let mut other = tail.clone();
        other["extendedProperties"]["private"]["pmSplitPlan"] = json!("another");
        for answer in [Scripted::Status(404), Scripted::Json(other)] {
            let script = Script::new(vec![event_read(TAIL, answer)]);
            assert_eq!(tail_check(&script, &held).await, TailCheck::Gone);
        }
    }

    /// One plan, one tail id (A29): each send mints its own, and the token doesn't read it.
    #[tokio::test]
    async fn every_new_plan_mints_a_new_tail_id() {
        let host = TestHost::holding(&october());
        let target = target(&monday_19());
        let script = Script::new(Vec::new());
        let mut ctx = writer(&script, &host, &target);
        ctx.new_series_id = plan::client_event_id;
        let mut ids = Vec::new();
        let mut tokens = Vec::new();
        for _ in 0..2 {
            let reads = Script::new(october_reads());
            let planning = match prepare_change(
                &reads,
                &target,
                EditingStatus::On,
                &mirror(&["abc"]),
                &edit(title("Retro")),
                Some((&monday_19(), Some(&weekly()))),
            )
            .await
            {
                Prepared::Ready(planning) => planning,
                _ => panic!("not ready"),
            };
            let plan = planned_or_refused(&ctx, &planning, &edit(title("Retro")), Scope::Following)
                .unwrap();
            ids.push(new_series(&plan).unwrap().to_string());
            tokens.push(plan.choice_digest(&planning.bind));
        }
        assert!(ids[0] != ids[1]);
        assert!(tokens[0] == tokens[1]);
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

    /// Review round 2: a "this and following" delete floors what it removes at the cut's own
    /// `updated`, so a sync already running that lists an occurrence it cancelled, at the version
    /// changed in Google since the last sync, can't bring it back.
    #[tokio::test]
    async fn a_cut_delete_floors_its_removals_at_the_cuts_reply() {
        let mut events = october();
        events.push(off_site());
        let host = TestHost::holding(&events);
        let changed = with(
            written(
                &off_site(),
                &[("summary", json!("Off-site (room 2)"))],
                "\"o1\"",
            ),
            "updated",
            json!("2026-10-17T11:00:00.000Z"),
        );
        let reads_now = || {
            reads(
                &monday_19(),
                &weekly(),
                vec![weekly(), changed.clone()],
                vec![monday_19(), plain("2026-10-26T09:00:00Z"), changed.clone()],
            )
        };
        let cancelled = written(&changed, &[("status", json!("cancelled"))], "\"o2\"");
        let mut steps = reads_now();
        steps.extend(reads_now());
        steps.extend([
            series_patch(Scripted::Json(cut_series(vec![changed.clone()]))),
            Step::write(
                Method::Patch,
                "/events/abc_20261102T090000Z",
                "\"o1\"",
                Scripted::Json(cancelled.clone()),
            ),
            refetch_read(Scripted::Json(page(
                vec![october()[0].clone(), cancelled],
                None,
            ))),
        ]);
        let script = Script::new(steps);
        let before = Instant::now();
        let outcome = held_delete(&host, &script, &monday_19(), Scope::Following).await;
        script.asked_all();
        assert_eq!(outcome, saved());
        assert_eq!(host.ids(), ["cal:abc_20261012T080000Z"]);
        let row = "cal:abc_20261102T090000Z".to_string();
        assert!(!ids_of(host.synced(std::slice::from_ref(&changed), before)).contains(&row));
        let later = with(
            with(changed, "etag", json!("\"o3\"")),
            "updated",
            json!("2026-10-17T12:00:01.000Z"),
        );
        assert!(ids_of(host.synced(&[later], Instant::now())).contains(&row));
    }

    /// Review round 2: a whole-series delete proved without a reply (a lost answer, then a 404) is
    /// floored at the series' copies alone, so each occurrence changed on its own counts.
    #[tokio::test]
    async fn a_series_delete_proved_without_a_reply_is_floored_at_its_changed_occurrences() {
        let host = TestHost::holding(&october());
        let demo = written(
            &plain("2026-10-26T09:00:00Z"),
            &[("summary", json!("Demo"))],
            "\"d2\"",
        );
        let reads_now = || {
            reads(
                &monday_19(),
                &weekly(),
                vec![weekly(), demo.clone()],
                vec![monday_19(), demo.clone()],
            )
        };
        let mut steps = reads_now();
        steps.extend(reads_now());
        steps.extend([
            Step::write(
                Method::Delete,
                "/events/abc",
                "\"m1\"",
                Scripted::Lost { retried: false },
            ),
            Step::status("/events/abc", "", 404),
            refetch_read(Scripted::Status(410)),
        ]);
        let script = Script::new(steps);
        let before = Instant::now();
        assert_eq!(
            held_delete(&host, &script, &monday_19(), Scope::All).await,
            saved()
        );
        script.asked_all();
        assert!(host.ids().is_empty());
        assert!(host.synced(&[demo], before).is_empty());
    }

    /// Review round 2: a "this event" edit that meets its occurrence gone takes that row alone off,
    /// at the version planned from; the series is still there, and so is the editor.
    #[tokio::test]
    async fn a_this_edit_that_meets_its_occurrence_gone_takes_only_that_row() {
        let host = TestHost::holding(&october());
        let instance = monday_19();
        let mut steps = october_reads();
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            instance["etag"].as_str().unwrap(),
            Scripted::Status(404),
        ));
        let script = Script::new(steps);
        let (outcome, session) = save(
            &host,
            &script,
            &instance,
            &weekly(),
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert_eq!(outcome, WriteOutcome::Gone);
        assert_eq!(
            host.ids(),
            ["cal:abc_20261012T080000Z", "cal:abc_20261026T090000Z"]
        );
        assert_eq!(host.told(), ["cal"]);
        assert!(open_session(&host.edit, &session).unwrap().is_some());
        // Restored in Google afterwards, under another etag: it shows again.
        let restored = with(
            with(instance, "etag", json!("\"r9\"")),
            "updated",
            json!("2026-10-17T12:00:00.000Z"),
        );
        let shown = ids_of(host.synced(&[restored], Instant::now()));
        assert!(shown.contains(&"cal:abc_20261019T080000Z".to_string()));
    }

    /// An "all events" delete of Monday 19-10, planned in `host` and held under the token "t".
    async fn planned(host: &TestHost) -> HeldSeriesDelete {
        let instance = monday_19();
        let target = target(&instance);
        let seen = seen_by_user(&fresh_event(instance));
        let script = Script::new(october_reads());
        let ctx = writer(&script, host, &target);
        let (_, plan) = plan_held_delete(&ctx, &target.row_id, &seen, Scope::All, None)
            .await
            .unwrap();
        HeldSeriesDelete {
            token: "t".into(),
            event_id: target.row_id,
            seen,
            scope: Scope::All,
            plan,
        }
    }

    /// The claim a held delete of Monday 19-10 keeps through its wait and its send, as the shell's
    /// handed-off claim does.
    fn held_claim(host: &TestHost) -> InFlight<'_> {
        let keys = [series_key(CALENDAR, "abc"), target(&monday_19()).row_id];
        InFlight::claim_all(&host.edit, &keys).unwrap()
    }

    /// Review round 2: the held series delete's end of wait. An Undo first sends nothing; the wait
    /// first takes the entry, so a late Undo is told it's too late and the delete goes; editing
    /// turned off meanwhile stops it; an entry gone (PM quitting) sends nothing.
    #[tokio::test]
    async fn a_held_series_delete_ends_its_wait_as_whoever_took_it_says() {
        let instance = monday_19();
        let target = target(&instance);
        let hold = |host: &TestHost| {
            host.edit
                .held_deletes
                .hold(
                    "t",
                    &target.row_id,
                    "Standup",
                    Vec::new(),
                    Instant::now() + UNDO,
                )
                .unwrap()
        };

        // Undone first, with editing on or off: nothing is sent and nothing is said.
        for editing in [EditingStatus::On, EditingStatus::Paused] {
            let host = TestHost::holding(&october());
            let held = planned(&host).await;
            let _claim = held_claim(&host);
            let _rx = hold(&host);
            *host.editing.lock().unwrap() = editing;
            assert!(host.edit.held_deletes.cancel("t"));
            let silent = Script::new(Vec::new());
            assert_eq!(
                finish_held_series_delete(&silent, &*host, &target, &held, true).await,
                None,
                "{editing:?}"
            );
            silent.asked_all();
        }

        // The wait first: an Undo during the send is too late, and the delete goes, under the claim.
        let host = TestHost::holding(&october());
        let held = planned(&host).await;
        let _claim = held_claim(&host);
        let _rx = hold(&host);
        let undoing = Arc::clone(&host);
        let mut steps = october_reads();
        steps[0] = instance_read(&instance).then(move || {
            assert!(!undoing.edit.held_deletes.cancel("t"), "too late to undo");
        });
        steps.push(Step::write(
            Method::Delete,
            "/events/abc",
            "\"m1\"",
            Scripted::Status(204),
        ));
        steps.push(refetch_read(Scripted::Status(410)));
        let script = Script::new(steps);
        assert_eq!(
            finish_held_series_delete(&script, &*host, &target, &held, false).await,
            Some(saved())
        );
        script.asked_all();
        assert!(host.edit.held_deletes.list().is_empty());
        assert!(host.ids().is_empty());

        // Editing turned off during the wait: the entry is taken all the same (a reloaded webview
        // mustn't get it back), and nothing is sent.
        let host = TestHost::holding(&october());
        let held = planned(&host).await;
        let _claim = held_claim(&host);
        let _rx = hold(&host);
        *host.editing.lock().unwrap() = EditingStatus::Paused;
        let silent = Script::new(Vec::new());
        assert_eq!(
            finish_held_series_delete(&silent, &*host, &target, &held, false).await,
            Some(WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::EditingOff
            })
        );
        silent.asked_all();
        assert!(host.edit.held_deletes.list().is_empty());
        assert_eq!(host.ids(), ids_of(rows_of(&october())));

        // The wait over but its entry gone first (an Undo that got there, or PM quitting): nothing
        // goes and nothing is said, editing on or off.
        for editing in [EditingStatus::On, EditingStatus::Paused] {
            let host = TestHost::holding(&october());
            let held = planned(&host).await;
            let _claim = held_claim(&host);
            let _rx = hold(&host);
            *host.editing.lock().unwrap() = editing;
            host.edit.abandon_held_deletes();
            let silent = Script::new(Vec::new());
            assert_eq!(
                finish_held_series_delete(&silent, &*host, &target, &held, false).await,
                None,
                "{editing:?}"
            );
            silent.asked_all();
        }
    }

    // --- one event, unscoped (review round 2: H1's routing through the shells' own paths) ---

    fn dentist() -> Value {
        json!({
            "id": "solo", "etag": "\"s1\"", "summary": "Dentist", "status": "confirmed",
            "updated": "2026-10-01T10:00:00.000Z",
            "organizer": { "self": true },
            "start": { "dateTime": "2026-10-12T09:00:00+01:00" },
            "end": { "dateTime": "2026-10-12T10:00:00+01:00" }
        })
    }

    /// A one-event delete whose answer never came looks: gone means it landed, still there means
    /// it didn't; one never sent says why and doesn't look.
    #[tokio::test]
    async fn an_unscoped_delete_looks_after_a_lost_answer_and_not_after_an_unsent_one() {
        let event = dentist();
        let target = target(&event);
        let seen = seen_by_user(&fresh_event(event.clone()));
        let delete =
            |answer: Scripted| Step::write(Method::Delete, "/events/solo", "\"s1\"", answer);
        let moved = written(&event, &[("description", json!("Bring forms"))], "\"s3\"");
        let cases = [
            (
                vec![
                    delete(Scripted::Lost { retried: false }),
                    Step::status("/events/solo", "", 404),
                ],
                saved(),
                true,
            ),
            (
                vec![
                    delete(Scripted::Lost { retried: true }),
                    Step::get("/events/solo", "", event.clone()),
                ],
                WriteOutcome::Failed {
                    message: "PM lost touch with Google, and the event is still there. \
                              Nothing was deleted; try again."
                        .into(),
                },
                false,
            ),
            (
                vec![delete(Scripted::Unsent(Unsent::Offline))],
                WriteOutcome::Failed {
                    message: OFFLINE.into(),
                },
                false,
            ),
            // A server error, then the retry refused (a first copy that landed late makes it a
            // 412): look, and gone means it landed.
            (
                vec![
                    delete(Scripted::Reply {
                        status: 412,
                        body: None,
                        date: None,
                        maybe_applied: true,
                    }),
                    Step::status("/events/solo", "", 404),
                ],
                saved(),
                true,
            ),
            // Still there, changed elsewhere outside what the user saw: the 412 gets its one fresh
            // round, on Google's new etag.
            (
                vec![
                    delete(Scripted::Reply {
                        status: 412,
                        body: None,
                        date: None,
                        maybe_applied: true,
                    }),
                    Step::get("/events/solo", "", moved.clone()),
                    Step::get("/events/solo", "", moved.clone()),
                    Step::write(
                        Method::Delete,
                        "/events/solo",
                        "\"s3\"",
                        Scripted::Status(204),
                    ),
                ],
                saved(),
                true,
            ),
        ];
        for (answers, expected, removed) in cases {
            let host = TestHost::holding(std::slice::from_ref(&event));
            let mut steps = vec![Step::get("/events/solo", "", event.clone())];
            steps.extend(answers);
            let script = Script::new(steps);
            assert_eq!(send_delete(&script, &*host, &target, &seen).await, expected);
            script.asked_all();
            let row = host
                .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
                .unwrap();
            assert_eq!(row.is_none(), removed, "{expected:?}");
        }
        // Editing turned off during the Undo wait: read again at the send, so nothing goes.
        let host = TestHost::holding(std::slice::from_ref(&event));
        *host.editing.lock().unwrap() = EditingStatus::Paused;
        let script = Script::new(vec![Step::get("/events/solo", "", event.clone())]);
        assert_eq!(
            send_delete(&script, &*host, &target, &seen).await,
            WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::EditingOff
            }
        );
        script.asked_all();
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
            .unwrap();
        assert!(row.is_some());
    }

    /// A one-event save whose answer never came looks: holding the change, it landed; not, it didn't.
    /// One never sent says why and doesn't look.
    #[tokio::test]
    async fn an_unscoped_save_looks_after_a_lost_answer_and_not_after_an_unsent_one() {
        let event = dentist();
        let target = target(&event);
        let patch = |answer: Scripted| Step::write(Method::Patch, "/events/solo", "\"s1\"", answer);
        let renamed = retitled(&event, "\"s2\"");
        let moved = written(&event, &[("location", json!("Room 2"))], "\"s3\"");
        let cases = [
            (
                vec![
                    patch(Scripted::Lost { retried: false }),
                    Step::get("/events/solo", "", renamed.clone()),
                ],
                saved(),
                "Retro",
            ),
            (
                vec![
                    patch(Scripted::Lost { retried: false }),
                    Step::get("/events/solo", "", event.clone()),
                ],
                WriteOutcome::Failed {
                    message: "PM lost touch with Google, and your change isn't there. Nothing \
                              was saved; try again."
                        .into(),
                },
                "Dentist",
            ),
            (
                vec![patch(Scripted::Unsent(Unsent::SignIn))],
                WriteOutcome::Reauth,
                "Dentist",
            ),
            // A server error, then the retry refused: look first. Holding the change, it landed;
            // not, the 412 gets its one fresh round, and that copy lands.
            (
                vec![
                    patch(Scripted::Reply {
                        status: 412,
                        body: None,
                        date: None,
                        maybe_applied: true,
                    }),
                    Step::get("/events/solo", "", renamed.clone()),
                ],
                saved(),
                "Retro",
            ),
            // (Changed elsewhere meanwhile, in a field this save doesn't send: the fresh round goes
            // out on Google's new etag.)
            (
                vec![
                    patch(Scripted::Reply {
                        status: 412,
                        body: None,
                        date: None,
                        maybe_applied: true,
                    }),
                    Step::get("/events/solo", "", moved.clone()),
                    Step::get("/events/solo", "", moved.clone()),
                    Step::write(
                        Method::Patch,
                        "/events/solo",
                        "\"s3\"",
                        Scripted::Json(renamed.clone()),
                    ),
                ],
                saved(),
                "Retro",
            ),
        ];
        for (answers, expected, title_now) in cases {
            let host = TestHost::holding(std::slice::from_ref(&event));
            let session = keep_session(&host.edit, target.clone(), event.clone(), None).unwrap();
            let open = open_session(&host.edit, &session).unwrap().unwrap();
            let mut steps = vec![Step::get("/events/solo", "", event.clone())];
            steps.extend(answers);
            let script = Script::new(steps);
            let outcome = save_event(
                &script,
                &*host,
                &session,
                &open,
                &title("Retro"),
                EditingStatus::On,
            )
            .await;
            script.asked_all();
            assert_eq!(outcome, expected);
            let row = host
                .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
                .unwrap()
                .unwrap();
            assert_eq!(row.summary, title_now, "{expected:?}");
        }
    }

    // --- a one-off Google has since turned into a repeating event (C9 design critique 2 M6) ---

    /// The dentist one-off as Google holds it once a weekly rule was added there: a series itself,
    /// on the same first day, so what the user saw of the one-off still matches it.
    fn dentist_turned_series() -> Value {
        with(dentist(), "recurrence", json!(["RRULE:FREQ=WEEKLY"]))
    }

    #[tokio::test]
    async fn a_row_turned_series_is_read_only_in_the_editor() {
        let turned = dentist_turned_series();
        let edit_state = CalendarEditState::default();
        // No `recurringEventId`, so nothing more is read.
        let script = Script::new(Vec::new());
        let load = open_editor(
            &script,
            &edit_state,
            target(&turned),
            turned.clone(),
            EditingStatus::On,
            None,
        )
        .await
        .unwrap();
        script.finished();
        let EditLoad::Ready { permissions, .. } = load else {
            panic!("the editor didn't open");
        };
        assert!(
            permissions == FieldPermissions::none(ReadOnlyReason::BecameRepeating),
            "read-only, with its own reason"
        );
    }

    #[tokio::test]
    async fn a_save_on_a_row_turned_series_sends_nothing() {
        let (event, turned) = (dentist(), dentist_turned_series());
        let target = target(&event);
        let host = TestHost::holding(std::slice::from_ref(&event));
        let session = keep_session(&host.edit, target.clone(), event.clone(), None).unwrap();
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        let script = Script::new(vec![Step::get("/events/solo", "", turned)]);
        let outcome = save_event(
            &script,
            &*host,
            &session,
            &open,
            &title("Retro"),
            EditingStatus::On,
        )
        .await;
        script.finished();
        assert!(matches!(
            outcome,
            WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::BecameRepeating
            }
        ));
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert_eq!(script.sent().len(), 1);
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
            .unwrap()
            .unwrap();
        assert_eq!(row.summary, "Dentist");
    }

    /// Critique M6's own case: a one-off deleted in PM, turned into a series in Google during the
    /// Undo wait, on the same first day. The gate stops it first, with its own reason; "is it still
    /// what you saw" would refuse it too (no series is a row PM mirrors), but as a changed start.
    #[tokio::test]
    async fn a_held_delete_of_a_row_turned_series_deletes_nothing() {
        let (event, turned) = (dentist(), dentist_turned_series());
        let target = target(&event);
        let seen = seen_by_user(&fresh_event(event.clone()));
        assert!(
            patch::delete_still_matches(&seen, &turned) != DeleteCheck::Matches,
            "the backstop behind the gate"
        );
        let host = TestHost::holding(std::slice::from_ref(&event));
        let script = Script::new(vec![Step::get("/events/solo", "", turned)]);
        let outcome = send_delete(&script, &*host, &target, &seen).await;
        script.finished();
        assert!(matches!(
            outcome,
            WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::BecameRepeating
            }
        ));
        assert_eq!(script.sent().len(), 1);
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
            .unwrap();
        assert!(row.is_some());
    }

    // --- an unscoped write on an occurrence asks which occurrences (R9, C9b-1b) ---

    /// The dentist as one occurrence of a weekly event you organise.
    fn dentist_occurrence() -> Value {
        with(dentist(), "recurringEventId", json!("weekly"))
    }

    #[tokio::test]
    async fn an_unscoped_save_on_an_occurrence_asks_which_occurrences() {
        let event = dentist_occurrence();
        let target = target(&event);
        let host = TestHost::holding(std::slice::from_ref(&event));
        let session = keep_session(&host.edit, target.clone(), event.clone(), None).unwrap();
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        let script = Script::new(vec![Step::get("/events/solo", "", event)]);
        let outcome = save_event(
            &script,
            &*host,
            &session,
            &open,
            &title("Retro"),
            EditingStatus::On,
        )
        .await;
        script.finished();
        assert!(
            matches!(outcome, WriteOutcome::NeedsChoice),
            "asks rather than saving this event alone"
        );
        assert_eq!(script.sent().len(), 1);
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
            .unwrap()
            .unwrap();
        assert_eq!(row.summary, "Dentist");
    }

    /// C11 critique 2 L6: until C11d makes a one-off repeat, a draft that asks it to is refused,
    /// never saved without its repeat; only a GET goes out.
    #[tokio::test]
    async fn a_one_off_refuses_a_repeat_until_it_can_make_one() {
        let event = dentist();
        let target = target(&event);
        let host = TestHost::holding(std::slice::from_ref(&event));
        let session = keep_session(&host.edit, target.clone(), event.clone(), None).unwrap();
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        for recurrence in [
            crate::calendar_write::dto::RepeatDraft::Once,
            crate::calendar_write::dto::RepeatDraft::Rule { rule: every_day() },
        ] {
            let script = Script::new(vec![Step::get("/events/solo", "", event.clone())]);
            let draft = EventPatchDraft {
                recurrence: Some(recurrence),
                ..title("Retro")
            };
            let outcome =
                save_event(&script, &*host, &session, &open, &draft, EditingStatus::On).await;
            script.finished();
            assert!(
                matches!(&outcome, WriteOutcome::Failed { message } if message == ONE_OFF_REPEAT),
                "refused in words"
            );
            assert_eq!(script.sent().len(), 1);
        }
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
            .unwrap()
            .unwrap();
        assert_eq!(row.summary, "Dentist");
    }

    /// A read-only reason outranks the choice: asking which occurrences of an event PM can't
    /// change would lead nowhere.
    #[tokio::test]
    async fn an_unscoped_save_on_a_read_only_occurrence_says_why_first() {
        let event = with(
            dentist_occurrence(),
            "attendees",
            json!([{ "email": "guest@x.com" }]),
        );
        let target = target(&event);
        let host = TestHost::holding(std::slice::from_ref(&event));
        let session = keep_session(&host.edit, target.clone(), event.clone(), None).unwrap();
        let open = open_session(&host.edit, &session).unwrap().unwrap();
        let script = Script::new(vec![Step::get("/events/solo", "", event)]);
        let outcome = save_event(
            &script,
            &*host,
            &session,
            &open,
            &title("Retro"),
            EditingStatus::On,
        )
        .await;
        script.finished();
        assert!(matches!(
            outcome,
            WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::HasGuests
            }
        ));
        assert_eq!(script.sent().len(), 1);
    }

    /// A held delete with no choice whose row turned out to be an occurrence (the mirror row looked
    /// like a one-off when it was held) sends nothing: the user deletes again and chooses.
    #[tokio::test]
    async fn an_unscoped_delete_on_an_occurrence_asks_which_occurrences() {
        let event = dentist_occurrence();
        let target = target(&event);
        let seen = seen_by_user(&fresh_event(event.clone()));
        let host = TestHost::holding(std::slice::from_ref(&event));
        let script = Script::new(vec![Step::get("/events/solo", "", event)]);
        let outcome = send_delete(&script, &*host, &target, &seen).await;
        script.finished();
        assert!(
            matches!(outcome, WriteOutcome::NeedsChoice),
            "asks rather than deleting this event alone"
        );
        assert_eq!(script.sent().len(), 1);
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        let row = host
            .with_conn(|conn| calendar::event_by_id(conn, "cal:solo"))
            .unwrap();
        assert!(row.is_some());
    }

    // --- the release gate, and the words rows show (C9b-1 critiques 1 M8, 2 M4) ---

    /// The one release gate for a scoped write without its choice: every shell's series write takes
    /// [`MISSING_CHOICE_OK`] (a settle, which plans nothing anew, `false`), and nothing spreads
    /// another value in. Read from this file's own text, so a later `missing_ok: true` fails here.
    #[test]
    fn every_shell_takes_the_one_release_gate_for_a_missing_choice() {
        let source = include_str!("calendar_edit.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        let mut gated = 0;
        let mut literals = 0;
        for (at, _) in shipped.match_indices("SeriesWrite {") {
            literals += 1;
            let body = &shipped[at..];
            let body = &body[..body.find("};").unwrap_or(body.len())];
            assert!(
                !body.contains(".."),
                "a series write spreads another's fields"
            );
            if body.contains("missing_ok: MISSING_CHOICE_OK,") {
                gated += 1;
            } else {
                assert!(
                    body.contains("missing_ok: false,"),
                    "a series write sets its own missing-choice rule"
                );
            }
        }
        assert_eq!((literals, gated), (4, 3), "the three shells and the settle");
        // And the gate itself is a dev build's only.
        assert!(
            shipped.contains("const MISSING_CHOICE_OK: bool = cfg!(debug_assertions);"),
            "only a dev build may leave out the choice"
        );
    }

    /// No row, banner or settle answer says "series", "split", "instance", "exception" or "master":
    /// every refusal a row can show, nested ones included, and the two banners that said it.
    #[test]
    fn what_rows_show_has_no_jargon() {
        use crate::calendar_recur::lines::TextOnly;
        use crate::calendar_recur::reanchor::ReanchorError;
        use crate::calendar_recur::remap::RetimeError;
        use crate::calendar_recur::spec::SpecError;
        use crate::calendar_recur::RecurError;
        use crate::calendar_write::time::TimeError;
        use chrono::{NaiveDate, Weekday};
        let day = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let text_only = [
            TextOnly::NoRule,
            TextOnly::SeveralRules,
            TextOnly::ExRule,
            TextOnly::SubDaily,
            TextOnly::Part("BYSETPOS".into()),
            TextOnly::Unreadable,
        ];
        let recur: Vec<RecurError> = text_only
            .iter()
            .cloned()
            .map(RecurError::TextOnly)
            .chain([
                RecurError::Ended,
                RecurError::CannotCount,
                RecurError::BeforeStart,
                RecurError::Mismatch,
            ])
            .collect();
        let reanchor = [
            ReanchorError::Collision(Weekday::Tue),
            ReanchorError::Crosses(Weekday::Tue),
            ReanchorError::CrossWeek,
            ReanchorError::CrossMonth,
            ReanchorError::CrossYear,
            ReanchorError::Fifth,
            ReanchorError::NotLast,
            ReanchorError::NotDerivable,
        ];
        let retime: Vec<RetimeError> = recur
            .iter()
            .cloned()
            .map(RetimeError::Recur)
            .chain(reanchor.iter().cloned().map(RetimeError::Reanchor))
            .chain([
                RetimeError::KindChange,
                RetimeError::HasExtraDates,
                RetimeError::NotOnRule,
                RetimeError::StartNotOnRule(day),
                RetimeError::RepeatedTime(day),
                RetimeError::Gap(day),
                RetimeError::Overlaps(day),
                RetimeError::Loses(day),
                RetimeError::Adds(day),
                RetimeError::CannotPlace,
            ])
            .collect();
        let time = [
            TimeError::NoZone,
            TimeError::UnknownZone("Mars/Base".into()),
            TimeError::BadDate("x".into()),
            TimeError::BadTime("x".into()),
            TimeError::Gap {
                zone: "Europe/London".into(),
                wall: "01:30".into(),
            },
            TimeError::EndBeforeStart,
        ];
        let refusals: Vec<SeriesRefusal> = recur
            .into_iter()
            .map(SeriesRefusal::Recur)
            .chain(retime.into_iter().map(SeriesRefusal::Retime))
            .chain(time.into_iter().map(SeriesRefusal::Time))
            .chain([
                SeriesRefusal::Unreadable,
                SeriesRefusal::NoZone,
                SeriesRefusal::NotInSeries,
                SeriesRefusal::Incomplete,
                SeriesRefusal::NoBase,
                SeriesRefusal::CannotCarry(day),
                SeriesRefusal::GuestsElsewhere(day),
                SeriesRefusal::FormattedDescription,
                SeriesRefusal::VisibilityIsSeriesWide,
                SeriesRefusal::OnlyThisDiffers,
                SeriesRefusal::OnlyThisTimeDiffers,
                SeriesRefusal::SplitInGap(day),
                SeriesRefusal::ZoneChange,
                SeriesRefusal::SplitOverlap(day),
                SeriesRefusal::PartsUnread,
                SeriesRefusal::RuleIsSeriesWide,
                SeriesRefusal::RepeatsPastPart(day),
                SeriesRefusal::LaterPartUnread,
                SeriesRefusal::NothingToKeep,
                SeriesRefusal::KeptHasOwn(day),
            ])
            // Every repeat the editor can't save, as a row repeats it (C11 §9.8).
            .chain(
                [
                    SpecError::Interval,
                    SpecError::NoWeekdays,
                    SpecError::DayOfMonth,
                    SpecError::Month,
                    SpecError::Count,
                    SpecError::TooMany,
                    SpecError::EndsBeforeStart,
                    SpecError::NoOccurrence,
                    SpecError::Gap(day),
                    SpecError::EndGap(day),
                    SpecError::BadDate,
                ]
                .into_iter()
                .map(SeriesRefusal::Spec),
            )
            .collect();
        let reasons = [
            ReadOnlyReason::NotGoogle,
            ReadOnlyReason::EditingOff,
            ReadOnlyReason::CalendarReadOnly,
            ReadOnlyReason::PrivateEvent,
            ReadOnlyReason::NotOrganizer,
            ReadOnlyReason::BecameRepeating,
            ReadOnlyReason::HasGuests,
            ReadOnlyReason::SpecialType,
            ReadOnlyReason::Locked,
            ReadOnlyReason::HtmlDescription,
        ];
        let mut lines: Vec<String> = refusals.iter().map(ToString::to_string).collect();
        lines.extend(reasons.iter().map(|r| read_only_message(*r).to_string()));
        lines.extend([PUT_BACK.to_string(), RETIME_UNSURE.to_string()]);
        for line in &lines {
            let lower = line.to_lowercase();
            for word in ["series", "split", "instance", "exception", "master"] {
                assert!(!lower.contains(word), "“{line}” says “{word}”");
            }
            // Straight quotes only around nothing: titles and labels take curly ones.
            assert!(
                !line.contains("\"This event\""),
                "“{line}” has straight quotes"
            );
        }
        // Dates as the design system writes them, DD-MM-YYYY.
        assert!(SeriesRefusal::CannotCarry(day)
            .to_string()
            .contains("19-10-2026"));
        for dated in [
            SeriesRefusal::RepeatsPastPart(day),
            SeriesRefusal::KeptHasOwn(day),
        ] {
            assert!(dated.to_string().contains("19-10-2026"));
        }
        // A row refused for a reason says that reason, never just "can't change this event".
        let reason_lines: Vec<&str> = reasons.iter().map(|r| read_only_message(*r)).collect();
        assert!(reason_lines
            .iter()
            .all(|l| *l != "PM can't change this event."));
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
        // A settle reaches a whole new part and a cut series: it fails closed too (C9a-3a
        // critique 9).
        assert_eq!(
            dev_allow::decide(None, None, "home@group", dev_allow::SETTLE_SERIES_WIDE),
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

    // --- creating an event (C10) ---

    use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern, RepeatRule};

    /// The id PM picks for the new event in these tests.
    const NEW: &str = "fedcba9876543210fedcba9876543210";

    /// Dentist on Monday 26-10, 10:00 to 11:00 in London: inside the tests' mirror window.
    fn dentist_draft() -> NewEventDraft {
        on_day("2026-10-26")
    }

    fn on_day(day: &str) -> NewEventDraft {
        NewEventDraft {
            summary: "Dentist".into(),
            time: TimeDraft::Timed {
                start_date: day.into(),
                start_time: "10:00".into(),
                start_zone: "Europe/London".into(),
                end_date: day.into(),
                end_time: "11:00".into(),
                end_zone: "Europe/London".into(),
            },
            location: String::new(),
            description: String::new(),
            show_as: None,
            visibility: None,
            recurrence: None,
        }
    }

    /// The dentist from `day`, weekly on its Monday, `count` times.
    fn weekly_dentist(day: &str, count: u16) -> NewEventDraft {
        NewEventDraft {
            recurrence: Some(RepeatRule {
                pattern: RepeatPattern::Weekly {
                    days: vec![DayCode::Mon],
                },
                interval: 1,
                ends: RepeatEnds::After { count },
            }),
            ..on_day(day)
        }
    }

    /// The tests' calendar "cal" ([`CALENDAR`] at Google), as the create command resolves it.
    fn new_target() -> CalendarTarget {
        CalendarTarget {
            calendar_id: "cal".into(),
            remote_calendar: CALENDAR.into(),
            email: "me@example.com".into(),
            token_key: "key".into(),
            facts: CalendarFacts {
                access_role: Some("owner".into()),
                time_zone: Some("Europe/London".into()),
                ..Default::default()
            },
            selected: true,
        }
    }

    /// A create through `script` against `host`, editing as `editing` says, the new event under
    /// [`NEW`] and its retry token "ref-1".
    fn creator<'a>(
        script: &'a Script,
        host: &'a TestHost,
        target: &'a CalendarTarget,
        editing: EditingStatus,
    ) -> CreateWrite<'a, Script, TestHost> {
        CreateWrite {
            wire: script,
            host,
            target,
            editing,
            new_id: || NEW.to_string(),
            new_ref: || "ref-1".to_string(),
        }
    }

    /// Google's copy of the new event `draft` makes, at `etag`.
    fn google_copy(draft: &NewEventDraft, etag: &str) -> Value {
        let mut v = Value::Object(patch::insert_body(draft).unwrap());
        v["kind"] = json!("calendar#event");
        v["id"] = json!(NEW);
        v["etag"] = json!(etag);
        v["status"] = json!("confirmed");
        v["iCalUID"] = json!("new@google.com");
        v["updated"] = json!(SAVED_AT);
        v
    }

    fn new_read(answer: Scripted) -> Step {
        event_read(NEW, answer)
    }

    fn absent() -> Scripted {
        Scripted::Status(404)
    }

    /// Create `draft` (or retry `token`) on `target` through `steps`, which must all be asked for.
    async fn create_on(
        host: &TestHost,
        target: &CalendarTarget,
        editing: EditingStatus,
        steps: Vec<Step>,
        draft: NewEventDraft,
        token: Option<&str>,
    ) -> (WriteOutcome, Script) {
        let script = Script::new(steps);
        let outcome = create_event(&creator(&script, host, target, editing), draft, token).await;
        script.asked_all();
        (outcome, script)
    }

    /// The dentist created on "cal", editing on.
    async fn create(
        host: &TestHost,
        steps: Vec<Step>,
        token: Option<&str>,
    ) -> (WriteOutcome, Script) {
        create_on(
            host,
            &new_target(),
            EditingStatus::On,
            steps,
            dentist_draft(),
            token,
        )
        .await
    }

    /// A create whose answer is lost and whose event two looks don't find; posted again under
    /// the same id, that answer is lost too: held as "ref-1", two POSTs spent.
    async fn lost_create(host: &TestHost) -> WriteOutcome {
        let (outcome, _) = create(
            host,
            vec![
                insert_step(lost()),
                new_read(absent()),
                new_read(absent()),
                insert_step(lost()),
                new_read(absent()),
            ],
            None,
        )
        .await;
        outcome
    }

    fn held_creates(host: &TestHost) -> usize {
        host.edit.creates.lock().unwrap().len()
    }

    fn held_posts(host: &TestHost, token: &str) -> Option<usize> {
        host.edit
            .creates
            .lock()
            .unwrap()
            .get(token)
            .map(|h| h.posts)
    }

    /// Say the create `token` names was last sent `at`, to age it without waiting.
    fn set_held_at(host: &TestHost, token: &str, at: Instant) {
        if let Some(held) = host.edit.creates.lock().unwrap().get_mut(token) {
            held.at = at;
        }
    }

    fn new_row(host: &TestHost) -> Option<CalendarEvent> {
        host.with_conn(|conn| calendar::event_by_id(conn, &format!("cal:{NEW}")))
            .unwrap()
    }

    fn ambiguous(token: &str) -> WriteOutcome {
        WriteOutcome::Ambiguous {
            retry_token: token.into(),
        }
    }

    fn failed(message: &str) -> WriteOutcome {
        WriteOutcome::Failed {
            message: message.into(),
        }
    }

    fn inserts(script: &Script) -> Vec<RequestPlan> {
        script
            .sent()
            .into_iter()
            .filter(|r| r.method == Method::Insert)
            .collect()
    }

    #[tokio::test]
    async fn a_create_posts_under_pms_id_with_send_updates_none_and_lands_as_created() {
        let host = TestHost::holding(&[]);
        let copy = google_copy(&dentist_draft(), "\"n1\"");
        let (outcome, script) = create(&host, vec![insert_step(Scripted::Json(copy))], None).await;
        assert!(outcome == saved(), "the new event is saved");
        let sent = script.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].url.ends_with(&format!(
            "/calendars/{CALENDAR}/events?sendUpdates=none&conferenceDataVersion=1"
        )));
        let body = sent[0].body.as_ref().unwrap().as_object().unwrap();
        assert_eq!(body["id"], NEW);
        assert!(body
            .keys()
            .all(|k| k == "id" || patch::CREATABLE.contains(&k.as_str())));
        let row = new_row(&host).expect("the new event is in the mirror");
        assert!(row.summary == "Dentist" && row.start == "2026-10-26T10:00:00Z");
        assert_eq!(host.told(), vec!["cal".to_string()]);
        // A complete list that doesn't show it yet keeps it on screen: it was recorded as new.
        let shown = host.synced(&[], Instant::now());
        assert!(shown.iter().any(|r| r.id == format!("cal:{NEW}")));
        assert_eq!(held_creates(&host), 0);
    }

    #[tokio::test]
    async fn a_create_sends_no_guests_reminders_conference_or_notes() {
        let host = TestHost::holding(&[]);
        let draft = NewEventDraft {
            location: "High St".into(),
            description: "Bring the form".into(),
            show_as: Some(ShowAs::Free),
            visibility: Some(Visibility::Private),
            ..dentist_draft()
        };
        let copy = google_copy(&draft, "\"n1\"");
        let (outcome, script) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            vec![insert_step(Scripted::Json(copy))],
            draft,
            None,
        )
        .await;
        assert!(outcome == saved(), "saved");
        let body = script.sent()[0].body.clone().unwrap();
        for never in [
            "attendees",
            "reminders",
            "conferenceData",
            "extendedProperties",
            "colorId",
        ] {
            assert!(body.get(never).is_none(), "{never}");
        }
        // A rule goes only with a draft that repeats (C11).
        assert!(body.get("recurrence").is_none(), "no rule, no recurrence");
        assert_eq!(body["transparency"], "transparent");
        assert_eq!(body["visibility"], "private");
    }

    #[tokio::test]
    async fn a_create_the_account_or_calendar_cant_take_sends_nothing() {
        let host = TestHost::holding(&[]);
        let with_role = |role: &str| CalendarTarget {
            facts: CalendarFacts {
                access_role: Some(role.into()),
                ..Default::default()
            },
            ..new_target()
        };
        let private = NewEventDraft {
            visibility: Some(Visibility::Private),
            ..dentist_draft()
        };
        let cases = [
            (
                with_role("reader"),
                EditingStatus::On,
                dentist_draft(),
                ReadOnlyReason::CalendarReadOnly,
            ),
            (
                with_role("writerWithoutPrivateAccess"),
                EditingStatus::On,
                private,
                ReadOnlyReason::PrivateEvent,
            ),
            (
                new_target(),
                EditingStatus::Off,
                dentist_draft(),
                ReadOnlyReason::EditingOff,
            ),
            (
                new_target(),
                EditingStatus::NeedsConsent,
                dentist_draft(),
                ReadOnlyReason::EditingOff,
            ),
        ];
        for (target, editing, draft, reason) in cases {
            let (outcome, _) = create_on(&host, &target, editing, Vec::new(), draft, None).await;
            assert!(
                outcome == WriteOutcome::ReadOnly { reason },
                "refused with its reason, nothing sent"
            );
        }
        // A calendar PM doesn't sync lands nothing, so it takes no creates.
        let unsynced = CalendarTarget {
            selected: false,
            ..new_target()
        };
        let (outcome, _) = create_on(
            &host,
            &unsynced,
            EditingStatus::On,
            Vec::new(),
            dentist_draft(),
            None,
        )
        .await;
        assert!(
            outcome == failed(NOT_SYNCED),
            "an unsynced calendar is refused"
        );
        // A time PM can't save builds nothing to send.
        let (outcome, _) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            Vec::new(),
            on_day("2026-02-30"),
            None,
        )
        .await;
        assert!(matches!(outcome, WriteOutcome::Failed { .. }));
        assert_eq!(held_creates(&host), 0);
    }

    /// R10: a 403 to a create, with editing on, is the calendar refusing; never a consent loop.
    #[tokio::test]
    async fn a_403_to_a_create_reads_as_the_calendar_or_editing() {
        let host = TestHost::holding(&[]);
        let forbidden = |reason: &str| Scripted::Reply {
            status: 403,
            body: Some(json!({ "error": { "errors": [{ "reason": reason }] } })),
            date: None,
            maybe_applied: false,
        };
        for reason in ["requiredAccessLevel", "insufficientPermissions"] {
            let (outcome, script) = create(&host, vec![insert_step(forbidden(reason))], None).await;
            assert!(
                outcome
                    == WriteOutcome::ReadOnly {
                        reason: ReadOnlyReason::CalendarReadOnly
                    },
                "{reason}: the calendar's"
            );
            assert_eq!(script.sent().len(), 1);
        }
        // Without the write scope, the same refusal asks for editing to be turned on again.
        assert!(
            not_done_outcome(
                Why::Refused(Verdict::InsufficientScope),
                EditingStatus::NeedsConsent,
                false
            ) == WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::EditingOff
            },
            "editing's"
        );
        assert_eq!(held_creates(&host), 0);
    }

    #[tokio::test]
    async fn a_404_create_says_the_calendar_isnt_there() {
        let host = TestHost::holding(&[]);
        let (outcome, script) = create(&host, vec![insert_step(absent())], None).await;
        assert!(outcome == failed(NO_CALENDAR), "no calendar");
        assert_eq!(script.sent().len(), 1);
        assert!(script.waits().is_empty());
        assert_eq!(held_creates(&host), 0);
    }

    #[tokio::test]
    async fn an_unsent_create_sends_nothing_more() {
        let host = TestHost::holding(&[]);
        for (why, want) in [
            (Unsent::SignIn, WriteOutcome::Reauth),
            (Unsent::Offline, failed(OFFLINE)),
            (Unsent::Local, failed(NOT_SENT)),
        ] {
            let (outcome, script) =
                create(&host, vec![insert_step(Scripted::Unsent(why))], None).await;
            assert!(outcome == want, "nothing was sent, and it says so");
            assert_eq!(script.sent().len(), 1);
        }
        assert_eq!(held_creates(&host), 0);
        assert!(new_row(&host).is_none());
    }

    /// Google made it, but its copy isn't a row PM can mirror: saved, and the next sync shows it.
    #[tokio::test]
    async fn a_create_reply_pm_cant_parse_is_saved_pending() {
        let host = TestHost::holding(&[]);
        let unreadable = Scripted::Reply {
            status: 200,
            body: None,
            date: None,
            maybe_applied: false,
        };
        let bare = json!({ "id": NEW, "status": "confirmed", "etag": "\"n1\"" });
        let (outcome, script) = create(
            &host,
            vec![insert_step(unreadable), new_read(Scripted::Json(bare))],
            None,
        )
        .await;
        assert!(outcome == pending(), "saved, the mirror to catch up");
        assert_eq!(inserts(&script).len(), 1);
        assert!(host.hash_dropped());
        assert!(new_row(&host).is_none());
    }

    #[tokio::test]
    async fn a_create_outside_the_mirror_is_saved_but_not_landed() {
        let host = TestHost::holding(&[]);
        let draft = on_day("2028-01-10");
        let copy = google_copy(&draft, "\"n1\"");
        let (outcome, _) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            vec![insert_step(Scripted::Json(copy))],
            draft,
            None,
        )
        .await;
        assert!(
            outcome
                == WriteOutcome::Saved {
                    warnings: vec![WriteWarning::OutsideMirror],
                    left_behind: Vec::new(),
                    milestone_moves: None,
                },
            "saved in Google, and said to be outside"
        );
        assert!(new_row(&host).is_none());
        assert!(host.nothing_recorded());
        assert!(host.told().is_empty());
    }

    /// R5: the webview gets PM's own token, never the id PM chose for Google.
    #[tokio::test]
    async fn a_lost_create_is_held_under_a_retry_token_never_the_event_id() {
        let host = TestHost::holding(&[]);
        let outcome = lost_create(&host).await;
        assert!(outcome == ambiguous("ref-1"), "held as ref-1");
        let said = serde_json::to_string(&outcome).unwrap();
        assert!(
            !said.contains(NEW),
            "the event's id never reaches the webview"
        );
        assert_eq!(held_posts(&host, "ref-1"), Some(2));
        assert!(new_row(&host).is_none());
    }

    #[tokio::test]
    async fn a_retry_finds_the_event_and_lands_it_without_posting() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let copy = google_copy(&dentist_draft(), "\"n1\"");
        let (outcome, script) =
            create(&host, vec![new_read(Scripted::Json(copy))], Some("ref-1")).await;
        assert!(outcome == saved(), "found and landed");
        script.finished();
        assert_eq!(held_creates(&host), 0);
        assert!(new_row(&host).is_some());
    }

    #[tokio::test]
    async fn a_retry_finding_nothing_twice_posts_the_same_id_once() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let copy = google_copy(&dentist_draft(), "\"n1\"");
        let (outcome, script) = create(
            &host,
            vec![
                new_read(absent()),
                new_read(absent()),
                insert_step(Scripted::Json(copy)),
            ],
            Some("ref-1"),
        )
        .await;
        assert!(outcome == saved(), "sent again, and landed");
        assert_eq!(script.waits(), vec![RETRY_PAUSE]);
        let posts = inserts(&script);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].body.as_ref().unwrap()["id"], NEW);
        assert_eq!(held_creates(&host), 0);
        assert!(new_row(&host).is_some());
    }

    /// Made, then deleted in Google: gone, the token with it, nothing landed.
    #[tokio::test]
    async fn a_retry_finding_it_cancelled_says_gone() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let cancelled = with(
            google_copy(&dentist_draft(), "\"n2\""),
            "status",
            json!("cancelled"),
        );
        let (outcome, script) = create(
            &host,
            vec![new_read(Scripted::Json(cancelled))],
            Some("ref-1"),
        )
        .await;
        assert!(outcome == WriteOutcome::Gone, "gone");
        script.finished();
        assert_eq!(held_creates(&host), 0);
        assert!(new_row(&host).is_none());
    }

    #[tokio::test]
    async fn a_retry_whose_look_is_lost_keeps_the_token() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        for (look, want) in [
            (lost(), ambiguous("ref-1")),
            (Scripted::Status(503), ambiguous("ref-1")),
            (Scripted::Status(401), WriteOutcome::Reauth),
            (Scripted::Status(429), WriteOutcome::Busy),
        ] {
            let (outcome, script) = create(&host, vec![new_read(look)], Some("ref-1")).await;
            assert!(outcome == want, "kept, and said why");
            script.finished();
            assert_eq!(held_posts(&host, "ref-1"), Some(2));
        }
    }

    /// The re-send's answer lost too: held again under the same token, never a new one.
    #[tokio::test]
    async fn a_retry_that_is_unknown_again_keeps_the_same_token() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let script = Script::new(vec![
            new_read(absent()),
            new_read(absent()),
            insert_step(lost()),
            new_read(absent()),
        ]);
        let target = new_target();
        let ctx = CreateWrite {
            new_ref: || "ref-2".to_string(),
            ..creator(&script, &host, &target, EditingStatus::On)
        };
        let outcome = create_event(&ctx, dentist_draft(), Some("ref-1")).await;
        script.asked_all();
        assert!(outcome == ambiguous("ref-1"), "the same token");
        assert_eq!(held_creates(&host), 1);
        assert_eq!(held_posts(&host, "ref-1"), Some(3));
    }

    /// C10 critiques 1 M1, 2 H1: after two empty looks, a re-send that never left, or one answered
    /// 404, proves nothing about the first copy: still ambiguous, the token still held.
    #[tokio::test]
    async fn a_resend_unsent_or_answered_404_is_never_proof() {
        for answer in [
            Scripted::Unsent(Unsent::Offline),
            Scripted::Unsent(Unsent::SignIn),
            absent(),
            Scripted::Reply {
                status: 403,
                body: None,
                date: None,
                maybe_applied: false,
            },
        ] {
            let host = TestHost::holding(&[]);
            lost_create(&host).await;
            let (outcome, script) = create(
                &host,
                vec![
                    new_read(absent()),
                    new_read(absent()),
                    insert_step(answer),
                    new_read(absent()),
                ],
                Some("ref-1"),
            )
            .await;
            assert!(outcome == ambiguous("ref-1"), "still unknown");
            assert_eq!(inserts(&script).len(), 1);
            assert_eq!(held_creates(&host), 1);
        }
    }

    /// C10 critique 1 M5: one id gets three POSTs at most, across the first run and every retry;
    /// after that a retry only looks.
    #[tokio::test]
    async fn the_posts_under_one_id_are_bounded_across_retries() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let (outcome, script) = create(
            &host,
            vec![
                new_read(absent()),
                new_read(absent()),
                insert_step(lost()),
                new_read(absent()),
            ],
            Some("ref-1"),
        )
        .await;
        assert!(outcome == ambiguous("ref-1"), "held");
        assert_eq!(inserts(&script).len(), 1);
        assert_eq!(held_posts(&host, "ref-1"), Some(MAX_POSTS));
        let (outcome, script) = create(
            &host,
            vec![new_read(absent()), new_read(absent())],
            Some("ref-1"),
        )
        .await;
        assert!(outcome == failed(CREATE_LATER), "looks only, then says so");
        script.finished();
        assert_eq!(held_posts(&host, "ref-1"), Some(MAX_POSTS));
        // Still found when it shows.
        let copy = google_copy(&dentist_draft(), "\"n1\"");
        let (outcome, _) = create(&host, vec![new_read(Scripted::Json(copy))], Some("ref-1")).await;
        assert!(outcome == saved(), "landed at last");
        assert_eq!(held_creates(&host), 0);
    }

    /// The looks are reads: editing turned off since doesn't stop a retry finding the event, and
    /// only a re-send is refused, the token kept (C10 critique 1 M1).
    #[tokio::test]
    async fn a_retry_looks_whatever_editing_says_and_only_a_resend_needs_it() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let (outcome, _) = create_on(
            &host,
            &new_target(),
            EditingStatus::Paused,
            vec![new_read(absent()), new_read(absent())],
            dentist_draft(),
            Some("ref-1"),
        )
        .await;
        assert!(
            outcome
                == WriteOutcome::ReadOnly {
                    reason: ReadOnlyReason::EditingOff
                },
            "the re-send is refused"
        );
        assert_eq!(held_posts(&host, "ref-1"), Some(2));
        let copy = google_copy(&dentist_draft(), "\"n1\"");
        let (outcome, _) = create_on(
            &host,
            &new_target(),
            EditingStatus::Paused,
            vec![new_read(Scripted::Json(copy))],
            dentist_draft(),
            Some("ref-1"),
        )
        .await;
        assert!(outcome == saved(), "found with editing paused");
    }

    #[tokio::test]
    async fn a_retry_with_a_changed_draft_or_calendar_is_refused_and_kept() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let renamed = NewEventDraft {
            summary: "Dentist (moved)".into(),
            ..dentist_draft()
        };
        let other = CalendarTarget {
            calendar_id: "cal2".into(),
            ..new_target()
        };
        for (target, draft) in [(new_target(), renamed), (other, dentist_draft())] {
            let (outcome, script) = create_on(
                &host,
                &target,
                EditingStatus::On,
                Vec::new(),
                draft,
                Some("ref-1"),
            )
            .await;
            assert!(outcome == failed(CREATE_CHANGED), "refused, nothing sent");
            assert!(script.sent().is_empty());
            assert_eq!(held_posts(&host, "ref-1"), Some(2));
        }
    }

    /// A retry running holds its claim and has taken the create out, as `retry_create` does: a
    /// second one is Busy, never told the token is lost, and sends nothing (claim before take).
    #[tokio::test]
    async fn a_retry_while_one_runs_is_busy() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let claim = InFlight::claim(&host.edit, &create_key("ref-1")).unwrap();
        let (taken, _held) = TakenCreate::take(&host.edit, "ref-1").unwrap();
        assert_eq!(held_creates(&host), 0);
        let (outcome, script) = create(&host, Vec::new(), Some("ref-1")).await;
        assert!(outcome == WriteOutcome::Busy, "busy, not lost");
        assert!(script.sent().is_empty());
        // The running one ends: the create goes back under its token.
        drop(taken);
        drop(claim);
        assert_eq!(held_creates(&host), 1);
        assert_eq!(held_posts(&host, "ref-1"), Some(2));
    }

    /// The eight hours run out with no prune but the command's own: a retry after them is told
    /// the token is lost, and nothing is sent.
    #[tokio::test]
    async fn a_retry_past_the_eight_hours_finds_the_token_lapsed() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        // Only where the clock reaches back that far (every Unix one), as `local_slot`'s TTL test.
        let Some(old) = Instant::now().checked_sub(SESSION_TTL + Duration::from_secs(1)) else {
            return;
        };
        set_held_at(&host, "ref-1", old);
        let (outcome, script) = create(&host, Vec::new(), Some("ref-1")).await;
        assert!(outcome == failed(CREATE_LOST), "lapsed");
        assert!(script.sent().is_empty());
        assert_eq!(held_creates(&host), 0);
    }

    /// A re-send starts the eight hours over: a create sent again just before they ran out is
    /// still held after them.
    #[tokio::test]
    async fn a_resend_starts_the_eight_hours_over() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let Some(old) = Instant::now().checked_sub(SESSION_TTL - Duration::from_secs(60)) else {
            return;
        };
        set_held_at(&host, "ref-1", old);
        let (outcome, _) = create(
            &host,
            vec![
                new_read(absent()),
                new_read(absent()),
                insert_step(lost()),
                new_read(absent()),
            ],
            Some("ref-1"),
        )
        .await;
        assert!(outcome == ambiguous("ref-1"), "held");
        host.edit
            .prune_creates(Instant::now() + Duration::from_secs(120));
        assert_eq!(
            held_posts(&host, "ref-1"),
            Some(3),
            "kept past the old eight hours"
        );
    }

    /// A re-send Google answers with the copy cancelled, or one whose answer is lost and then
    /// found cancelled: made, then deleted in Google. Gone, the token with it.
    #[tokio::test]
    async fn a_resend_that_meets_a_cancelled_copy_says_gone() {
        let cancelled = with(
            google_copy(&dentist_draft(), "\"n2\""),
            "status",
            json!("cancelled"),
        );
        for tail in [
            vec![insert_step(Scripted::Json(cancelled.clone()))],
            vec![
                insert_step(lost()),
                new_read(Scripted::Json(cancelled.clone())),
            ],
        ] {
            let host = TestHost::holding(&[]);
            lost_create(&host).await;
            let mut steps = vec![new_read(absent()), new_read(absent())];
            steps.extend(tail);
            let (outcome, script) = create(&host, steps, Some("ref-1")).await;
            assert!(outcome == WriteOutcome::Gone, "gone");
            assert_eq!(inserts(&script).len(), 1);
            assert_eq!(held_creates(&host), 0);
            assert!(new_row(&host).is_none());
        }
    }

    /// A retry into a calendar PM stopped syncing still looks, but never sends it again there:
    /// refused as a first create is, the token kept.
    #[tokio::test]
    async fn a_retry_never_resends_into_a_calendar_pm_doesnt_sync() {
        let host = TestHost::holding(&[]);
        lost_create(&host).await;
        let unsynced = CalendarTarget {
            selected: false,
            ..new_target()
        };
        let (outcome, script) = create_on(
            &host,
            &unsynced,
            EditingStatus::On,
            vec![new_read(absent()), new_read(absent())],
            dentist_draft(),
            Some("ref-1"),
        )
        .await;
        assert!(
            outcome == failed(CREATE_NOT_SYNCED),
            "refused before a re-send"
        );
        assert!(inserts(&script).is_empty());
        assert_eq!(held_posts(&host, "ref-1"), Some(2));
    }

    #[tokio::test]
    async fn an_unknown_or_lapsed_token_says_so_and_sends_nothing() {
        let host = TestHost::holding(&[]);
        let (outcome, _) = create(&host, Vec::new(), Some("nope")).await;
        assert!(outcome == failed(CREATE_LOST), "unknown");
        lost_create(&host).await;
        host.edit
            .prune_creates(Instant::now() + SESSION_TTL + Duration::from_secs(1));
        let (outcome, _) = create(&host, Vec::new(), Some("ref-1")).await;
        assert!(outcome == failed(CREATE_LOST), "lapsed");
    }

    /// A28: a list that lags behind the new event can't take it off the screen, and a fetch that
    /// began before the landing can't settle its entry.
    #[tokio::test]
    async fn a_lagging_list_keeps_a_new_event_on_screen() {
        let host = TestHost::holding(&[]);
        let before = Instant::now();
        let copy = google_copy(&dentist_draft(), "\"n1\"");
        create(&host, vec![insert_step(Scripted::Json(copy.clone()))], None).await;
        let id = format!("cal:{NEW}");
        let after = Instant::now();
        assert!(host.synced(&[], after).iter().any(|r| r.id == id));
        // An older fetch that happens to show it settles nothing.
        assert!(host
            .synced(std::slice::from_ref(&copy), before)
            .iter()
            .any(|r| r.id == id));
        assert!(!host.nothing_recorded());
        // A fetch that began after the landing and shows it ends the entry.
        host.synced(&[copy], after);
        assert!(host.nothing_recorded());
    }

    // --- creating a repeating event (C11) ---

    /// One occurrence of the new repeating event, at `at` for an hour, as an instances listing
    /// gives it; `stamp` is its start in an instance id's compact form.
    fn new_occurrence(stamp: &str, at: &str, end: &str) -> Value {
        json!({
            "kind": "calendar#event",
            "id": format!("{NEW}_{stamp}"),
            "etag": "\"o1\"",
            "status": "confirmed",
            "summary": "Dentist",
            "recurringEventId": NEW,
            "originalStartTime": { "dateTime": at, "timeZone": "Europe/London" },
            "start": { "dateTime": at, "timeZone": "Europe/London" },
            "end": { "dateTime": end, "timeZone": "Europe/London" },
            "iCalUID": "new@google.com",
            "updated": SAVED_AT,
        })
    }

    /// The weekly dentist from Monday 26-10, three times, as Google lists it.
    fn three_mondays() -> Vec<Value> {
        vec![
            new_occurrence(
                "20261026T100000Z",
                "2026-10-26T10:00:00Z",
                "2026-10-26T11:00:00Z",
            ),
            new_occurrence(
                "20261102T100000Z",
                "2026-11-02T10:00:00Z",
                "2026-11-02T11:00:00Z",
            ),
            new_occurrence(
                "20261109T100000Z",
                "2026-11-09T10:00:00Z",
                "2026-11-09T11:00:00Z",
            ),
        ]
    }

    /// The new repeating event's occurrences over the mirror's window, 50 a page.
    fn new_listing(answer: Scripted) -> Step {
        Step::new(
            Method::Get,
            &format!("/events/{NEW}/instances"),
            "timeMin=2026-09-01T00%3A00%3A00Z timeMax=2027-11-01T00%3A00%3A00Z maxResults=50",
            answer,
        )
    }

    fn listed(items: Vec<Value>) -> Scripted {
        Scripted::Json(page(items, None))
    }

    /// The new repeating event's rows in the mirror, in time order.
    fn new_rows(host: &TestHost) -> Vec<String> {
        ids_of(
            host.with_conn(|conn| calendar::series_rows(conn, "cal", &[NEW]))
                .unwrap(),
        )
    }

    fn three_ids() -> Vec<String> {
        ["20261026T100000Z", "20261102T100000Z", "20261109T100000Z"]
            .iter()
            .map(|stamp| format!("cal:{NEW}_{stamp}"))
            .collect()
    }

    /// C11 §3.4: Google answers a repeating create with its master, which is never a row, so its
    /// occurrences are listed and landed as created rows. Nobody is emailed, and the rule and the
    /// zone go out with it.
    #[tokio::test]
    async fn a_created_series_lands_its_occurrences_from_a_listing() {
        let host = TestHost::holding(&[]);
        let draft = weekly_dentist("2026-10-26", 3);
        let copy = google_copy(&draft, "\"n1\"");
        let (outcome, script) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            vec![
                insert_step(Scripted::Json(copy)),
                new_listing(listed(three_mondays())),
            ],
            draft,
            None,
        )
        .await;
        assert!(outcome == saved(), "saved and landed");
        let sent = script.sent();
        assert!(sent[0]
            .url
            .ends_with("?sendUpdates=none&conferenceDataVersion=1"));
        let body = sent[0].body.as_ref().unwrap();
        assert_eq!(
            body["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=3"])
        );
        assert_eq!(body["start"]["timeZone"], "Europe/London");
        assert_eq!(body["end"]["timeZone"], "Europe/London");
        assert!(script.waits().is_empty(), "the first look found them");
        assert_eq!(new_rows(&host), three_ids());
        assert!(new_row(&host).is_none(), "the master itself is never a row");
        assert_eq!(host.told(), vec!["cal".to_string()]);
        // Recorded as created: a complete list that doesn't show them yet keeps them on screen.
        let shown = ids_of(host.synced(&[], Instant::now()));
        assert!(three_ids().iter().all(|id| shown.contains(id)));
        assert_eq!(held_creates(&host), 0);
    }

    /// A28: a listing that lags shows nothing at first, or only its first page before the second is
    /// refused; the second look, a second later, lands it.
    #[tokio::test]
    async fn a_created_series_whose_listing_lags_lands_on_the_second_look() {
        let empty = vec![new_listing(listed(Vec::new()))];
        let half = vec![
            new_listing(Scripted::Json(page(three_mondays(), Some("p2")))),
            new_listing(Scripted::Status(503)),
        ];
        for first in [empty, half] {
            let host = TestHost::holding(&[]);
            let draft = weekly_dentist("2026-10-26", 3);
            let copy = google_copy(&draft, "\"n1\"");
            let mut steps = vec![insert_step(Scripted::Json(copy))];
            steps.extend(first);
            steps.push(new_listing(listed(three_mondays())));
            let (outcome, script) =
                create_on(&host, &new_target(), EditingStatus::On, steps, draft, None).await;
            assert!(outcome == saved(), "landed on the second look");
            assert_eq!(script.waits(), vec![REFETCH_AGAIN[0]]);
            assert_eq!(new_rows(&host), three_ids());
        }
    }

    /// Three looks that never show it whole: saved, the next sync to show it, and no guess landed.
    /// Half a listing is no listing, and one that shows only deleted occurrences lands nothing.
    #[tokio::test]
    async fn a_created_series_that_never_lists_is_saved_pending() {
        let half = vec![
            // A first page, then its second refused.
            new_listing(Scripted::Json(page(three_mondays(), Some("p2")))),
            new_listing(Scripted::Status(503)),
        ];
        let deleted = vec![new_listing(listed(
            three_mondays()
                .into_iter()
                .map(|o| with(o, "status", json!("cancelled")))
                .collect(),
        ))];
        for last in [half, deleted] {
            let host = TestHost::holding(&[]);
            let draft = weekly_dentist("2026-10-26", 3);
            let copy = google_copy(&draft, "\"n1\"");
            let mut steps = vec![
                insert_step(Scripted::Json(copy)),
                new_listing(listed(Vec::new())),
                new_listing(Scripted::Status(503)),
            ];
            steps.extend(last);
            let (outcome, script) =
                create_on(&host, &new_target(), EditingStatus::On, steps, draft, None).await;
            assert!(outcome == pending(), "saved, the mirror to catch up");
            assert_eq!(script.waits(), REFETCH_AGAIN.to_vec());
            assert!(new_rows(&host).is_empty());
            assert!(host.hash_dropped());
            assert!(host.nothing_recorded());
        }
    }

    /// C10 §6.2, C11 critique 1 L8: a new repeating event with no occurrence in the months PM
    /// mirrors, after them or ended before them, is saved and said to be outside, with no listing
    /// at all. One begun before them that runs into them is listed.
    #[tokio::test]
    async fn a_created_series_after_the_window_is_saved_not_landed() {
        let outside = WriteOutcome::Saved {
            warnings: vec![WriteWarning::OutsideMirror],
            left_behind: Vec::new(),
            milestone_moves: None,
        };
        // Monday 10-01-2028 is past the window; Monday 01-06-2026 three times ends before it.
        for draft in [
            weekly_dentist("2028-01-10", 3),
            weekly_dentist("2026-06-01", 3),
        ] {
            let host = TestHost::holding(&[]);
            let copy = google_copy(&draft, "\"n1\"");
            let (outcome, script) = create_on(
                &host,
                &new_target(),
                EditingStatus::On,
                vec![insert_step(Scripted::Json(copy))],
                draft,
                None,
            )
            .await;
            assert!(
                outcome == outside,
                "saved in Google, and said to be outside"
            );
            assert_eq!(script.sent().len(), 1, "nothing listed");
            assert!(new_rows(&host).is_empty() && host.nothing_recorded());
            assert!(host.told().is_empty());
        }
        // From Monday 03-08-2026, ten times: its last five fall in the window, so it is listed (the
        // listing's own occurrences are what land, three here).
        let host = TestHost::holding(&[]);
        let draft = weekly_dentist("2026-08-03", 10);
        let copy = google_copy(&draft, "\"n1\"");
        let (outcome, _) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            vec![
                insert_step(Scripted::Json(copy)),
                new_listing(listed(three_mondays())),
            ],
            draft,
            None,
        )
        .await;
        assert!(outcome == saved(), "listed and landed");
        assert_eq!(new_rows(&host).len(), 3);
    }

    /// A retry that finds the lost repeating create lands its occurrences, never its master.
    #[tokio::test]
    async fn a_retried_create_of_a_series_lands_its_occurrences() {
        let host = TestHost::holding(&[]);
        let draft = weekly_dentist("2026-10-26", 3);
        let (outcome, _) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            vec![
                insert_step(lost()),
                new_read(absent()),
                new_read(absent()),
                insert_step(lost()),
                new_read(absent()),
            ],
            draft.clone(),
            None,
        )
        .await;
        assert!(outcome == ambiguous("ref-1"), "held as ref-1");
        let copy = google_copy(&draft, "\"n1\"");
        let (outcome, script) = create_on(
            &host,
            &new_target(),
            EditingStatus::On,
            vec![
                new_read(Scripted::Json(copy)),
                new_listing(listed(three_mondays())),
            ],
            draft,
            Some("ref-1"),
        )
        .await;
        assert!(outcome == saved(), "found and landed");
        script.finished();
        assert_eq!(new_rows(&host), three_ids());
        assert_eq!(held_creates(&host), 0);
    }

    /// C11 critique 1 L11: a repeating create reaches a whole series, so a dev build's unset
    /// allow-list refuses it, as it does every write that reaches more than one event.
    #[test]
    fn a_repeating_create_is_series_wide_to_the_dev_allow_list() {
        let source = include_str!("calendar_edit.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        let start = shipped
            .find("pub async fn create_calendar_event(")
            .expect("the create command");
        let shell = &shipped[start..];
        let shell = &shell[..shell.find("\n}\n").unwrap_or(shell.len())];
        assert!(shell.contains("let series_wide = draft.recurrence.is_some();"));
        assert!(shell
            .contains("dev_allow::check(&*state.conn()?, &target.remote_calendar, series_wide)"));
        assert_eq!(
            dev_allow::decide(None, None, "home@group", true),
            Err(dev_allow::UNSET)
        );
    }

    /// Creating events ships in every build (C10b): the command makes the window and owner checks
    /// every write makes before it reads anything or sends, and no build refuses it outright.
    #[test]
    fn the_create_command_checks_the_window_and_owner_before_anything_else() {
        let source = include_str!("calendar_edit.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        let start = shipped
            .find("pub async fn create_calendar_event(")
            .expect("the create command");
        let shell = &shipped[start..];
        let shell = &shell[..shell.find("\n}\n").unwrap_or(shell.len())];
        let at = |needle: &str| shell.find(needle).unwrap_or(usize::MAX);
        assert!(
            at("require_main_window(&window)?;") < at("require_owner(&app)?;")
                && at("require_owner(&app)?;") < at("state.conn()")
                && at("state.conn()") < at("create_event(&ctx"),
            "the window and owner checks come before reading anything or sending"
        );
        assert!(
            !shell.contains("if !cfg!(debug_assertions)"),
            "no build refuses creating events outright"
        );
    }

    /// The webview ends a retry token on two of the create's refusals, which it knows only by their
    /// words: its copies of them must be these very strings (C10 critiques: the token drops only on
    /// saved, gone, these two).
    #[test]
    fn the_webview_knows_the_words_that_end_a_retry_token() {
        let words = include_str!("../../../src/lib/calendarEdit/editReasons.ts");
        for (name, text) in [
            ("CREATE_LOST", CREATE_LOST),
            ("CREATE_CHANGED", CREATE_CHANGED),
        ] {
            let line = format!("export const {name} =");
            let at = words.find(&line).expect("the webview names it");
            let rest = &words[at..];
            let decl = &rest[..rest.find(";\n").unwrap_or(rest.len())];
            assert!(
                decl.contains(&format!("\"{text}\"")),
                "{name}: the webview's copy differs from the backend's"
            );
        }
    }

    /// The retry's table: two empty looks before a re-send, a cancelled copy gone, anything it
    /// can't read keeping the create held.
    #[test]
    fn retry_next_follows_the_table() {
        let copy = json!({ "id": NEW });
        for first in [true, false] {
            assert!(
                matches!(retry_next(RetryLook::Live(copy.clone()), first), RetryStep::Land(c) if c == copy)
            );
            assert!(matches!(
                retry_next(RetryLook::Cancelled, first),
                RetryStep::Gone
            ));
            assert!(matches!(
                retry_next(RetryLook::Reauth, first),
                RetryStep::Keep(Kept::Reauth)
            ));
            assert!(matches!(
                retry_next(RetryLook::Busy, first),
                RetryStep::Keep(Kept::Busy)
            ));
            assert!(matches!(
                retry_next(RetryLook::Unknown, first),
                RetryStep::Keep(Kept::Ambiguous)
            ));
        }
        assert!(matches!(
            retry_next(RetryLook::Absent, true),
            RetryStep::LookAgain
        ));
        assert!(matches!(
            retry_next(RetryLook::Absent, false),
            RetryStep::Send
        ));
        let cancelled = json!({ "id": NEW, "status": "cancelled" });
        assert!(matches!(
            RetryLook::of(Fetched::Event(cancelled)),
            RetryLook::Cancelled
        ));
        assert!(matches!(
            RetryLook::of(Fetched::Event(copy)),
            RetryLook::Live(_)
        ));
        assert!(matches!(RetryLook::of(Fetched::Gone), RetryLook::Absent));
        assert!(matches!(
            RetryLook::of(Fetched::Failed("x".into())),
            RetryLook::Unknown
        ));
    }

    #[test]
    fn in_mirror_reads_overlap_at_both_edges() {
        let window = mirror(&[]).instants();
        let row = |start: &str, end: Option<&str>, all_day: bool| CalendarEvent {
            start: start.into(),
            end: end.map(str::to_string),
            all_day,
            ..Default::default()
        };
        // Ends exactly where the window starts: out.
        let edge = row("2026-08-31T23:00:00Z", Some("2026-09-01T00:00:00Z"), false);
        assert_eq!(in_mirror(&edge, window), Some(false));
        let across = row("2026-08-31T23:00:00Z", Some("2026-09-01T00:30:00Z"), false);
        assert_eq!(in_mirror(&across, window), Some(true));
        // The last day before the window ends: in. The day the window ends on: out.
        let last = row("2027-10-31", Some("2027-11-01"), true);
        assert_eq!(in_mirror(&last, window), Some(true));
        let after = row("2027-11-01", Some("2027-11-02"), true);
        assert_eq!(in_mirror(&after, window), Some(false));
        // No end: its start alone.
        let point = row("2026-10-26T10:00:00Z", None, false);
        assert_eq!(in_mirror(&point, window), Some(true));
        // What can't be read lands.
        assert_eq!(in_mirror(&row("soon", None, false), window), None);
        assert_eq!(in_mirror(&point, None), None);
    }

    #[test]
    fn resolve_calendar_turns_feeds_and_outlook_into_not_google() {
        let (_d, conn) = store();
        let cal = "gcal:me@x.com:primary";
        add_calendar(&conn, "gcal:me@x.com", "google", cal, Some("me@x.com"));
        let google = resolve_calendar(&conn, cal, "gone").unwrap().unwrap();
        assert!(google.remote_calendar == "me@x.com" && google.email == "me@x.com");
        assert_eq!(google.token_key, "google_oauth_token_calendar::me@x.com");
        assert!(google.selected);
        calendar::set_calendar_selected(&conn, cal, false).unwrap();
        assert!(
            !resolve_calendar(&conn, cal, "gone")
                .unwrap()
                .unwrap()
                .selected
        );
        add_calendar(&conn, "ics:feed", "ics", "ics:feed:1", None);
        add_calendar(
            &conn,
            "outlook:me@x.com",
            "outlook",
            "outlook:me@x.com:1",
            None,
        );
        for other in ["ics:feed:1", "outlook:me@x.com:1"] {
            assert!(matches!(
                resolve_calendar(&conn, other, "gone").unwrap(),
                Err(ReadOnlyReason::NotGoogle)
            ));
        }
        // Each caller says it its own way when the calendar has gone.
        let missing = resolve_calendar(&conn, "gcal:me@x.com:nope", "That calendar is gone.");
        assert!(matches!(missing, Err(Error::Other(m)) if m == "That calendar is gone."));
    }

    // --- how a repeating event repeats, for the popover (C9b-2) ---

    /// The popover's ask about a row of series "abc" at `etag`.
    fn summary_ask_at(etag: Option<&str>) -> SummaryAsk {
        SummaryAsk {
            remote_calendar: CALENDAR.into(),
            series: "abc".into(),
            etag: etag.map(str::to_string),
            token_key: "key".into(),
        }
    }

    /// What PM keeps for series "abc": its words, and the row etags they stand for.
    fn kept(edit: &CalendarEditState) -> Option<(String, Vec<String>)> {
        let summaries = edit.summaries.lock().unwrap();
        summaries.known.get(&series_key(CALENDAR, "abc")).map(|k| {
            let mut etags: Vec<String> = k.etags.iter().cloned().collect();
            etags.sort();
            (k.words.clone(), etags)
        })
    }

    /// One GET of the series, in the editor's own words; a second ask for the same row sends
    /// nothing (the script has no second step).
    #[tokio::test]
    async fn a_summary_reads_the_series_once_and_describes_it() {
        let edit = CalendarEditState::default();
        let ask = summary_ask_at(Some("\"e1\""));
        let script = Script::new(vec![event_read("abc", Scripted::Json(weekly()))]);
        let words = series_summary(&script, &edit, &ask).await;
        assert!(
            words.as_deref() == Some("Weekly on Monday"),
            "Google's series, described"
        );
        let again = series_summary(&script, &edit, &ask).await;
        assert!(again == words, "the kept words");
        script.finished();
        assert!(script.sent().len() == 1, "one GET");
    }

    /// Occurrences A, B, then A again: two GETs, and one entry holding both rows' etags.
    #[tokio::test]
    async fn another_occurrence_of_the_same_words_is_kept_beside_it() {
        let edit = CalendarEditState::default();
        let script = Script::new(vec![
            event_read("abc", Scripted::Json(weekly())),
            event_read("abc", Scripted::Json(weekly())),
        ]);
        for etag in ["\"e1\"", "\"e2\"", "\"e1\""] {
            let words = series_summary(&script, &edit, &summary_ask_at(Some(etag))).await;
            assert!(words.as_deref() == Some("Weekly on Monday"), "{etag}");
        }
        script.finished();
        assert!(
            kept(&edit)
                == Some((
                    "Weekly on Monday".into(),
                    vec!["\"e1\"".into(), "\"e2\"".into()]
                )),
            "one entry, both etags"
        );
    }

    /// The row changed (a new etag): the series is read again, and its new words replace the old.
    #[tokio::test]
    async fn a_new_row_etag_reads_the_series_again() {
        let edit = CalendarEditState::default();
        let script = Script::new(vec![
            event_read("abc", Scripted::Json(weekly())),
            event_read("abc", Scripted::Json(master(&["RRULE:FREQ=DAILY"]))),
        ]);
        series_summary(&script, &edit, &summary_ask_at(Some("\"e1\""))).await;
        let words = series_summary(&script, &edit, &summary_ask_at(Some("\"e2\""))).await;
        script.finished();
        assert!(words.as_deref() == Some("Daily"), "read again");
        assert!(
            kept(&edit) == Some(("Daily".into(), vec!["\"e2\"".into()])),
            "the new words, for the new etag only"
        );
    }

    /// Words stand for [`SUMMARY_TTL`] whatever the etags say, then are read again.
    #[test]
    fn words_older_than_their_time_are_read_again() {
        let edit = CalendarEditState::default();
        let key = series_key(CALENDAR, "abc");
        let now = Instant::now();
        let since = summary_writes(&edit);
        keep_summary(&edit, key.clone(), Some("\"e1\""), "Daily", since, now);
        let at = |later: Duration| known_summary(&edit, &key, Some("\"e1\""), now + later);
        assert!(at(SUMMARY_TTL - Duration::from_secs(1)).as_deref() == Some("Daily"));
        assert!(at(SUMMARY_TTL + Duration::from_secs(1)).is_none());
    }

    /// Past [`SUMMARY_KEEP`] series, every entry older than the TTL goes; with none that old, the
    /// one read longest ago goes, and only that one.
    #[test]
    fn past_the_cap_the_stale_go_then_the_oldest() {
        let edit = CalendarEditState::default();
        let since = summary_writes(&edit);
        let base = Instant::now();
        let fresh = base + SUMMARY_TTL;
        let secs = |n: usize| Duration::from_secs(n as u64);
        let key = |name: &str| series_key(CALENDAR, name);
        let keep = |name: &str, now: Instant| {
            keep_summary(&edit, key(name), Some("\"e1\""), "Daily", since, now);
        };
        keep("stale-0", base);
        keep("stale-1", base);
        for n in 2..SUMMARY_KEEP {
            keep(&format!("s{n}"), fresh + secs(n));
        }
        let held = |name: &str| {
            edit.summaries
                .lock()
                .unwrap()
                .known
                .contains_key(&key(name))
        };
        let count = || edit.summaries.lock().unwrap().known.len();
        assert!(count() == SUMMARY_KEEP, "full");
        keep("new-a", fresh + secs(SUMMARY_KEEP));
        assert!(
            !held("stale-0") && !held("stale-1") && held("new-a"),
            "both stale entries go"
        );
        assert!(count() == SUMMARY_KEEP - 1, "and only they");
        keep("new-b", fresh + secs(SUMMARY_KEEP + 1));
        assert!(count() == SUMMARY_KEEP, "full again, all of it fresh");
        keep("new-c", fresh + secs(SUMMARY_KEEP + 2));
        assert!(
            !held("s2") && held("s3") && held("new-b") && held("new-c"),
            "the one read longest ago goes"
        );
        assert!(count() == SUMMARY_KEEP, "and the cap holds");
    }

    /// A row mirrored before etags were kept is asked about every time, and nothing is kept.
    #[tokio::test]
    async fn a_row_without_an_etag_is_never_kept() {
        let edit = CalendarEditState::default();
        let script = Script::new(vec![
            event_read("abc", Scripted::Json(weekly())),
            event_read("abc", Scripted::Json(weekly())),
        ]);
        for _ in 0..2 {
            let words = series_summary(&script, &edit, &summary_ask_at(None)).await;
            assert!(words.as_deref() == Some("Weekly on Monday"));
        }
        script.finished();
        assert!(kept(&edit).is_none());
    }

    /// Whatever stops the read is `None` (the popover says "Repeats"), and nothing is kept.
    #[tokio::test]
    async fn a_summary_that_fails_is_none_and_never_kept() {
        let mut bare = weekly();
        bare.as_object_mut().unwrap().remove("recurrence");
        let mut zoneless = weekly();
        zoneless["start"] = json!({ "dateTime": "2026-10-05T09:00:00+01:00" });
        let answers = vec![
            ("404", Scripted::Status(404)),
            ("410", Scripted::Status(410)),
            ("signed out", Scripted::Status(401)),
            ("rate-limited", Scripted::Status(429)),
            ("server error", Scripted::Status(500)),
            (
                "unreadable",
                Scripted::Reply {
                    status: 200,
                    body: None,
                    date: None,
                    maybe_applied: false,
                },
            ),
            (
                "cancelled",
                Scripted::Json(with(weekly(), "status", json!("cancelled"))),
            ),
            (
                "another series",
                Scripted::Json(with(weekly(), "id", json!("xyz"))),
            ),
            ("no rule", Scripted::Json(bare)),
            ("no zone", Scripted::Json(zoneless)),
        ];
        for (what, answer) in answers {
            let edit = CalendarEditState::default();
            let script = Script::new(vec![event_read("abc", answer)]);
            let words = series_summary(&script, &edit, &summary_ask_at(Some("\"e1\""))).await;
            script.finished();
            assert!(words.is_none(), "{what}: no words");
            assert!(kept(&edit).is_none(), "{what}: nothing kept");
        }
    }

    /// A rule the editor can't make is said plainly, never guessed at.
    #[tokio::test]
    async fn a_rule_outside_the_editors_set_says_so() {
        let edit = CalendarEditState::default();
        let two = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "RRULE:FREQ=MONTHLY;BYMONTHDAY=1",
        ]);
        let script = Script::new(vec![event_read("abc", Scripted::Json(two))]);
        let words = series_summary(&script, &edit, &summary_ask_at(Some("\"e1\""))).await;
        script.finished();
        assert!(words.as_deref() == Some(crate::calendar_recur::spec::CUSTOM_RULE));
    }

    /// Only a Google occurrence is asked about: a one-off, a feed's row, a series id that isn't one
    /// path segment, and a row that's gone ask nothing, and none of them is an error.
    #[test]
    fn a_one_off_an_ical_row_and_a_missing_row_ask_nothing() {
        let (_d, conn) = store();
        let cal = "gcal:me@x.com:primary";
        add_calendar(&conn, "gcal:me@x.com", "google", cal, Some("me@x.com"));
        let one_off = CalendarEvent {
            id: format!("{cal}:solo"),
            calendar_id: cal.into(),
            summary: "Dentist".into(),
            start: "2026-10-12T09:00:00Z".into(),
            organizer_self: true,
            ..Default::default()
        };
        let occurrence = CalendarEvent {
            id: format!("{cal}:abc_20261012T090000Z"),
            series_id: Some("abc".into()),
            recurring: true,
            etag: Some("\"e1\"".into()),
            ..one_off.clone()
        };
        let odd = CalendarEvent {
            id: format!("{cal}:odd_20261012T090000Z"),
            series_id: Some("..".into()),
            ..occurrence.clone()
        };
        calendar::replace_events(
            &conn,
            cal,
            &[one_off.clone(), occurrence.clone(), odd.clone()],
            true,
        )
        .unwrap();
        add_calendar(&conn, "ics:feed", "other", "ics:feed", None);
        let feed = CalendarEvent {
            id: "ics:feed:uid1:2026-10-12".into(),
            calendar_id: "ics:feed".into(),
            series_id: Some("uid1".into()),
            recurrence_summary: Some("Weekly".into()),
            ..occurrence.clone()
        };
        calendar::replace_events(&conn, "ics:feed", std::slice::from_ref(&feed), true).unwrap();
        for nothing in [&one_off.id, &odd.id, &feed.id, &format!("{cal}:gone")] {
            assert!(summary_ask(&conn, nothing).is_none(), "{nothing}");
        }
        let ask = summary_ask(&conn, &occurrence.id).expect("a Google occurrence is asked about");
        assert!(
            ask.remote_calendar == "me@x.com"
                && ask.series == "abc"
                && ask.etag.as_deref() == Some("\"e1\"")
                && ask.token_key == "google_oauth_token_calendar::me@x.com",
            "its calendar, series, etag and sign-in"
        );
    }

    /// A write to a series forgets its words once it lets go of it; another series' stay.
    #[test]
    fn a_series_write_forgets_its_words() {
        let edit = CalendarEditState::default();
        let now = Instant::now();
        for series in ["abc", "other"] {
            let since = summary_writes(&edit);
            keep_summary(
                &edit,
                series_key(CALENDAR, series),
                Some("\"e1\""),
                "Daily",
                since,
                now,
            );
        }
        let claim = InFlight::claim_all(
            &edit,
            &[
                "cal:abc_20261019T080000Z".to_string(),
                series_key(CALENDAR, "abc"),
            ],
        );
        assert!(claim.is_some());
        drop(claim);
        let known =
            |series: &str| known_summary(&edit, &series_key(CALENDAR, series), Some("\"e1\""), now);
        assert!(known("abc").is_none(), "the written series' words go");
        assert!(known("other").is_some(), "another series' stay");
    }

    /// A held delete's claim ends in [`HeldClaim`], which no test can build (it holds the app), so
    /// its release is read from this file's own text (the C9b-1 C1-M8 precedent): it lets go the
    /// way [`InFlight`] does, which the test above runs, the words going after the claims' lock
    /// does, never while it's held, as [`CalendarEditState`] says of every lock but the DB's
    /// (C9b-2 review round 2).
    #[test]
    fn a_held_claim_lets_go_as_a_write_does() {
        let source = include_str!("calendar_edit.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        let body = |of: &str| {
            let at = shipped.find(of).expect("the drop is defined");
            let body = &shipped[at..];
            body[..body.find("\n}\n").unwrap_or(body.len())].to_string()
        };
        for of in ["impl Drop for HeldClaim {", "impl Drop for InFlight<'_> {"] {
            assert!(
                body(of).contains("release_claims("),
                "every claim ends in the one release"
            );
        }
        // Outside the block that holds the claims' lock: the release's last line, once it closes.
        assert!(
            body("fn release_claims(")
                .trim_end()
                .ends_with("        }\n    }\n    series_written(&edit.summaries, keys);"),
            "and the release forgets the series' words once the claims' lock is gone"
        );
    }

    /// C9b-2 critiques C1-M2, C2-L3: an ask a write overlaps keeps nothing, whether the write let go
    /// while the GET was out or still holds the series when it answers.
    #[tokio::test]
    async fn words_read_across_a_write_are_never_kept() {
        let edit: &'static CalendarEditState = Box::leak(Box::default());
        let key = series_key(CALENDAR, "abc");
        let ask = summary_ask_at(Some("\"e1\""));
        let writing = Mutex::new(InFlight::claim_all(edit, std::slice::from_ref(&key)));
        let script = Script::new(vec![event_read("abc", Scripted::Json(weekly())).then(
            move || {
                writing.lock().unwrap().take();
            },
        )]);
        let words = series_summary(&script, edit, &ask).await;
        script.finished();
        assert!(words.is_some(), "the answer still shows");
        assert!(kept(edit).is_none(), "but it isn't kept");
        assert!(
            edit.in_flight.lock().unwrap().is_empty(),
            "the write let go"
        );
        let _writing = InFlight::claim_all(edit, &[key]);
        let script = Script::new(vec![event_read("abc", Scripted::Json(weekly()))]);
        series_summary(&script, edit, &ask).await;
        script.finished();
        assert!(kept(edit).is_none(), "nor while a write holds the series");
    }

    /// C9 design scope-L3: the summary is a read, so the main window only, with no owner check and
    /// none of editing's gates. Read from this file's own text (the C9b-1 C1-M8 precedent).
    #[test]
    fn the_summary_checks_the_window_and_not_the_owner() {
        let source = include_str!("calendar_edit.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        let at = shipped
            .find("pub async fn get_series_summary(")
            .expect("the command is defined");
        let body = &shipped[at..];
        let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
        assert!(
            body.contains("require_main_window(&window)?;"),
            "the main window only"
        );
        assert!(
            !body.contains("require_owner") && !body.contains("require_vault_owner"),
            "a read checks no owner"
        );
        assert!(
            !body.contains("editing") && !body.contains("dev_allow"),
            "nor editing's gates"
        );
    }

    // --- the milestones a "this and following" change offers to move (C9b-2) ---

    /// The series' occurrences `events` as Google lists them, each carrying the series' iCal UID, so
    /// a milestone linked to it resolves through the mirror.
    fn linked(events: Vec<Value>) -> Vec<Value> {
        events
            .into_iter()
            .map(|e| with(e, "iCalUID", json!("abc@google.com")))
            .collect()
    }

    /// The October series, [`linked`]: 12-10, 19-10 and 26-10.
    fn linked_october() -> Vec<Value> {
        linked(october())
    }

    /// A host whose mirror holds [`linked_october`] and resolves milestones on `today`, with
    /// "Launch" in "Atlas" linked to the series: its id.
    fn linked_host(today: &str) -> (Arc<TestHost>, i64) {
        linked_host_holding(october(), today)
    }

    /// [`linked_host`], its mirror holding `events`, [`linked`].
    fn linked_host_holding(events: Vec<Value>, today: &str) -> (Arc<TestHost>, i64) {
        let host = TestHost::holding(&linked(events));
        host.today_is(today);
        let launch = host
            .with_conn(|conn| {
                milestones::add(conn, "Atlas", "Launch", None, Some("abc@google.com".into()))
            })
            .unwrap();
        (host, launch)
    }

    /// A split from Monday 19-10 that runs and lands, the original re-fetched as its 12-10
    /// occurrence alone.
    fn landed_split(plan: &SeriesPlan, insert: Value) -> Vec<Step> {
        vec![
            insert_step(Scripted::Json(insert)),
            series_patch(Scripted::Json(cut_copy(plan))),
            refetch_read(Scripted::Json(page(
                vec![linked_october()[0].clone()],
                None,
            ))),
            tail_refetch(Scripted::Json(page(tail_october(), None))),
        ]
    }

    fn launch_offer(id: i64, date: &str) -> MilestoneOffer {
        MilestoneOffer {
            id,
            label: "Launch".into(),
            project: "Atlas".into(),
            date: date.into(),
        }
    }

    /// The moves a `Saved` offers, if any.
    fn moves_of(outcome: &WriteOutcome) -> Option<&MilestoneMoves> {
        match outcome {
            WriteOutcome::Saved {
                milestone_moves, ..
            } => milestone_moves.as_deref(),
            _ => None,
        }
    }

    /// DECISIONS 17-10 #2: today 13-10, "Launch" takes the series' next occurrence, 19-10, the day
    /// the change starts, so it's offered, with both parts named by their titles, the new part's
    /// first day and the earlier part's last. A ticked-off one, and one on another event, aren't.
    #[tokio::test]
    async fn a_split_offers_the_milestones_dated_from_its_first_day() {
        let (host, launch) = linked_host("2026-10-13");
        host.with_conn(|conn| {
            milestones::add(conn, "Atlas", "Elsewhere", None, Some("else@x".into()))?;
            let done = milestones::add(conn, "Atlas", "Done", None, Some("abc@google.com".into()))?;
            milestones::set_state(conn, done, true)
        })
        .unwrap();
        let plan = split_plan(Vec::new());
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        let want = MilestoneMoves {
            new_series_ical: Some(TAIL_ICAL.into()),
            earlier_series_ical: "abc@google.com".into(),
            new_title: "Retro".into(),
            earlier_title: "Standup".into(),
            first_day: Some("2026-10-19".into()),
            earlier_last_day: Some("2026-10-12".into()),
            today: "2026-10-13".into(),
            milestones: vec![launch_offer(launch, "2026-10-19")],
        };
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: Some(moves) }
                if warnings.is_empty() && left_behind.is_empty() && **moves == want),
            "Launch is offered, the new part named from the insert's answer"
        );
    }

    /// §0.4.2: after the landing the series' iCal UID resolves only to the earlier part, so a read
    /// then would offer nothing; the one made before the first send still does.
    #[tokio::test]
    async fn the_milestones_are_read_before_the_first_send() {
        let (host, launch) = linked_host("2026-10-13");
        let plan = split_plan(Vec::new());
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        let after = host
            .with_conn(|conn| {
                milestones::linked_on_or_after(conn, "abc@google.com", "2026-10-19", "2026-10-13")
            })
            .unwrap();
        assert!(after.is_empty(), "a read after the landing finds nothing");
        let offered: Vec<i64> = moves_of(&outcome)
            .map(|m| m.milestones.iter().map(|o| o.id).collect())
            .unwrap_or_default();
        assert!(
            offered == [launch],
            "the read before the send still offers it"
        );
    }

    /// No linked milestone to offer: exactly the split's own requests, and no moves.
    #[tokio::test]
    async fn a_split_with_nothing_to_offer_reads_nothing_more() {
        let host = TestHost::holding(&linked_october());
        host.today_is("2026-10-13");
        host.with_conn(|conn| {
            milestones::add(conn, "Atlas", "Elsewhere", None, Some("else@x".into()))?;
            let done = milestones::add(conn, "Atlas", "Done", None, Some("abc@google.com".into()))?;
            milestones::set_state(conn, done, true)
        })
        .unwrap();
        let plan = split_plan(Vec::new());
        let insert = with(tail_copy(&plan, "\"t1\"", SAVED_AT), "iCalUID", Value::Null);
        // The script holds no GET of the new part: one would panic as unscripted.
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        assert!(outcome == saved(), "saved, nothing offered");
    }

    /// DECISIONS 17-10 #2's other half: today 12-10, "Launch" takes the 12-10 occurrence, a week
    /// before the change starts, so it stays put and nothing is asked.
    #[tokio::test]
    async fn a_split_from_a_later_occurrence_offers_nothing() {
        let (host, _) = linked_host("2026-10-12");
        let resolved = host
            .with_conn(|conn| milestones::list_all(conn, "2026-10-12"))
            .unwrap();
        assert!(
            resolved.len() == 1 && resolved[0].due_date.as_deref() == Some("2026-10-12"),
            "the fixture's rows put Launch on 12-10"
        );
        let plan = split_plan(Vec::new());
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        assert!(outcome == saved(), "saved, nothing offered");
    }

    /// C9b-2 critique C2-M1: an insert answer without the new part's iCal UID names it from its
    /// landed rows; with neither, one GET of the new part does.
    #[tokio::test]
    async fn the_new_part_is_named_by_one_get_when_the_insert_said_nothing() {
        let plan = split_plan(Vec::new());
        let quiet = with(tail_copy(&plan, "\"t1\"", SAVED_AT), "iCalUID", Value::Null);
        // The new part's rows carry it: no GET.
        let (host, launch) = linked_host("2026-10-13");
        let mut steps = landed_split(&plan, quiet.clone());
        let listed: Vec<Value> = tail_october()
            .into_iter()
            .map(|e| with(e, "iCalUID", json!(TAIL_ICAL)))
            .collect();
        steps[3] = tail_refetch(Scripted::Json(page(listed, None)));
        let (outcome, _, _) = split_save(&host, steps).await;
        assert!(
            moves_of(&outcome).and_then(|m| m.new_series_ical.as_deref()) == Some(TAIL_ICAL),
            "named from its rows"
        );
        // Neither: one GET of it.
        let (host, _) = linked_host("2026-10-13");
        let mut steps = landed_split(&plan, quiet);
        steps.push(event_read(
            TAIL,
            Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT)),
        ));
        let (outcome, script, _) = split_save(&host, steps).await;
        assert!(
            script
                .sent()
                .last()
                .is_some_and(|r| r.method == Method::Get),
            "the last request reads the new part"
        );
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, milestone_moves: Some(m), .. }
                if warnings.is_empty() && m.new_series_ical.as_deref() == Some(TAIL_ICAL)
                    && m.milestones == [launch_offer(launch, "2026-10-19")]),
            "named by the GET"
        );
    }

    /// C9b-2 critique C2-M1: a new part PM can't name still lists the milestones, so the notice
    /// can say they stayed on the earlier part, beside the warning; nothing can be moved.
    #[tokio::test]
    async fn an_unnamed_new_part_says_which_milestones_stayed() {
        let (host, launch) = linked_host("2026-10-13");
        let plan = split_plan(Vec::new());
        let quiet = with(tail_copy(&plan, "\"t1\"", SAVED_AT), "iCalUID", Value::Null);
        let mut steps = landed_split(&plan, quiet);
        steps.push(event_read(TAIL, Scripted::Status(500)));
        let (outcome, _, _) = split_save(&host, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, milestone_moves: Some(m), .. }
                if *warnings == [WriteWarning::MilestoneNotOffered]
                    && m.new_series_ical.is_none()
                    && m.milestones == [launch_offer(launch, "2026-10-19")]),
            "saved, the milestone said to stay"
        );
    }

    /// C9b-2 critique C1-M4: a split held after its cut was lost keeps what it read at the send.
    #[tokio::test]
    async fn a_held_split_keeps_movable() {
        let (host, launch) = linked_host("2026-10-13");
        let (held, _) = lost_cut(&host, lost()).await;
        assert!(matches!(held, WriteOutcome::PartialSplit { .. }));
        let movable: Vec<i64> = host
            .edit
            .partials
            .lock()
            .unwrap()
            .values()
            .filter_map(|h| h.facts.movable.as_ref())
            .flat_map(|m| m.milestones.iter().map(|o| o.offer.id))
            .collect();
        assert!(movable == [launch], "held with Launch");
    }

    /// C9b-2 critique C1-M4, end to end: the cut's answer lost, the split held, then Finish lands it
    /// and offers what the send read, though a read now (today 12-10) would offer nothing.
    #[tokio::test]
    async fn a_finish_offers_the_milestones_read_at_the_send() {
        let (host, launch) = linked_host("2026-10-13");
        let plan = split_plan(Vec::new());
        let (held, _) = lost_cut(&host, lost()).await;
        host.today_is("2026-10-12");
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let mut steps = vec![event_read("abc", Scripted::Json(cut_copy(&plan)))];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![cut_copy(&plan)], None)),
        ));
        steps.push(refetch_read(Scripted::Json(page(
            vec![linked_october()[0].clone()],
            None,
        ))));
        steps.push(tail_refetch(Scripted::Json(page(tail_october(), None))));
        let (outcome, script) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        assert!(script.sent().iter().all(|r| r.method == Method::Get));
        assert!(
            matches!(&outcome, WriteOutcome::Saved { milestone_moves: Some(m), .. }
                if m.new_series_ical.as_deref() == Some(TAIL_ICAL)
                    && m.milestones == [launch_offer(launch, "2026-10-19")]),
            "Finish offers Launch"
        );
        assert!(held_count(&host) == 0);
    }

    /// C9b-2 critique C1-L9: a milestone dated on an occurrence left on the earlier part stays
    /// there unasked; one dated on a moved occurrence is still offered. (The mirror's rows carry no
    /// iCal UID here, so each takes its own last known date.) Whether the landing reads Google or
    /// not: Finish sent nothing after the cut, so PM knows the occurrence stayed (review round 2).
    #[tokio::test]
    async fn a_milestone_on_an_occurrence_left_behind_isnt_offered() {
        for looked in [true, false] {
            let (outcome, launch) = finish_left_behind(looked).await;
            assert!(
                matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: Some(m) }
                    if *left_behind == ["2026-11-02"]
                        && warnings.contains(&WriteWarning::MirrorRefreshPending) != looked
                        && m.milestones == [launch_offer(launch, "2026-10-26")]),
                "Launch offered; Prep, on the occurrence left behind, not"
            );
            // The occurrence left behind may still show, but it isn't where the earlier part ends.
            assert!(
                moves_of(&outcome).and_then(|m| m.earlier_last_day.as_deref())
                    == Some("2026-10-12"),
                "the earlier part ended on 12-10"
            );
        }
    }

    /// A split from Monday 19-10 with [`off_site`] (02-11) changed in Google while its cut was
    /// held, then finished: what Google holds stands, the Off-site occurrence left behind, and the
    /// landing reads Google (`looked`) or only ever finds the new part empty. "Launch" is linked on
    /// 26-10 (its id is returned) and "Prep" on 02-11.
    async fn finish_left_behind(looked: bool) -> (WriteOutcome, i64) {
        let (events, mut steps) = with_off_site();
        let host = TestHost::holding(&events);
        let (launch, _) = host
            .with_conn(|conn| {
                let link = Some("abc@google.com".to_string());
                let launch = milestones::add(
                    conn,
                    "Atlas",
                    "Launch",
                    Some("2026-10-26".into()),
                    link.clone(),
                )?;
                let prep = milestones::add(conn, "Atlas", "Prep", Some("2026-11-02".into()), link)?;
                Ok((launch, prep))
            })
            .unwrap();
        let plan = split_plan(vec![off_site()]);
        let tail = tail_copy(&plan, "\"t1\"", SAVED_AT);
        steps.extend([
            insert_step(Scripted::Json(tail.clone())),
            series_patch(Scripted::Lost { retried: true }),
            event_read("abc", lost()),
            event_read("abc", lost()),
        ]);
        let script = Script::new(steps);
        let (held, _) = save(
            &host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.asked_all();
        let changed_since = with(off_site(), "summary", json!("Changed since"));
        let mut steps = vec![event_read("abc", Scripted::Json(cut_copy(&plan)))];
        steps.extend(tail_reads(tail, Vec::new()));
        steps.push(ical_listing(
            "iCalUID=abc%40google.com",
            Scripted::Json(page(vec![cut_copy(&plan), changed_since.clone()], None)),
        ));
        let original = page(vec![october()[0].clone(), changed_since], None);
        if looked {
            steps.push(refetch_read(Scripted::Json(original)));
            steps.push(tail_refetch(Scripted::Json(page(tail_october(), None))));
        } else {
            for _ in 0..3 {
                steps.push(refetch_read(Scripted::Json(original.clone())));
                steps.push(tail_refetch(Scripted::Json(page(Vec::new(), None))));
            }
        }
        let (outcome, _) = settle(&host, &held_token(&held), SplitAction::Finish, steps).await;
        (outcome, launch)
    }

    /// "Launch"'s date as the milestone list resolves it on `today`.
    fn launch_date(host: &TestHost, today: &str) -> Option<String> {
        host.with_conn(|conn| milestones::list_all(conn, today))
            .unwrap()
            .into_iter()
            .find(|m| m.label == "Launch")
            .and_then(|m| m.due_date)
    }

    /// Save "this and following" retitled from Monday 19-10 against `host`, the series holding
    /// `exception` and the mirror `events`, Google answering as `steps` say after the reads.
    async fn split_with(
        host: &TestHost,
        exception: &Value,
        events: Vec<Value>,
        steps: Vec<Step>,
    ) -> WriteOutcome {
        let mut all = reads(
            &monday_19(),
            &weekly(),
            vec![weekly(), exception.clone()],
            events,
        );
        all.extend(steps);
        let script = Script::new(all);
        let (outcome, _) = save(
            host,
            &script,
            &monday_19(),
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.asked_all();
        outcome
    }

    /// DECISIONS 17-10 #2 as decided (C9b-2 review round 2, R2-F1): the 12-10 occurrence moved to
    /// Wednesday 21-10, past the split from 19-10. "Launch" takes it (today 20-10), and its date is
    /// after the day the change starts, so it's asked about, though the split leaves that occurrence
    /// on the earlier part, whose last day it then is.
    #[tokio::test]
    async fn a_milestone_on_an_earlier_occurrence_moved_past_the_split_is_offered() {
        let moved_12 = moved(
            "2026-10-12T09:00:00+01:00",
            "2026-10-21T09:00:00+01:00",
            "2026-10-21T10:00:00+01:00",
        );
        let events = vec![moved_12.clone(), monday_19(), plain("2026-10-26T09:00:00Z")];
        let (host, launch) = linked_host_holding(events.clone(), "2026-10-20");
        assert!(
            launch_date(&host, "2026-10-20").as_deref() == Some("2026-10-21"),
            "the fixture's rows put Launch on 21-10"
        );
        let plan = split_plan(vec![moved_12.clone()]);
        let steps = vec![
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
            refetch_read(Scripted::Json(page(linked(vec![moved_12.clone()]), None))),
            tail_refetch(Scripted::Json(page(tail_october(), None))),
        ];
        let outcome = split_with(&host, &moved_12, events, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, milestone_moves: Some(m), .. }
                if warnings.is_empty()
                    && m.milestones == [launch_offer(launch, "2026-10-21")]
                    && m.earlier_last_day.as_deref() == Some("2026-10-21")),
            "Launch offered by its date"
        );
    }

    /// The new part's 26-10 occurrence, as the carry looks it up.
    fn tail_26_lookup(found: Vec<Value>) -> Step {
        Step::get(
            &format!("/events/{TAIL}/instances"),
            "timeMin=2026-10-25T09%3A00%3A00Z timeMax=2026-10-27T09%3A00%3A00Z maxResults=10",
            page(found, None),
        )
    }

    /// The 26-10 occurrence, moved to `start`–`end` (RFC 3339).
    fn moved_26(start: &str, end: &str) -> Value {
        moved("2026-10-26T09:00:00Z", start, end)
    }

    /// The split carrying `exception` (the 26-10 occurrence, moved) onto the new part, then
    /// cancelling it on the original as `cancel` answers, `then` following: the carry's lookup and
    /// PATCH, and the cancel.
    fn carry_26(plan: &SeriesPlan, exception: &Value, cancel: Scripted) -> Vec<Step> {
        let found = tail_occurrence("2026-10-26T09:00:00Z", "\"q1\"");
        let carried = written(
            &found,
            &[
                ("start", exception["start"].clone()),
                ("end", exception["end"].clone()),
            ],
            "\"q2\"",
        );
        vec![
            insert_step(Scripted::Json(tail_copy(plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(plan))),
            tail_26_lookup(vec![found]),
            Step::write(
                Method::Patch,
                &format!("/events/{TAIL}_20261026T090000Z"),
                "\"q1\"",
                Scripted::Json(carried),
            ),
            Step::write(
                Method::Patch,
                "/events/abc_20261026T090000Z",
                exception["etag"].as_str().unwrap(),
                cancel,
            ),
        ]
    }

    /// The original and the new part re-fetched after a split carried the 26-10 occurrence, the
    /// original listing `original` with its 12-10 occurrence.
    fn refetch_after_carry(exception: &Value, original: Option<Value>) -> Vec<Step> {
        let mut listed = linked(vec![plain("2026-10-12T09:00:00+01:00")]);
        listed.extend(original);
        let carried = written(
            &tail_occurrence("2026-10-26T09:00:00Z", "\"q1\""),
            &[
                ("start", exception["start"].clone()),
                ("end", exception["end"].clone()),
            ],
            "\"q2\"",
        );
        vec![
            refetch_read(Scripted::Json(page(listed, None))),
            tail_refetch(Scripted::Json(page(
                vec![
                    tail_occurrence("2026-10-19T09:00:00+01:00", "\"u0\""),
                    carried,
                ],
                None,
            ))),
        ]
    }

    /// DECISIONS 17-10 #2: "Earlier dates stay put with no prompt", even one on an occurrence the
    /// split takes to the new part (C9b-2 review round 2, R2-F1): the 26-10 occurrence moved to
    /// Friday 16-10, before the split from 19-10. "Launch" takes it (today 14-10), and the split
    /// carries it to the new part, but its date is before the day the change starts: nothing is
    /// asked.
    #[tokio::test]
    async fn a_milestone_dated_before_the_split_day_is_never_asked() {
        let exception = moved_26("2026-10-16T09:00:00+01:00", "2026-10-16T10:00:00+01:00");
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            monday_19(),
            exception.clone(),
        ];
        let (host, _) = linked_host_holding(events.clone(), "2026-10-14");
        assert!(
            launch_date(&host, "2026-10-14").as_deref() == Some("2026-10-16"),
            "the fixture's rows put Launch on 16-10"
        );
        let plan = split_plan(vec![exception.clone()]);
        let cancelled = written(
            &with(exception.clone(), "status", json!("cancelled")),
            &[],
            "\"c2\"",
        );
        let mut steps = carry_26(&plan, &exception, Scripted::Json(cancelled));
        steps.extend(refetch_after_carry(&exception, None));
        let outcome = split_with(&host, &exception, events, steps).await;
        assert!(outcome == saved(), "saved, nothing asked");
    }

    /// C9b-2 review F5: the 26-10 occurrence moved to Tuesday 27-10, 09:00 UTC, and its carry never
    /// found, so its old copy is left on the earlier part (left behind on 26-10, its place in the
    /// series). "Launch" takes it (today 20-10) and follows it there: nothing is asked. The user's
    /// zone finds it on Launch's date (review round 2, R2-F3): in Honolulu, 10 hours behind, both are
    /// 26-10, while in the series' London the occurrence is on 27-10.
    #[tokio::test]
    async fn a_milestone_on_a_moved_occurrence_left_behind_isnt_offered() {
        for (zone, date) in [
            (None, "2026-10-27"),
            (Some("Pacific/Honolulu"), "2026-10-26"),
        ] {
            let exception = moved_26("2026-10-27T09:00:00Z", "2026-10-27T10:00:00Z");
            let events = vec![
                plain("2026-10-12T09:00:00+01:00"),
                monday_19(),
                exception.clone(),
            ];
            let (host, _) = linked_host_holding(events.clone(), "2026-10-20");
            if let Some(zone) = zone {
                host.with_conn(|conn| {
                    crate::db::set_setting(conn, crate::settings::TIME_ZONE_KEY, zone)
                })
                .unwrap();
            }
            assert!(
                launch_date(&host, "2026-10-20").as_deref() == Some(date),
                "the fixture's rows put Launch on the occurrence"
            );
            let plan = split_plan(vec![exception.clone()]);
            let mut steps = vec![
                insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
                series_patch(Scripted::Json(cut_copy(&plan))),
            ];
            steps.extend((0..3).map(|_| tail_26_lookup(Vec::new())));
            let left = linked(vec![exception.clone()]).pop();
            steps.extend(refetch_after_carry(&exception, left));
            let outcome = split_with(&host, &exception, events, steps).await;
            assert!(
                matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: None }
                    if *warnings == [WriteWarning::LeftOnOldSeries] && *left_behind == ["2026-10-26"]),
                "saved with its occurrence left behind, nothing offered"
            );
        }
    }

    /// C9b-2 review round 2, R2-F2: the 26-10 occurrence retitled "Off-site" in Google, and the new
    /// part's listing lagging, so the carry never finds its copy there, the cancel that needs it is
    /// never sent, and the landing never reads the new part. The landing can't say where "Launch"'s
    /// occurrence is, but PM knows it never left the earlier part: nothing is asked.
    #[tokio::test]
    async fn a_milestone_on_an_occurrence_never_cancelled_isnt_offered_when_the_landing_couldnt_look(
    ) {
        let exception = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Off-site"));
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            monday_19(),
            exception.clone(),
        ];
        let (host, _) = linked_host_holding(events.clone(), "2026-10-20");
        assert!(
            launch_date(&host, "2026-10-20").as_deref() == Some("2026-10-26"),
            "the fixture's rows put Launch on 26-10"
        );
        let plan = split_plan(vec![exception.clone()]);
        let mut steps = vec![
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
        ];
        steps.extend((0..3).map(|_| tail_26_lookup(Vec::new())));
        for _ in 0..3 {
            let earlier = linked(vec![plain("2026-10-12T09:00:00+01:00"), exception.clone()]);
            steps.push(refetch_read(Scripted::Json(page(earlier, None))));
            steps.push(tail_refetch(Scripted::Json(page(Vec::new(), None))));
        }
        let outcome = split_with(&host, &exception, events, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: None }
                if *warnings == [WriteWarning::MirrorRefreshPending, WriteWarning::LeftOnOldSeries]
                    && *left_behind == ["2026-10-26"]),
            "pending, the occurrence left behind, and nothing offered"
        );
    }

    /// C9b-2 review F5: the same occurrence's cancel answered nothing PM could read, but the landing
    /// sees Google cancelled it, so "Launch" would go overdue on the earlier part: it's asked about.
    #[tokio::test]
    async fn a_milestone_whose_cleanup_landed_unanswered_is_offered() {
        let exception = moved_26("2026-10-27T09:00:00Z", "2026-10-27T10:00:00Z");
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            monday_19(),
            exception.clone(),
        ];
        let (host, launch) = linked_host_holding(events.clone(), "2026-10-20");
        let plan = split_plan(vec![exception.clone()]);
        let mut steps = carry_26(&plan, &exception, Scripted::Lost { retried: true });
        steps.push(event_read("abc_20261026T090000Z", lost()));
        steps.extend(refetch_after_carry(&exception, None));
        let outcome = split_with(&host, &exception, events, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { left_behind, milestone_moves: Some(m), .. }
                if *left_behind == ["2026-10-26"]
                    && m.milestones == [launch_offer(launch, "2026-10-27")]),
            "Launch offered beside the occurrence PM couldn't say it cancelled"
        );
    }

    /// C9b-2 review F5: the same cleanup unanswered, and a landing that couldn't read Google either.
    /// The mirror keeps that occurrence on the earlier part, as PM can't say it went, which shows
    /// nothing about where it is, so "Launch" is still asked about: unlike a cancel never sent, this
    /// one may have landed (review round 2, R2-F2).
    #[tokio::test]
    async fn a_milestone_whose_cleanup_went_unanswered_is_offered_when_the_landing_couldnt_look() {
        let exception = moved_26("2026-10-27T09:00:00Z", "2026-10-27T10:00:00Z");
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            monday_19(),
            exception.clone(),
        ];
        let (host, launch) = linked_host_holding(events.clone(), "2026-10-20");
        let plan = split_plan(vec![exception.clone()]);
        let mut steps = carry_26(&plan, &exception, Scripted::Lost { retried: true });
        steps.push(event_read("abc_20261026T090000Z", lost()));
        for _ in 0..3 {
            let earlier = linked(vec![plain("2026-10-12T09:00:00+01:00")]);
            steps.push(refetch_read(Scripted::Json(page(earlier, None))));
            steps.push(tail_refetch(Scripted::Json(page(Vec::new(), None))));
        }
        let outcome = split_with(&host, &exception, events, steps).await;
        let kept = host
            .with_conn(|conn| calendar::series_rows(conn, "cal", &["abc"]))
            .unwrap()
            .iter()
            .any(|r| r.start.starts_with("2026-10-27"));
        assert!(kept, "the mirror keeps the occurrence on the earlier part");
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: Some(m) }
                if *warnings == [WriteWarning::MirrorRefreshPending, WriteWarning::LeftOnOldSeries]
                    && *left_behind == ["2026-10-26"]
                    && m.milestones == [launch_offer(launch, "2026-10-27")]),
            "pending, the occurrence said to be left behind, and Launch offered"
        );
    }

    /// C9b-2 review round 3, R3-F2: the same cleanup unanswered, and a landing that reads Google and
    /// still finds the occurrence on the earlier part: "Launch" stays there with it, unasked. What PM
    /// knows it never sent would say nothing here, as the cancel may have gone: only the landing's
    /// own read shows the occurrence stayed.
    #[tokio::test]
    async fn a_milestone_whose_cleanup_went_unanswered_and_stayed_isnt_offered() {
        let exception = moved_26("2026-10-27T09:00:00Z", "2026-10-27T10:00:00Z");
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            monday_19(),
            exception.clone(),
        ];
        let (host, _) = linked_host_holding(events.clone(), "2026-10-20");
        let plan = split_plan(vec![exception.clone()]);
        let mut steps = carry_26(&plan, &exception, Scripted::Lost { retried: true });
        steps.push(event_read("abc_20261026T090000Z", lost()));
        let left = linked(vec![exception.clone()]).pop();
        steps.extend(refetch_after_carry(&exception, left));
        let outcome = split_with(&host, &exception, events, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: None }
                if !warnings.contains(&WriteWarning::MirrorRefreshPending)
                    && *left_behind == ["2026-10-26"]),
            "saved from Google's read, the occurrence left behind, and nothing offered"
        );
    }

    /// C9b-2 review F5: a landing that couldn't read Google keeps the earlier part's rows as they
    /// were, which shows nothing about where an occurrence went, so "Launch" is still asked about.
    #[tokio::test]
    async fn a_milestone_is_offered_when_the_landing_couldnt_look() {
        let (host, launch) = linked_host("2026-10-13");
        let plan = split_plan(Vec::new());
        let mut steps = vec![
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
        ];
        for _ in 0..3 {
            steps.push(refetch_read(Scripted::Json(page(
                vec![linked_october()[0].clone()],
                None,
            ))));
            steps.push(tail_refetch(Scripted::Json(page(Vec::new(), None))));
        }
        let (outcome, _, _) = split_save(&host, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, milestone_moves: Some(m), .. }
                if *warnings == [WriteWarning::MirrorRefreshPending]
                    && m.milestones == [launch_offer(launch, "2026-10-19")]),
            "pending, and Launch offered"
        );
    }

    /// A milestone whose event the mirror can't find keeps its own date, on no row of the series:
    /// that date decides, from the day the change starts on (DECISIONS 17-10 #2).
    #[tokio::test]
    async fn a_milestone_on_no_row_goes_by_its_date() {
        let host = TestHost::holding(&october());
        host.today_is("2026-10-13");
        let later = host
            .with_conn(|conn| {
                let link = Some("abc@google.com".to_string());
                milestones::add(
                    conn,
                    "Atlas",
                    "Earlier",
                    Some("2026-10-10".into()),
                    link.clone(),
                )?;
                milestones::add(conn, "Atlas", "Later", Some("2026-10-28".into()), link)
            })
            .unwrap();
        let plan = split_plan(Vec::new());
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        let offered: Vec<i64> = moves_of(&outcome)
            .map(|m| m.milestones.iter().map(|o| o.id).collect())
            .unwrap_or_default();
        assert!(offered == [later], "Later offered, Earlier not");
    }

    /// C9b-2 review F7: the one GET of the new part finds it deleted in Google: nothing can be moved
    /// there, so the milestones are said to stay.
    #[tokio::test]
    async fn a_new_part_deleted_before_its_get_offers_nothing() {
        let (host, launch) = linked_host("2026-10-13");
        let plan = split_plan(Vec::new());
        let quiet = with(tail_copy(&plan, "\"t1\"", SAVED_AT), "iCalUID", Value::Null);
        let mut steps = landed_split(&plan, quiet);
        let deleted = with(
            tail_copy(&plan, "\"t2\"", SAVED_AT),
            "status",
            json!("cancelled"),
        );
        steps.push(event_read(TAIL, Scripted::Json(deleted)));
        let (outcome, _, _) = split_save(&host, steps).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, milestone_moves: Some(m), .. }
                if *warnings == [WriteWarning::MilestoneNotOffered]
                    && m.new_series_ical.is_none()
                    && m.milestones == [launch_offer(launch, "2026-10-19")]),
            "saved, the milestone said to stay"
        );
    }

    /// C9b-2 critique C1-L8: the user's zone (Honolulu here, 10 hours behind) decides every day, not
    /// the series' (London): the 19-10 occurrence, 08:00 UTC, falls on 18-10 there, so "Launch"
    /// takes 18-10 and is offered, the new part starting that day and the earlier part ending on
    /// 11-10.
    #[tokio::test]
    async fn a_split_offers_by_the_users_days() {
        let (host, launch) = linked_host("2026-10-13");
        host.with_conn(|conn| {
            crate::db::set_setting(conn, crate::settings::TIME_ZONE_KEY, "Pacific/Honolulu")
        })
        .unwrap();
        let plan = split_plan(Vec::new());
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        assert!(
            moves_of(&outcome)
                .is_some_and(|m| m.milestones == [launch_offer(launch, "2026-10-18")]
                    && m.first_day.as_deref() == Some("2026-10-18")
                    && m.earlier_last_day.as_deref() == Some("2026-10-11")),
            "Launch offered on the user's 18-10"
        );
    }

    /// C9b-2 review round 4: today is the user's day too, not the series'. At 03:00 UTC it's still
    /// the evening before in Honolulu, the user's zone, and already the day in London, the series'.
    /// On 14-10 "Launch" takes the 19-10 occurrence, the user's 18-10, and is offered with today the
    /// user's 13-10, by which the words say whether the earlier part's last day has passed. On 12-10
    /// it takes the 12-10 occurrence, still to come at 22:00 on the user's 11-10, a week before the
    /// change starts, so nothing is asked; on the series' 12-10 it would take 19-10 and be asked
    /// about.
    #[tokio::test]
    async fn a_split_reads_today_in_the_users_zone() {
        let in_honolulu_at = |now: &str| {
            let (host, launch) = linked_host("2026-10-13");
            host.with_conn(|conn| {
                crate::db::set_setting(conn, crate::settings::TIME_ZONE_KEY, "Pacific/Honolulu")
            })
            .unwrap();
            host.now_is(now);
            (host, launch)
        };
        let (host, launch) = in_honolulu_at("2026-10-14T03:00:00Z");
        let plan = split_plan(Vec::new());
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        assert!(
            moves_of(&outcome)
                .is_some_and(|m| m.milestones == [launch_offer(launch, "2026-10-18")]
                    && m.today == "2026-10-13"),
            "Launch offered, today the user's 13-10"
        );
        let (host, _) = in_honolulu_at("2026-10-12T03:00:00Z");
        let insert = tail_copy(&plan, "\"t1\"", SAVED_AT);
        let (outcome, _, _) = split_save(&host, landed_split(&plan, insert)).await;
        assert!(
            outcome == saved(),
            "on the user's 11-10: saved, nothing offered"
        );
    }

    /// Save "this and following" retitled from `opened`, the 19-10 occurrence moved, against `host`,
    /// the mirror holding `events`: the split carries the move onto the new part's 19-10 occurrence,
    /// cancels it on the original, and lands, the original re-fetched as its 12-10 occurrence alone.
    async fn split_from_moved(host: &TestHost, opened: &Value, events: Vec<Value>) -> WriteOutcome {
        let plan = Case::new(weekly(), opened.clone())
            .exceptions(vec![opened.clone()])
            .edit(Scope::Following, title("Retro"))
            .unwrap();
        let found = tail_occurrence("2026-10-19T09:00:00+01:00", "\"q1\"");
        let carried = written(
            &found,
            &[
                ("start", opened["start"].clone()),
                ("end", opened["end"].clone()),
            ],
            "\"q2\"",
        );
        let cancelled = written(
            &with(opened.clone(), "status", json!("cancelled")),
            &[],
            "\"c2\"",
        );
        let mut all = reads(opened, &weekly(), vec![weekly(), opened.clone()], events);
        all.extend([
            insert_step(Scripted::Json(tail_copy(&plan, "\"t1\"", SAVED_AT))),
            series_patch(Scripted::Json(cut_copy(&plan))),
            Step::get(
                &format!("/events/{TAIL}/instances"),
                "timeMin=2026-10-18T08%3A00%3A00Z timeMax=2026-10-20T08%3A00%3A00Z maxResults=10",
                page(vec![found], None),
            ),
            Step::write(
                Method::Patch,
                &format!("/events/{TAIL}_20261019T080000Z"),
                "\"q1\"",
                Scripted::Json(carried.clone()),
            ),
            Step::write(
                Method::Patch,
                "/events/abc_20261019T080000Z",
                opened["etag"].as_str().unwrap(),
                Scripted::Json(cancelled),
            ),
            refetch_read(Scripted::Json(page(
                linked(vec![plain("2026-10-12T09:00:00+01:00")]),
                None,
            ))),
            tail_refetch(Scripted::Json(page(
                vec![carried, tail_occurrence("2026-10-26T09:00:00Z", "\"u1\"")],
                None,
            ))),
        ]);
        let script = Script::new(all);
        let (outcome, _) = save(
            host,
            &script,
            opened,
            &weekly(),
            title("Retro"),
            Scope::Following,
        )
        .await;
        script.asked_all();
        outcome
    }

    /// C9b-2 review round 3, R3-F1: the 19-10 occurrence moved to Sunday 18-10, where the user opens
    /// it and splits from. "Launch" takes it (today 14-10): its date is the day the split goes from as
    /// the user sees it, the "this day" the dialog names, though the occurrence's place in the series
    /// is 19-10, so it's asked about, the new part starting on 18-10 where the split carries the move.
    #[tokio::test]
    async fn a_split_from_an_occurrence_moved_earlier_goes_from_the_day_it_shows() {
        let opened = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-18T09:00:00+01:00",
            "2026-10-18T10:00:00+01:00",
        );
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            opened.clone(),
            plain("2026-10-26T09:00:00Z"),
        ];
        let (host, launch) = linked_host_holding(events.clone(), "2026-10-14");
        assert!(
            launch_date(&host, "2026-10-14").as_deref() == Some("2026-10-18"),
            "the fixture's rows put Launch on 18-10"
        );
        let outcome = split_from_moved(&host, &opened, events).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: Some(m) }
                if warnings.is_empty() && left_behind.is_empty()
                    && m.milestones == [launch_offer(launch, "2026-10-18")]
                    && m.first_day.as_deref() == Some("2026-10-18")
                    && m.earlier_last_day.as_deref() == Some("2026-10-12")),
            "Launch offered from the day the occurrence shows"
        );
    }

    /// C9b-2 review round 4: the same in Honolulu, the user's zone, 10 hours behind. The occurrence
    /// moved to Sunday 18-10, 09:00 London time, shows in Honolulu at 22:00 on Saturday 17-10,
    /// though its place in the series falls on the user's 18-10. "Launch" takes it (today 14-10) on
    /// 17-10, the day the split goes from as the user sees it, so it's asked about, the new part
    /// starting on 17-10 and the earlier part ending on 11-10; by the series' days the split would
    /// go from 18-10, after Launch's date.
    #[tokio::test]
    async fn a_split_from_an_occurrence_moved_earlier_goes_from_the_users_day_it_shows() {
        let opened = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-18T09:00:00+01:00",
            "2026-10-18T10:00:00+01:00",
        );
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            opened.clone(),
            plain("2026-10-26T09:00:00Z"),
        ];
        let (host, launch) = linked_host_holding(events.clone(), "2026-10-14");
        host.with_conn(|conn| {
            crate::db::set_setting(conn, crate::settings::TIME_ZONE_KEY, "Pacific/Honolulu")
        })
        .unwrap();
        assert!(
            launch_date(&host, "2026-10-14").as_deref() == Some("2026-10-17"),
            "the fixture's rows put Launch on the user's 17-10"
        );
        let outcome = split_from_moved(&host, &opened, events).await;
        assert!(
            matches!(&outcome, WriteOutcome::Saved { warnings, left_behind, milestone_moves: Some(m) }
                if warnings.is_empty() && left_behind.is_empty()
                    && m.milestones == [launch_offer(launch, "2026-10-17")]
                    && m.first_day.as_deref() == Some("2026-10-17")
                    && m.earlier_last_day.as_deref() == Some("2026-10-11")),
            "Launch offered from the user's day the occurrence shows"
        );
    }

    /// C9b-2 review round 3, R3-F1's other half: the 19-10 occurrence moved to Wednesday 21-10. The
    /// split goes from the earlier of the two days, its place in the series, so "Prep", dated 20-10
    /// on no row, is asked about; "Earlier", dated 18-10, isn't.
    #[tokio::test]
    async fn a_split_from_an_occurrence_moved_later_goes_from_its_place_in_the_series() {
        let opened = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-21T09:00:00+01:00",
            "2026-10-21T10:00:00+01:00",
        );
        let events = vec![
            plain("2026-10-12T09:00:00+01:00"),
            opened.clone(),
            plain("2026-10-26T09:00:00Z"),
        ];
        let host = TestHost::holding(&events);
        host.today_is("2026-10-14");
        let prep = host
            .with_conn(|conn| {
                let link = Some("abc@google.com".to_string());
                milestones::add(
                    conn,
                    "Atlas",
                    "Earlier",
                    Some("2026-10-18".into()),
                    link.clone(),
                )?;
                milestones::add(conn, "Atlas", "Prep", Some("2026-10-20".into()), link)
            })
            .unwrap();
        let outcome = split_from_moved(&host, &opened, events).await;
        let offered: Vec<i64> = moves_of(&outcome)
            .map(|m| m.milestones.iter().map(|o| o.id).collect())
            .unwrap_or_default();
        assert!(offered == [prep], "Prep offered, Earlier not");
    }

    /// "This event" and "All events" never offer a move: nothing gets a new identity.
    #[tokio::test]
    async fn this_and_all_saves_never_offer() {
        let (host, _) = linked_host("2026-10-13");
        let (series, instance) = (weekly(), monday_19());
        let mut steps = october_reads();
        steps.push(Step::write(
            Method::Patch,
            "/events/abc_20261019T080000Z",
            instance["etag"].as_str().unwrap(),
            Scripted::Json(retitled(&instance, "\"i2\"")),
        ));
        let script = Script::new(steps);
        let (outcome, _) = save(
            &host,
            &script,
            &instance,
            &series,
            title("Retro"),
            Scope::This,
        )
        .await;
        script.asked_all();
        assert!(outcome == saved(), "this event: saved, nothing offered");
        let (host, _) = linked_host("2026-10-13");
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
        assert!(outcome == saved(), "all events: saved, nothing offered");
    }
}
