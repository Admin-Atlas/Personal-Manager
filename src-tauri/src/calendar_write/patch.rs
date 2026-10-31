// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The body of a save, and the rules for an event that changed in Google while the editor was open.
//!
//! Every PATCH is built against a fresh `events.get` of the event (rule R1), never the mirror, which
//! clips titles and descriptions on purpose. It carries only the fields the user touched whose value
//! differs from what Google holds now; start and end always travel together. PM never uses
//! `events.update` (a PUT), because an update that omits a field removes it: a label, an attachment,
//! a field PM doesn't know about yet.
//!
//! A changed event (its etag moved) is handled before sending (rule R2):
//! - an update goes ahead only when none of the fields it sends changed in Google
//!   ([`update_check`]), and is otherwise a conflict, never an overwrite; a deleted event is gone;
//! - a delete sends no fields, so it has its own rule: it goes ahead only when what the user saw
//!   still matches the fresh copy ([`delete_still_matches`]). Re-deleting just because the etag moved
//!   would delete a version of the event nobody looked at.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use super::attendees;
use super::call;
use super::dto::{
    ColourDraft, EventPatchDraft, NewEventDraft, RemindersDraft, SeenSummary, SeenTimes, TimeDraft,
    Visibility,
};
use super::time::{self, TimeError};
use crate::calendar::Reminder;
use crate::calendar_recur::expand::sync_start;
use crate::calendar_recur::spec::{rule_from_spec, SpecError};
use crate::calendar_recur::wire::RepeatRule;
use crate::calendar_recur::SeriesStart;

/// The only fields a single-event save sends: the guest list and what guests may do among them
/// (C12), the event's notifications and its colour (C13), and its video call (C18: a new Meet call
/// asked for, or `null` to remove one). Never a label (`eventLabelId`): PM doesn't set Google's
/// labels (the C13 spec's Adopted list).
pub const PATCHABLE: &[&str] = &[
    "summary",
    "location",
    "description",
    "start",
    "end",
    "transparency",
    "visibility",
    "attendees",
    "guestsCanModify",
    "guestsCanInviteOthers",
    "guestsCanSeeOtherGuests",
    "reminders",
    "colorId",
    "conferenceData",
];

/// What a save that makes a one-off repeat may send (C11 §10): [`PATCHABLE`]'s fields and the rule,
/// its own list so that guests changed in the same save go too (C12 critiques C1-H4, C2-M4).
/// `repeat_patchable_is_patchable_and_the_rule` pins it.
pub const REPEAT_PATCHABLE: &[&str] = &[
    "summary",
    "location",
    "description",
    "start",
    "end",
    "transparency",
    "visibility",
    "attendees",
    "guestsCanModify",
    "guestsCanInviteOthers",
    "guestsCanSeeOtherGuests",
    "reminders",
    "colorId",
    "conferenceData",
    "recurrence",
];

/// What a create may send: a save's fields, the repeat rule (C11), the guests with what they may do
/// (C12d), the notifications and colour (C13), and a request for a new Meet call (C18b). Still no
/// label or private notes. Widening it is a decision, never a slip:
/// `creatable_is_the_c11_list_guests_notifications_and_colour` pins it to exactly this, and the
/// create plan's own refusal test names what it still refuses (C10 critique 1 L10).
pub const CREATABLE: &[&str] = &[
    "summary",
    "location",
    "description",
    "start",
    "end",
    "transparency",
    "visibility",
    "recurrence",
    "attendees",
    "guestsCanModify",
    "guestsCanInviteOthers",
    "guestsCanSeeOtherGuests",
    "reminders",
    "colorId",
    "conferenceData",
];

/// What a drop may send (C16a): its start and end, nothing else. A move's body is checked against it
/// before it goes, failing closed.
pub const TIMING_KEYS: &[&str] = &["start", "end"];

/// The fields an Undo may put back: the C6 set, the event's core fields, and its notifications and
/// colour (C16d, Q3 as the C16 critique restated it: C13 gave both an absent value PM can send,
/// `{"useDefault": true}` and `colorId: null`). A write that sent anything else (guests and what
/// they may do, how it repeats) is offered no Undo: putting a guest list back would email people
/// again. Its own list, not [`PATCHABLE`]: nothing becomes undoable by being added there.
/// `undoable_is_the_core_fields_notifications_and_colour` pins it.
pub const UNDOABLE: &[&str] = &[
    "summary",
    "location",
    "description",
    "start",
    "end",
    "transparency",
    "visibility",
    "reminders",
    "colorId",
];

/// Google's limits on an event's own notifications (the events reference): five at most, each 0 to
/// 40 320 minutes (four weeks) before.
pub const MAX_REMINDERS: usize = 5;
pub const MAX_REMINDER_MINUTES: i64 = 40_320;
/// The ways PM adds a notification: Google's two, a pop-up and an email.
pub const REMINDER_METHODS: &[&str] = &["popup", "email"];

/// Why a draft's body can't be built. Shown to the user as is: the inner words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    Time(TimeError),
    Reminders(ReminderError),
    Colour(ColourError),
    /// A Meet call was asked for without the session's request id (C18): never on a path the
    /// backend put its id in, so nothing is sent rather than a call asked for twice.
    MeetUnready,
}

/// Why a draft's notifications can't be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderError {
    /// More than Google's five.
    TooMany,
    /// The calendar's defaults and a list of its own at once.
    DefaultWithOwn,
    /// A new one outside 0 to four weeks before.
    OutOfRange,
    /// A new one sent a way PM doesn't add (an old text message).
    UnknownMethod,
    /// A new one the same as another on the list (C13 critique C1-L7).
    Duplicate,
    /// The copy the list goes onto holds one PM can't read, which a list sent whole would lose (C13
    /// critique C1-M6).
    Unreadable,
}

/// Why a draft's colour can't be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourError {
    /// Not one of Google's eleven.
    UnknownColour,
    /// The copy it goes onto has a Google Calendar label, which sets its colour (R-LABEL): a colour
    /// under it wouldn't show, and PM doesn't change labels.
    Labelled,
}

impl From<TimeError> for PatchError {
    fn from(e: TimeError) -> Self {
        PatchError::Time(e)
    }
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words = match self {
            PatchError::Time(e) => return e.fmt(f),
            PatchError::Reminders(ReminderError::TooMany) => {
                "Google allows up to 5 notifications on an event."
            }
            PatchError::Reminders(ReminderError::DefaultWithOwn) => {
                "An event uses its calendar's default notifications or its own, not both."
            }
            PatchError::Reminders(ReminderError::OutOfRange) => {
                "Google allows notifications up to 4 weeks before the event."
            }
            PatchError::Reminders(ReminderError::UnknownMethod) => {
                "PM can only add notifications and emails."
            }
            PatchError::Reminders(ReminderError::Duplicate) => {
                "The same notification is on the list twice. Remove one."
            }
            PatchError::Reminders(ReminderError::Unreadable) => {
                "PM can't read one of this event's notifications, so it can't change them. Nothing \
                 was saved."
            }
            PatchError::Colour(ColourError::UnknownColour) => {
                "That isn't one of Google Calendar's colours, so nothing was saved."
            }
            PatchError::Colour(ColourError::Labelled) => {
                "This event has a Google Calendar label, which sets its colour. Change it in Google \
                 Calendar."
            }
            PatchError::MeetUnready => {
                "PM couldn't get the video call ready, so nothing was sent. Try again."
            }
        };
        f.write_str(words)
    }
}

/// Why a create's body can't be built. Shown to the user as is: the inner words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertError {
    Time(TimeError),
    Rule(SpecError),
    /// A one-off given a repeat, its time left alone, whose start or end has no zone, with none to
    /// read it in: not the calendar's, nor this computer's (C11d review 1).
    NoEventZone,
    /// A guest PM can't invite: not an address, or listed twice (C12d). Its words name no address.
    Guest(attendees::GuestError),
    /// Notifications or a colour Google wouldn't take (C13).
    Draft(PatchError),
}

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertError::Time(e) => e.fmt(f),
            InsertError::Rule(e) => e.fmt(f),
            InsertError::Guest(e) => e.fmt(f),
            InsertError::Draft(e) => e.fmt(f),
            InsertError::NoEventZone => write!(
                f,
                "PM can't tell which time zone this event is in, so it can't make it repeat."
            ),
        }
    }
}

impl From<TimeError> for InsertError {
    fn from(e: TimeError) -> Self {
        InsertError::Time(e)
    }
}

impl From<SpecError> for InsertError {
    fn from(e: SpecError) -> Self {
        InsertError::Rule(e)
    }
}

/// The insert body for `draft`. Google's `start`/`end` come from [`time::resolve`] (explicit zones,
/// a skipped hour refused, a repeated hour's first occurrence, an all-day end the day after the
/// last day); empty text is left out, so Google keeps no empty strings. No `id`: the insert request
/// writes PM's own in ([`super::plan::insert_event`]).
///
/// A repeating event starts on the first day its rule makes ([`sync_start`]: "Weekly on Monday and
/// Wednesday" chosen on a Tuesday starts on the Wednesday, A27), its end moved the same number of
/// days, and carries one rule line and nothing else (no EXDATE or RDATE). A timed one names its
/// zone on both nodes, as every timed node does; an all-day one sends dates only (probe P-R1).
///
/// Guests (C12d) go as new entries, in the order added ([`attendees::new_list`]), and only when
/// there are any; each guest permission only where it isn't Google's default (C12 §3.2).
///
/// Notifications (C13) go only as a list of the event's own, checked as any new ones are; the
/// calendar's defaults send nothing, since Google gives a new event those (INFERRED, probe E9). A
/// colour goes only when it's one of Google's eleven, never the calendar's own.
///
/// A Meet call (C18b) goes only when the draft asks for one, under the request id the create
/// command put in; asked for without one, nothing is built ([`PatchError::MeetUnready`]), so a call
/// is never asked for twice under two ids.
pub fn insert_body(draft: &NewEventDraft) -> Result<Map<String, Value>, InsertError> {
    let (time, rule) = match &draft.recurrence {
        None => (draft.time.clone(), None),
        Some(rule) => {
            let spec = rule.to_spec()?;
            let asked = time::series_start(&draft.time)?;
            let synced = sync_start(&spec, &asked)?;
            let moved = (synced.date() - asked.date()).num_days();
            (
                time::shifted(&draft.time, moved)?,
                Some(rule_from_spec(&spec, &synced)?),
            )
        }
    };
    let when = time::resolve(&time)?;
    let guests = attendees::new_list(&draft.guests).map_err(InsertError::Guest)?;
    let mut body = Map::new();
    for (key, text) in [
        ("summary", &draft.summary),
        ("location", &draft.location),
        ("description", &draft.description),
    ] {
        // As typed, untrimmed, as a save sends it; only text that is all blank is left out.
        if !text.trim().is_empty() {
            body.insert(key.into(), Value::String(text.clone()));
        }
    }
    body.insert("start".into(), when.start);
    body.insert("end".into(), when.end);
    if let Some(show_as) = draft.show_as {
        body.insert(
            "transparency".into(),
            Value::String(show_as.as_transparency().into()),
        );
    }
    if let Some(visibility) = draft.visibility.filter(|v| *v != Visibility::Default) {
        body.insert(
            "visibility".into(),
            Value::String(visibility.as_param().into()),
        );
    }
    if let Some(line) = rule {
        body.insert("recurrence".into(), Value::from(vec![line]));
    }
    if !guests.is_empty() {
        body.insert("attendees".into(), Value::from(guests));
    }
    for (key, default, new) in [
        ("guestsCanModify", false, draft.guests_can_modify),
        (
            "guestsCanInviteOthers",
            true,
            draft.guests_can_invite_others,
        ),
        (
            "guestsCanSeeOtherGuests",
            true,
            draft.guests_can_see_other_guests,
        ),
    ] {
        // Google's own default goes unsaid, as a save leaves an unchanged one.
        if let Some(new) = new.filter(|new| *new != default) {
            body.insert(key.into(), Value::Bool(new));
        }
    }
    let refused = |e| InsertError::Draft(PatchError::Reminders(e));
    if let Some(reminders) = &draft.reminders {
        check_reminders(reminders, &[]).map_err(refused)?;
        if !reminders.use_default {
            body.insert("reminders".into(), reminders_value(reminders));
        }
    }
    if let Some(ColourDraft::Colour { id }) = &draft.colour {
        if !is_event_colour(id) {
            return Err(InsertError::Draft(PatchError::Colour(
                ColourError::UnknownColour,
            )));
        }
        body.insert("colorId".into(), Value::String(id.clone()));
    }
    if draft.meet {
        let request = draft
            .meet_request
            .as_deref()
            .ok_or(InsertError::Draft(PatchError::MeetUnready))?;
        body.insert("conferenceData".into(), call::meet_request_body(request));
    }
    Ok(body)
}

/// Make a one-off repeat by `rule` (C11 §10): its `start`, `end` and `recurrence` into `body`, and
/// the start it repeats from. The times are the draft's (`time`), or the event's own as the editor
/// showed them ([`time::time_draft_of`]: each node's zone, else `fallback_zone`, the calendar's or
/// else this computer's as the editor had it; never UTC, R6), moved to the first day the rule makes
/// ([`sync_start`]) with their length kept, as a new repeating event's are. Both nodes are always
/// sent, a timed one naming its `timeZone` (G6): Google's own node, which a save hands back for a
/// half the user left alone, may have none. A switch between timed and all-day sends the other
/// kind's keys as `null`, as any save does.
pub fn add_repeat(
    body: &mut Map<String, Value>,
    fresh: &Value,
    time: Option<&TimeDraft>,
    rule: &RepeatRule,
    fallback_zone: Option<&str>,
) -> Result<SeriesStart, InsertError> {
    let own;
    let time = match time {
        Some(time) => time,
        // The editor opened on these very nodes in this very zone, so only a node with no zone, and
        // no zone to read it in, can't be read here.
        None => {
            own = time::time_draft_of(fresh, fallback_zone.unwrap_or(""))
                .ok_or(InsertError::NoEventZone)?;
            &own
        }
    };
    let spec = rule.to_spec()?;
    let asked = time::series_start(time)?;
    let synced = sync_start(&spec, &asked)?;
    let moved = (synced.date() - asked.date()).num_days();
    let when = time::resolve(&time::shifted(time, moved)?)?;
    let line = rule_from_spec(&spec, &synced)?;
    let (mut start, mut end) = (when.start, when.end);
    let was_all_day = fresh.get("start").is_some_and(|s| s.get("date").is_some());
    if was_all_day != when.all_day {
        for node in [&mut start, &mut end] {
            drop_other_kind(node);
        }
    }
    body.insert("start".into(), start);
    body.insert("end".into(), end);
    body.insert("recurrence".into(), Value::from(vec![line]));
    Ok(synced)
}

/// The PATCH body for `draft` against `fresh`, Google's current copy. Empty means Google already
/// holds exactly this, and nothing should be sent.
///
/// Switching between a timed and an all-day event sends the other key as `null` so Google drops it
/// rather than holding both (INFERRED: confirmed or refuted by live test L6).
///
/// Notifications go whole, as the draft lists them, and only when they differ from Google's in more
/// than order; a colour goes as one of Google's eleven, or `null` for the calendar's own (INFERRED:
/// PATCH clears a field sent as null, probe E15b). Neither goes onto a copy they can't be checked
/// against ([`check_draft_extras`]). A video call goes as [`call::meet_value`] says (C18): a request
/// for a new Meet call, or `null` to remove one, only where the copy holds otherwise.
pub fn build_patch(
    fresh: &Value,
    draft: &EventPatchDraft,
) -> Result<Map<String, Value>, PatchError> {
    patch_onto(fresh, None, draft)
}

/// [`build_patch`] onto `master`, a repeating event's own copy, for a draft made in an editor opened
/// on `occurrence`: a notification either copy holds goes back as Google holds it, since the editor
/// showed the occurrence's (C13 critique C2-L6).
pub fn series_patch(
    master: &Value,
    occurrence: &Value,
    draft: &EventPatchDraft,
) -> Result<Map<String, Value>, PatchError> {
    patch_onto(master, Some(occurrence), draft)
}

fn patch_onto(
    fresh: &Value,
    shown: Option<&Value>,
    draft: &EventPatchDraft,
) -> Result<Map<String, Value>, PatchError> {
    check_draft_extras(fresh, shown, draft)?;
    let mut body = Map::new();
    let mut text = |key: &str, new: &Option<String>| {
        if let Some(new) = new {
            if text_of(fresh, key) != new.as_str() {
                body.insert(key.to_string(), Value::String(new.clone()));
            }
        }
    };
    text("summary", &draft.summary);
    text("location", &draft.location);
    text("description", &draft.description);
    if let Some(show_as) = draft.show_as {
        if transparency_of(fresh) != show_as.as_transparency() {
            body.insert(
                "transparency".into(),
                Value::String(show_as.as_transparency().into()),
            );
        }
    }
    if let Some(visibility) = draft.visibility {
        if visibility_of(fresh) != visibility.as_param() {
            body.insert(
                "visibility".into(),
                Value::String(visibility.as_param().into()),
            );
        }
    }
    let ops = draft.guest_ops();
    if !ops.is_empty() {
        // R1: Google's own list with the user's changes, never the editor's list.
        let list = Value::from(attendees::apply(fresh.get("attendees"), ops));
        if attendees::guest_set(Some(&list)) != attendees::guest_set(fresh.get("attendees")) {
            body.insert("attendees".into(), list);
        }
    }
    for (key, default, new) in [
        ("guestsCanModify", false, draft.guests_can_modify),
        (
            "guestsCanInviteOthers",
            true,
            draft.guests_can_invite_others,
        ),
        (
            "guestsCanSeeOtherGuests",
            true,
            draft.guests_can_see_other_guests,
        ),
    ] {
        if let Some(new) = new {
            if bool_of(fresh.get(key), default) != new {
                body.insert(key.into(), Value::Bool(new));
            }
        }
    }
    if let Some(reminders) = &draft.reminders {
        let mut value = reminders_value(reminders);
        if !reminders_eq(Some(&value), fresh.get("reminders")) {
            // `events.patch` merges an object it is sent (INFERRED, as for start and end): the
            // defaults alone would leave Google's own list beside them, so it goes as an empty one
            // (probe E9h).
            let held = fresh
                .pointer("/reminders/overrides")
                .and_then(Value::as_array)
                .is_some_and(|list| !list.is_empty());
            if reminders.use_default && held {
                value["overrides"] = Value::Array(Vec::new());
            }
            body.insert("reminders".into(), value);
        }
    }
    match &draft.colour {
        Some(ColourDraft::Calendar) if !text_of(fresh, "colorId").is_empty() => {
            body.insert("colorId".into(), Value::Null);
        }
        Some(ColourDraft::Colour { id }) if text_of(fresh, "colorId") != id.as_str() => {
            body.insert("colorId".into(), Value::String(id.clone()));
        }
        _ => {}
    }
    // The call on the copy it goes to: a series' own for every occurrence, an occurrence's for one.
    if let Some(call) = call::meet_value(fresh, draft)? {
        body.insert("conferenceData".into(), call);
    }
    if let Some(draft_time) = &draft.time {
        // Against Google's current start and end, so the half the user didn't change is sent exactly
        // as Google holds it (time::resolve_against).
        let when = time::resolve_against(draft_time, fresh.get("start"), fresh.get("end"))?;
        let unchanged = when_eq(fresh.get("start"), Some(&when.start))
            && when_eq(fresh.get("end"), Some(&when.end));
        if !unchanged {
            let was_all_day = fresh.get("start").is_some_and(|s| s.get("date").is_some());
            let (mut start, mut end) = (when.start, when.end);
            if was_all_day != when.all_day {
                for node in [&mut start, &mut end] {
                    drop_other_kind(node);
                }
            }
            body.insert("start".into(), start);
            body.insert("end".into(), end);
        }
    }
    Ok(body)
}

/// Whether a draft's notifications and colour may go onto `fresh`, the copy they're sent to (C13),
/// `shown` being the copy the editor showed when that's another (an occurrence saved onto its
/// series). Its notifications need Google's own on `fresh` to be readable, since the list goes whole
/// and would lose one PM can't read (C13 critique C1-M6), and are checked by [`check_reminders`]
/// against what either copy holds. A colour can't go onto a copy with a Google Calendar label, which
/// sets its colour and which PM doesn't change (R-LABEL), and must be one of Google's eleven.
/// [`build_patch`] runs it; so does a "this and following" change, whose new part takes the draft's
/// values without a body of its own.
pub fn check_draft_extras(
    fresh: &Value,
    shown: Option<&Value>,
    draft: &EventPatchDraft,
) -> Result<(), PatchError> {
    if let Some(reminders) = &draft.reminders {
        let held = reminders_of(fresh).ok_or(PatchError::Reminders(ReminderError::Unreadable))?;
        let other = shown.and_then(reminders_of).map(|r| r.overrides);
        let held = held_by_either(held.overrides, other.unwrap_or_default());
        check_reminders(reminders, &held).map_err(PatchError::Reminders)?;
    }
    if let Some(colour) = &draft.colour {
        if !text_of(fresh, "eventLabelId").is_empty() {
            return Err(PatchError::Colour(ColourError::Labelled));
        }
        if let ColourDraft::Colour { id } = colour {
            if !is_event_colour(id) {
                return Err(PatchError::Colour(ColourError::UnknownColour));
            }
        }
    }
    Ok(())
}

/// Every notification `a` holds, and each of `b`'s beyond as many of it as `a` holds: a list either
/// copy holds, counted one for one.
fn held_by_either(a: Vec<Reminder>, b: Vec<Reminder>) -> Vec<Reminder> {
    let count = |list: &[Reminder], r: &Reminder| list.iter().filter(|x| *x == r).count();
    let mut out = a;
    for r in &b {
        if count(&out, r) < count(&b, r) {
            out.push(r.clone());
        }
    }
    out
}

// --- notifications and colour (C13) ---

/// An event's notifications as Google holds them: none at all reads as the calendar's defaults, and
/// `useDefault` left out as true only where there's no list of its own (INFERRED). `None` when one
/// can't be read (no method, minutes that aren't a whole number, a shape PM doesn't know), so a save
/// that sends the list whole never loses it.
pub fn reminders_of(event: &Value) -> Option<RemindersDraft> {
    reminders_read(event.get("reminders"))
}

/// [`reminders_of`] for a `reminders` value itself, absent or null being the defaults.
fn reminders_read(value: Option<&Value>) -> Option<RemindersDraft> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Some(RemindersDraft {
            use_default: true,
            overrides: Vec::new(),
        });
    };
    value.as_object()?;
    let overrides = match value.get("overrides").filter(|v| !v.is_null()) {
        None => Vec::new(),
        Some(list) => list
            .as_array()?
            .iter()
            .map(|o| {
                Some(Reminder {
                    method: o.get("method")?.as_str()?.to_string(),
                    minutes: o.get("minutes")?.as_i64()?,
                })
            })
            .collect::<Option<Vec<_>>>()?,
    };
    let use_default = match value.get("useDefault") {
        Some(flag) => flag.as_bool()?,
        None => overrides.is_empty(),
    };
    Some(RemindersDraft {
        use_default,
        overrides,
    })
}

/// The `reminders` value for `draft`: `{"useDefault": true}`, or `{"useDefault": false,
/// "overrides": […]}` with each `{method, minutes}` in the draft's order. The whole list, since
/// Google replaces a list it is sent.
pub fn reminders_value(draft: &RemindersDraft) -> Value {
    if draft.use_default {
        return serde_json::json!({ "useDefault": true });
    }
    let list: Vec<Value> = draft
        .overrides
        .iter()
        .map(|r| serde_json::json!({ "method": r.method, "minutes": r.minutes }))
        .collect();
    serde_json::json!({ "useDefault": false, "overrides": list })
}

/// Whether `draft` may go onto an event whose own notifications are `held` (none for a new event):
/// at most five, not the calendar's defaults and a list at once, and each one new to it (not among
/// `held`, counted one for one) a way PM adds one, 0 to four weeks before, and not on the list
/// twice. One Google holds goes back as it is, whatever it is.
pub fn check_reminders(draft: &RemindersDraft, held: &[Reminder]) -> Result<(), ReminderError> {
    if draft.use_default && !draft.overrides.is_empty() {
        return Err(ReminderError::DefaultWithOwn);
    }
    if draft.overrides.len() > MAX_REMINDERS {
        return Err(ReminderError::TooMany);
    }
    let key = |r: &Reminder| (r.method.clone(), r.minutes);
    let mut left: BTreeMap<(String, i64), usize> = BTreeMap::new();
    for r in held {
        *left.entry(key(r)).or_default() += 1;
    }
    let mut listed: BTreeMap<(String, i64), usize> = BTreeMap::new();
    for r in &draft.overrides {
        *listed.entry(key(r)).or_default() += 1;
    }
    for r in &draft.overrides {
        if let Some(n) = left.get_mut(&key(r)).filter(|n| **n > 0) {
            *n -= 1;
            continue;
        }
        if !REMINDER_METHODS.contains(&r.method.as_str()) {
            return Err(ReminderError::UnknownMethod);
        }
        if !(0..=MAX_REMINDER_MINUTES).contains(&r.minutes) {
            return Err(ReminderError::OutOfRange);
        }
        if listed.get(&key(r)).is_some_and(|n| *n > 1) {
            return Err(ReminderError::Duplicate);
        }
    }
    Ok(())
}

/// Whether two `reminders` values say the same: both the calendar's defaults, or the same
/// notifications in any order (Google may hand a list back in another order than it was sent).
/// Absent or null is the defaults, and a list left out an empty one (C13 critique C1-L7). A value
/// PM can't read is the same only as itself. Every comparison of notifications goes through here:
/// [`field_eq`], and the repeating-event planners'.
pub(crate) fn reminders_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let sorted = |v: Option<&Value>| {
        reminders_read(v).map(|r| {
            let mut list: Vec<(String, i64)> = r
                .overrides
                .into_iter()
                .map(|o| (o.method, o.minutes))
                .collect();
            list.sort_unstable();
            (r.use_default, list)
        })
    };
    match (sorted(a), sorted(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.filter(|v| !v.is_null()) == b.filter(|v| !v.is_null()),
    }
}

/// Whether `id` is one of Google's eleven event colours: "1" to "11", digits only, no leading zero.
pub fn is_event_colour(id: &str) -> bool {
    !id.starts_with('0')
        && id.chars().all(|c| c.is_ascii_digit())
        && matches!(id.parse::<u8>(), Ok(1..=11))
}

/// The keys a save's conflict check compares: those it sends, and with a colour the label too,
/// which a colour saved while someone gave the event a label in Google would vanish under (C13).
pub(crate) fn watched(body: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = body.keys().cloned().collect();
    if body.contains_key("colorId") && !body.contains_key("eventLabelId") {
        keys.push("eventLabelId".into());
    }
    keys
}

/// A start or end node sent onto an event of the other kind (timed onto all-day, or back), with the
/// other kind's keys set to null: `events.patch` merges nested objects, so a key left out would stay.
pub(crate) fn drop_other_kind(node: &mut Value) {
    let all_day = node.get("date").is_some_and(|d| !d.is_null());
    let dropped: &[&str] = if all_day {
        &["dateTime", "timeZone"]
    } else {
        &["date"]
    };
    if let Some(obj) = node.as_object_mut() {
        for key in dropped {
            obj.insert((*key).to_string(), Value::Null);
        }
    }
}

/// Whether Google's copy is a deleted event (`get` still returns those, details and all, so they can
/// be restored). Nothing may be saved onto it, and it can't be deleted again.
pub fn is_gone(fresh: &Value) -> bool {
    fresh.get("status").and_then(Value::as_str) == Some("cancelled")
}

/// The answer to "may this update still go ahead?" after Google's copy turned out to be newer than
/// the one the editor opened on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// Nothing the save sends changed in Google: send it on the fresh etag.
    Proceed,
    /// Deleted in Google meanwhile.
    Gone,
    /// Google changed these fields, which the save also sends. Nothing is overwritten.
    Conflict(Vec<String>),
}

/// Decide an update against `base` (the copy the editor opened on) and `fresh` (Google's copy now).
/// Fields the save doesn't send never count: a location changed in Google doesn't stop a title edit,
/// and both changes are kept. A colour counts the label beside it ([`watched`]).
pub fn update_check(base: &Value, fresh: &Value, body: &Map<String, Value>) -> UpdateCheck {
    if is_gone(fresh) {
        return UpdateCheck::Gone;
    }
    let changed: Vec<String> = watched(body)
        .into_iter()
        .filter(|key| !field_eq(key, base.get(key.as_str()), fresh.get(key.as_str())))
        .collect();
    if changed.is_empty() {
        UpdateCheck::Proceed
    } else {
        UpdateCheck::Conflict(changed)
    }
}

/// Whether `fresh` already holds everything `body` would set: after a save whose answer never
/// arrived (a dropped connection), this tells "it landed" from "send it again". A deleted copy never
/// counts as landed, whatever its details still say.
pub fn already_applied(fresh: &Value, body: &Map<String, Value>) -> bool {
    !is_gone(fresh)
        && body
            .iter()
            .all(|(key, value)| field_eq(key, Some(value), fresh.get(key.as_str())))
}

/// The keys of `after` (Google's values of the fields a write sent, as its reply held them) that
/// `fresh` no longer holds: empty when Google still holds what the write left, so an Undo may go
/// ahead (C16a). Compared the way Google means each field ([`field_eq`]); a field the write didn't
/// send never counts (a title edited in Google after a move doesn't stop the move's Undo).
pub fn still_holds(fresh: &Value, after: &Map<String, Value>) -> Vec<String> {
    after
        .iter()
        .filter(|(key, value)| !field_eq(key, Some(value), fresh.get(key.as_str())))
        .map(|(key, _)| key.clone())
        .collect()
}

/// The body that puts back `before` (Google's values of the fields a write sent, from before it;
/// `null` where it had none) onto `fresh`, Google's copy now (C16a). A start or end goes back as
/// Google's own node, with every key the node now has that the old one lacked sent as `null` (a
/// `timeZone` the write added, the other kind's keys after a switch): `events.patch` merges nested
/// objects, so a key left out would stay. Text that was absent goes back as `""`, as a save clears
/// it; busy or free and visibility as Google's defaults. Notifications go back as a save sends them
/// (C16d): the calendar's defaults when there were none of its own, over a list Google holds now as
/// an empty one; the colour as it was, or `null` (the calendar's) when there was none. `None` for a
/// field PM doesn't put back, a start or end that was never there, or notifications it can't read.
pub fn undo_body(fresh: &Value, before: &Map<String, Value>) -> Option<Map<String, Value>> {
    let mut body = Map::new();
    for (key, was) in before {
        let value = match key.as_str() {
            "start" | "end" => {
                let was = was.as_object()?;
                let mut node = was.clone();
                if let Some(now) = fresh.get(key.as_str()).and_then(Value::as_object) {
                    for extra in now.keys().filter(|k| !was.contains_key(k.as_str())) {
                        node.insert(extra.clone(), Value::Null);
                    }
                }
                Value::Object(node)
            }
            "summary" | "location" | "description" => {
                Value::String(was.as_str().unwrap_or("").to_string())
            }
            "transparency" => Value::String(was.as_str().unwrap_or("opaque").to_string()),
            "visibility" => Value::String(was.as_str().unwrap_or("default").to_string()),
            "reminders" => {
                let draft = reminders_read(Some(was))?;
                let mut value = reminders_value(&draft);
                let held = fresh
                    .pointer("/reminders/overrides")
                    .and_then(Value::as_array)
                    .is_some_and(|list| !list.is_empty());
                if draft.use_default && held {
                    value["overrides"] = Value::Array(Vec::new());
                }
                value
            }
            "colorId" => match was.as_str().filter(|id| !id.is_empty()) {
                Some(id) => Value::String(id.to_string()),
                None => Value::Null,
            },
            _ => return None,
        };
        body.insert(key.clone(), value);
    }
    Some(body)
}

/// Whether `now` holds any field `body` changed from `before` (the copy it went out on), as sent: a
/// field sent as `before` already had it says nothing either way. Beside [`already_applied`] false,
/// Google applied the save without keeping all of it (a colour it wouldn't clear beside a new
/// title, say), which is never "nothing changed" (C13 review round 1).
pub fn any_applied(before: &Value, now: &Value, body: &Map<String, Value>) -> bool {
    !is_gone(now)
        && body.iter().any(|(key, value)| {
            !field_eq(key, before.get(key.as_str()), Some(value))
                && field_eq(key, Some(value), now.get(key.as_str()))
        })
}

/// Whether `now` still holds what `before` held for every field `body` sends: Google kept the event
/// as it was. Only then is a save Google answered with success one it ignored (C13 critique C2-H1).
pub fn none_applied(before: &Value, now: &Value, body: &Map<String, Value>) -> bool {
    body.keys()
        .all(|key| field_eq(key, before.get(key.as_str()), now.get(key.as_str())))
}

/// The answer to "may this still go ahead?" after Google's copy turned out to be newer: a delete
/// ([`delete_still_matches`]) or a move ([`timing_still_matches`]), neither of which sends the
/// fields it is checked on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteCheck {
    /// What the user saw is still what's there (only the etag moved, say a reminder): delete it.
    Matches,
    /// Already gone (deleted, or cancelled from its series).
    Gone,
    /// It changed in ways the user didn't see: these fields. Nothing is deleted.
    Changed(Vec<String>),
}

/// Compare what the user saw (`seen`, from the mirror row they clicked) with Google's fresh copy, in
/// the mirror's own terms so like is compared with like.
pub fn delete_still_matches(seen: &SeenSummary, fresh: &Value) -> DeleteCheck {
    if is_gone(fresh) {
        return DeleteCheck::Gone;
    }
    // `parse_event` normalises start and end exactly as the mirror stores them; a calendar id is
    // needed for its row id but plays no part in the comparison.
    let Some(now) = crate::calendar::parse_event("", fresh) else {
        // Not a single event PM could mirror (no start, or a series master): don't delete blind.
        return DeleteCheck::Changed(vec!["start".into()]);
    };
    let mut changed = Vec::new();
    if crate::calendar::mirrored_summary(&now.summary) != seen.summary {
        changed.push("summary".to_string());
    }
    if now.start != seen.start || now.all_day != seen.all_day {
        changed.push("start".to_string());
    }
    if now.end != seen.end {
        changed.push("end".to_string());
    }
    let location = now
        .location
        .as_deref()
        .map(crate::calendar::mirrored_location);
    if location.as_deref().unwrap_or("") != seen.location.as_deref().unwrap_or("") {
        changed.push("location".to_string());
    }
    if changed.is_empty() {
        DeleteCheck::Matches
    } else {
        DeleteCheck::Changed(changed)
    }
}

/// Whether the time the user dragged (`seen`) is still Google's (C16a): start, end and kind, in the
/// mirror's own terms. A title or place changed in Google since is no reason to refuse: a move sends
/// only its start and end, on the fresh etag. The answer is the delete's (`Matches`: it may go
/// ahead).
pub fn timing_still_matches(seen: &SeenTimes, fresh: &Value) -> DeleteCheck {
    if is_gone(fresh) {
        return DeleteCheck::Gone;
    }
    let Some(now) = crate::calendar::parse_event("", fresh) else {
        // Not a single event PM could mirror (no start, or a series itself): never moved blind.
        return DeleteCheck::Changed(vec!["start".into()]);
    };
    let mut changed = Vec::new();
    if now.start != seen.start || now.all_day != seen.all_day {
        changed.push("start".to_string());
    }
    if now.end != seen.end {
        changed.push("end".to_string());
    }
    if changed.is_empty() {
        DeleteCheck::Matches
    } else {
        DeleteCheck::Changed(changed)
    }
}

// --- comparing one field the way Google means it ---

/// A text field, with absent read as empty (Google omits empty ones).
fn text_of<'a>(event: &'a Value, key: &str) -> &'a str {
    event.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Absent `transparency` is Google's default, `opaque` (busy).
fn transparency_of(event: &Value) -> &str {
    event
        .get("transparency")
        .and_then(Value::as_str)
        .unwrap_or("opaque")
}

/// Absent `visibility` is Google's default, `default`.
fn visibility_of(event: &Value) -> &str {
    event
        .get("visibility")
        .and_then(Value::as_str)
        .unwrap_or("default")
}

/// A flag Google leaves out when it holds its default: `guestsCanModify` false,
/// `guestsCanInviteOthers` and `guestsCanSeeOtherGuests` true (INFERRED, probe G2).
pub(crate) fn bool_of(v: Option<&Value>, default: bool) -> bool {
    v.and_then(Value::as_bool).unwrap_or(default)
}

/// Whether one patchable field has the same value in `a` and `b`, each a field value as it appears
/// in an event (or in a PATCH body), `None` when absent.
fn field_eq(key: &str, a: Option<&Value>, b: Option<&Value>) -> bool {
    match key {
        "start" | "end" => when_eq(a, b),
        "transparency" => {
            a.and_then(Value::as_str).unwrap_or("opaque")
                == b.and_then(Value::as_str).unwrap_or("opaque")
        }
        "visibility" => {
            a.and_then(Value::as_str).unwrap_or("default")
                == b.and_then(Value::as_str).unwrap_or("default")
        }
        // Who is on it, not each one's reply (R2: someone's RSVP is no conflict).
        "attendees" => attendees::guest_set(a) == attendees::guest_set(b),
        // A flag read as text would be "" either way, so any two would match (C12 §3.3).
        "guestsCanModify" => bool_of(a, false) == bool_of(b, false),
        "guestsCanInviteOthers" | "guestsCanSeeOtherGuests" => bool_of(a, true) == bool_of(b, true),
        // In any order, the defaults however Google writes them (C13): by value, a list handed back
        // reordered would read as not landed after a lost answer, and as a conflict on the next save.
        "reminders" => reminders_eq(a, b),
        // By the call it is (C18): by value, PM's request for a call would never match the call
        // Google made of it, so a save would never read as landed (G4).
        "conferenceData" => call::same_call(a, b),
        // A list or an object (`recurrence`…) by value: read as text, any two would be ""
        // and equal, so a changed one would never be a conflict (G4, found by C8's survey; the guest
        // list and the notifications, the ones PM sends, have their own arms above).
        _ if [a, b]
            .iter()
            .any(|v| v.is_some_and(|v| v.is_array() || v.is_object())) =>
        {
            a.filter(|v| !v.is_null()) == b.filter(|v| !v.is_null())
        }
        _ => a.and_then(Value::as_str).unwrap_or("") == b.and_then(Value::as_str).unwrap_or(""),
    }
}

/// Whether two start (or end) nodes name the same time: the same date for all-day, or the same
/// instant in the same zone for a timed one (offsets may be written differently). A `null` key, as a
/// kind-switching PATCH sends, counts as absent.
pub(crate) fn when_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let key = |node: Option<&Value>, k: &str| -> Option<String> {
        node.and_then(|n| n.get(k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let (a_date, b_date) = (key(a, "date"), key(b, "date"));
    let (a_dt, b_dt) = (key(a, "dateTime"), key(b, "dateTime"));
    match (a_date, b_date, a_dt, b_dt) {
        (Some(x), Some(y), None, None) => x == y,
        (None, None, Some(x), Some(y)) => {
            let instant = |s: &str| chrono::DateTime::parse_from_rfc3339(s.trim()).ok();
            match (instant(&x), instant(&y)) {
                (Some(i), Some(j)) => i == j && key(a, "timeZone") == key(b, "timeZone"),
                _ => x == y,
            }
        }
        (None, None, None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern, RepeatRule};
    use crate::calendar_write::dto::{
        GuestOp, GuestState, MeetChange, NewGuest, ShowAs, TimeDraft, Visibility,
    };
    use serde_json::json;

    fn day(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Google's fresh copy of a one-off event, with fields PM must never touch.
    fn fresh() -> Value {
        json!({
            "id": "ev1",
            "etag": "\"100\"",
            "status": "confirmed",
            "summary": "Dentist",
            "location": "High St",
            "start": { "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London" },
            "end": { "dateTime": "2026-10-12T11:00:00+01:00", "timeZone": "Europe/London" },
            "iCalUID": "ev1@google.com",
            "sequence": 2,
            "eventType": "default",
            "organizer": { "email": "me@example.com", "self": true },
            "creator": { "email": "me@example.com" },
            "attachments": [{ "fileUrl": "https://drive.google.com/x" }],
            "extendedProperties": { "private": { "k": "v" } },
            "colorId": "5",
            "eventLabelId": "label-1"
        })
    }

    fn timed(sd: &str, st: &str, ed: &str, et: &str) -> TimeDraft {
        TimeDraft::Timed {
            start_date: sd.into(),
            start_time: st.into(),
            start_zone: "Europe/London".into(),
            end_date: ed.into(),
            end_time: et.into(),
            end_zone: "Europe/London".into(),
        }
    }

    #[test]
    fn only_touched_fields_that_differ_are_sent() {
        let draft = EventPatchDraft {
            summary: Some("Dentist (moved)".into()),
            // Touched but unchanged: not sent.
            location: Some("High St".into()),
            show_as: Some(ShowAs::Busy),
            visibility: Some(Visibility::Default),
            ..Default::default()
        };
        let body = build_patch(&fresh(), &draft).unwrap();
        assert_eq!(Value::Object(body), json!({ "summary": "Dentist (moved)" }));
        // Nothing touched, or everything already so: an empty body, and nothing is sent.
        assert!(build_patch(&fresh(), &EventPatchDraft::default())
            .unwrap()
            .is_empty());
    }

    /// Google's copy without its label, which holds the colour (R-LABEL).
    fn unlabelled() -> Value {
        let mut event = fresh();
        event.as_object_mut().unwrap().remove("eventLabelId");
        event
    }

    /// Google's fields stay Google's: nothing outside the patchable set is ever in a body, however the
    /// draft is built. C13 flips the colour to sendable on purpose; a label never is. C18 flips the
    /// video call: a request for a new Meet call, under the session's id.
    #[test]
    fn nothing_but_the_patchable_fields_is_ever_sent() {
        let draft = EventPatchDraft {
            summary: Some("x".into()),
            location: Some("".into()),
            description: Some("notes".into()),
            // The gate's to read, never a body key (C19).
            replace_formatting: true,
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-13".into(),
                last_day: "2026-10-13".into(),
            }),
            show_as: Some(ShowAs::Free),
            visibility: Some(Visibility::Private),
            // A repeat is never a one-event body's: the series planners write it (C11).
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
            guests: Some(vec![GuestOp {
                email: "b@x.org".into(),
                to: GuestState::Required,
            }]),
            guests_can_modify: Some(true),
            guests_can_invite_others: Some(false),
            guests_can_see_other_guests: Some(false),
            reminders: Some(own(&[("popup", 10)])),
            colour: Some(ColourDraft::Colour { id: "9".into() }),
            meet: Some(MeetChange::Add),
            meet_request: Some("r1".into()),
            // Never a body key: it goes as a request of its own (C18d).
            calendar: Some("cal-b".into()),
        };
        let body = build_patch(&unlabelled(), &draft).unwrap();
        for key in body.keys() {
            assert!(PATCHABLE.contains(&key.as_str()), "{key}");
        }
        assert_eq!(
            body.len(),
            14,
            "every patchable field, the guests', notifications, colour and call included"
        );
        assert_eq!(body["conferenceData"], call::meet_request_body("r1"));
        for never in [
            "eventType",
            "recurringEventId",
            "originalStartTime",
            "iCalUID",
            "sequence",
            "organizer",
            "creator",
            "attachments",
            "extendedProperties",
            // PM doesn't set Google's labels (C13's Adopted list).
            "eventLabelId",
            "id",
            "etag",
        ] {
            assert!(!body.contains_key(never), "{never}");
        }
        assert_eq!(body["transparency"], "transparent");
        assert_eq!(body["visibility"], "private");
        // Clearing the location sends an empty string.
        assert_eq!(body["location"], "");
    }

    #[test]
    fn start_and_end_always_travel_together() {
        // Only the end moves; both are sent.
        let body = build_patch(
            &fresh(),
            &EventPatchDraft {
                time: Some(timed("2026-10-12", "10:00", "2026-10-12", "11:30")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(body["end"]["dateTime"], "2026-10-12T11:30:00+01:00");
        // The same instants written with another offset are no change.
        let mut utc = fresh();
        utc["start"]["dateTime"] = "2026-10-12T09:00:00Z".into();
        utc["end"]["dateTime"] = "2026-10-12T10:00:00Z".into();
        let same = build_patch(
            &utc,
            &EventPatchDraft {
                time: Some(timed("2026-10-12", "10:00", "2026-10-12", "11:00")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(same.is_empty(), "{same:?}");
    }

    /// INFERRED until L6: a kind switch nulls the key the event no longer uses.
    #[test]
    fn switching_between_timed_and_all_day_drops_the_old_key() {
        let to_all_day = build_patch(
            &fresh(),
            &EventPatchDraft {
                time: Some(TimeDraft::AllDay {
                    first_day: "2026-10-12".into(),
                    last_day: "2026-10-12".into(),
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            to_all_day["start"],
            json!({ "date": "2026-10-12", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            to_all_day["end"],
            json!({ "date": "2026-10-13", "dateTime": null, "timeZone": null })
        );

        let mut all_day = fresh();
        all_day["start"] = json!({ "date": "2026-10-12" });
        all_day["end"] = json!({ "date": "2026-10-13" });
        let to_timed = build_patch(
            &all_day,
            &EventPatchDraft {
                time: Some(timed("2026-10-12", "10:00", "2026-10-12", "11:00")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(to_timed["start"]["date"], Value::Null);
        assert_eq!(to_timed["start"]["timeZone"], "Europe/London");
        // And a body that switched counts as applied once Google holds the new kind.
        let mut landed = all_day.clone();
        landed["start"] =
            json!({ "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London" });
        landed["end"] =
            json!({ "dateTime": "2026-10-12T11:00:00+01:00", "timeZone": "Europe/London" });
        assert!(already_applied(&landed, &to_timed));
    }

    #[test]
    fn a_bad_time_is_refused_before_anything_is_built() {
        let err = build_patch(
            &fresh(),
            &EventPatchDraft {
                summary: Some("x".into()),
                time: Some(timed("2026-03-29", "01:30", "2026-03-29", "03:00")),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::Time(TimeError::Gap { .. })));
    }

    /// An end-only change to an event that starts at the second 01:30 of London's fall-back: the
    /// start goes out exactly as Google holds it, never an hour earlier.
    #[test]
    fn the_untouched_half_is_sent_as_google_holds_it() {
        let mut folded = fresh();
        folded["start"] =
            json!({ "dateTime": "2026-10-25T01:30:00Z", "timeZone": "Europe/London" });
        folded["end"] = json!({ "dateTime": "2026-10-25T02:30:00Z", "timeZone": "Europe/London" });
        let body = build_patch(
            &folded,
            &EventPatchDraft {
                time: Some(timed("2026-10-25", "01:30", "2026-10-25", "03:00")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(body["start"], folded["start"]);
        assert_eq!(body["end"]["dateTime"], "2026-10-25T03:00:00+00:00");
        // And with nothing moved, nothing is sent at all.
        let unchanged = build_patch(
            &folded,
            &EventPatchDraft {
                time: Some(timed("2026-10-25", "01:30", "2026-10-25", "02:30")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(unchanged.is_empty(), "{unchanged:?}");
    }

    /// R2 for an update: a field Google changed that the save also sends is a conflict; one it
    /// doesn't send isn't, and both changes are kept.
    #[test]
    fn an_update_conflicts_only_on_fields_it_sends() {
        let base = fresh();
        let body = build_patch(
            &base,
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let mut location_moved = base.clone();
        location_moved["location"] = "Low St".into();
        location_moved["etag"] = "\"101\"".into();
        assert_eq!(
            update_check(&base, &location_moved, &body),
            UpdateCheck::Proceed
        );

        let mut retitled = base.clone();
        retitled["summary"] = "Dentist (cancelled?)".into();
        assert_eq!(
            update_check(&base, &retitled, &body),
            UpdateCheck::Conflict(vec!["summary".into()])
        );

        // Only the etag moved (a reminder changed): go ahead.
        let mut etag_only = base.clone();
        etag_only["etag"] = "\"102\"".into();
        assert_eq!(update_check(&base, &etag_only, &body), UpdateCheck::Proceed);
    }

    /// C8b review R7: a list or an object is compared by value, a null as absent; never as text,
    /// which would read any two as equal.
    #[test]
    fn a_list_or_an_object_conflicts_by_value() {
        let base = fresh();
        let body: Map<String, Value> = [(
            "reminders".to_string(),
            serde_json::json!({ "useDefault": true }),
        )]
        .into_iter()
        .collect();
        let mut own = base.clone();
        own["reminders"] = serde_json::json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": 5 }] });
        assert_eq!(
            update_check(&base, &own, &body),
            UpdateCheck::Conflict(vec!["reminders".into()])
        );
        let mut nulled = base.clone();
        nulled["reminders"] = Value::Null;
        let mut absent = base.clone();
        if let Some(obj) = absent.as_object_mut() {
            obj.retain(|k, _| k != "reminders");
        }
        assert_eq!(update_check(&nulled, &absent, &body), UpdateCheck::Proceed);
    }

    /// R2: a fresh `status: cancelled` is gone, for a save as for a delete, even though `get` still
    /// returns all its details (so it can be restored) and they match.
    #[test]
    fn an_event_deleted_meanwhile_is_gone_not_saved_onto() {
        let base = fresh();
        let body = build_patch(
            &base,
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let mut deleted = base.clone();
        deleted["status"] = "cancelled".into();
        deleted["etag"] = "\"103\"".into();
        assert_eq!(update_check(&base, &deleted, &body), UpdateCheck::Gone);
        deleted["summary"] = "Dentist (moved)".into();
        assert!(
            !already_applied(&deleted, &body),
            "a deleted copy never counts as landed"
        );
    }

    #[test]
    fn a_landed_save_is_recognised_after_a_lost_answer() {
        let body = build_patch(
            &fresh(),
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                show_as: Some(ShowAs::Free),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!already_applied(&fresh(), &body));
        let mut landed = fresh();
        landed["summary"] = "Dentist (moved)".into();
        landed["transparency"] = "transparent".into();
        assert!(already_applied(&landed, &body));
    }

    /// C18: a save that asked for a call reads as landed on any live Meet call Google holds (a
    /// whole-value compare would never match PM's request against Google's answer, G4), and on its
    /// own request Google failed (that write landed: the failure is said apart); not on another
    /// request's failure, nor on no call. Two copies of Google's are compared by the call they hold.
    #[test]
    fn a_save_reads_an_asked_for_call_as_landed() {
        let body: Map<String, Value> =
            [("conferenceData".to_string(), call::meet_request_body("r1"))]
                .into_iter()
                .collect();
        let ready = with_key(
            &fresh(),
            "conferenceData",
            json!({ "conferenceId": "abc", "conferenceSolution": { "key": { "type": "hangoutsMeet" } } }),
        );
        assert!(already_applied(&ready, &body));
        let failed = |request: &str| {
            with_key(
                &fresh(),
                "conferenceData",
                json!({ "createRequest": { "requestId": request,
                        "conferenceSolutionKey": { "type": "hangoutsMeet" },
                        "status": { "statusCode": "failure" } } }),
            )
        };
        assert!(already_applied(&failed("r1"), &body));
        assert!(!already_applied(&failed("r0"), &body));
        assert!(!already_applied(&fresh(), &body));
        // Someone added a call in Google since the editor opened: a conflict, never a second call.
        assert_eq!(
            update_check(&fresh(), &ready, &body),
            UpdateCheck::Conflict(vec!["conferenceData".into()])
        );
        assert_eq!(update_check(&ready, &ready, &body), UpdateCheck::Proceed);
        // A removal lands where Google holds no call.
        let removing: Map<String, Value> = [("conferenceData".to_string(), Value::Null)]
            .into_iter()
            .collect();
        assert!(already_applied(&fresh(), &removing));
        assert!(!already_applied(&ready, &removing));
        // The draft's request goes into the body under its own id; one Google already holds sends
        // nothing.
        let adding = EventPatchDraft {
            meet: Some(MeetChange::Add),
            meet_request: Some("r1".into()),
            ..Default::default()
        };
        assert_eq!(build_patch(&fresh(), &adding).unwrap(), body);
        assert!(build_patch(&ready, &adding).unwrap().is_empty());
        assert!(
            build_patch(
                &fresh(),
                &EventPatchDraft {
                    meet_request: None,
                    ..adding.clone()
                }
            ) == Err(PatchError::MeetUnready),
            "a call asked for without the session's id sends nothing"
        );
    }

    /// C13 review round 1: a copy holding part of a save is told apart from one holding none of it,
    /// so a save Google applied without keeping all of it never reads "nothing changed". A field sent
    /// as it already was proves nothing either way, and a deleted copy holds nothing.
    #[test]
    fn a_save_held_in_part_is_told_from_one_held_not_at_all() {
        // The dentist with a colour and no label, so "Calendar colour" may go.
        let before = with_key(&fresh(), "eventLabelId", json!(""));
        let mut body = build_patch(
            &before,
            &EventPatchDraft {
                summary: Some("Team lunch".into()),
                colour: Some(ColourDraft::Calendar),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            body.get("colorId") == Some(&Value::Null),
            "the calendar's colour as null"
        );
        // The title landed; the colour Google kept.
        let part = with_key(&before, "summary", json!("Team lunch"));
        assert!(!already_applied(&part, &body));
        assert!(any_applied(&before, &part, &body));
        assert!(!none_applied(&before, &part, &body));
        // Nothing landed: Google kept every field as it was.
        assert!(!any_applied(&before, &before, &body));
        assert!(none_applied(&before, &before, &body));
        // A field Google holds neither as sent nor as it was: not kept, and not landed either.
        let other = with_key(&before, "summary", json!("Lunch (Google)"));
        assert!(!any_applied(&before, &other, &body));
        assert!(!none_applied(&before, &other, &body));
        // Sent as it already was, a field proves nothing landed.
        body.insert("location".into(), json!("High St"));
        assert!(!any_applied(&before, &before, &body));
        // Deleted, a copy holds none of it, whatever its details say.
        let gone = with_key(&part, "status", json!("cancelled"));
        assert!(!any_applied(&before, &gone, &body));
    }

    fn with_key(event: &Value, key: &str, value: Value) -> Value {
        let mut v = event.clone();
        v[key] = value;
        v
    }

    fn seen() -> SeenSummary {
        SeenSummary {
            summary: "Dentist".into(),
            start: "2026-10-12T09:00:00Z".into(),
            end: Some("2026-10-12T10:00:00Z".into()),
            all_day: false,
            location: Some("High St".into()),
        }
    }

    /// R2 for a delete: only the etag moving (a reminder, a colour) doesn't stop it; anything the
    /// user saw changing does.
    #[test]
    fn a_delete_goes_ahead_only_on_what_the_user_saw() {
        let mut etag_only = fresh();
        etag_only["etag"] = "\"200\"".into();
        assert_eq!(
            delete_still_matches(&seen(), &etag_only),
            DeleteCheck::Matches
        );

        let mut moved = fresh();
        moved["start"]["dateTime"] = "2026-10-13T10:00:00+01:00".into();
        moved["end"]["dateTime"] = "2026-10-13T11:00:00+01:00".into();
        assert_eq!(
            delete_still_matches(&seen(), &moved),
            DeleteCheck::Changed(vec!["start".into(), "end".into()])
        );

        let mut renamed = fresh();
        renamed["summary"] = "Dentist\nreschedule".into();
        assert_eq!(
            delete_still_matches(&seen(), &renamed),
            DeleteCheck::Changed(vec!["summary".into()])
        );

        let mut gone = fresh();
        gone["status"] = "cancelled".into();
        assert_eq!(delete_still_matches(&seen(), &gone), DeleteCheck::Gone);
    }

    #[test]
    fn a_delete_compares_in_the_mirrors_own_terms() {
        // The mirror clips a title to 300 characters; the fresh copy's full title still matches.
        let long = "x".repeat(400);
        let mut fresh_long = fresh();
        fresh_long["summary"] = long.clone().into();
        let seen_long = SeenSummary {
            summary: long.chars().take(300).collect(),
            ..seen()
        };
        assert_eq!(
            delete_still_matches(&seen_long, &fresh_long),
            DeleteCheck::Matches
        );
        // An empty location and an absent one are the same.
        let mut no_location = fresh();
        no_location.as_object_mut().unwrap().remove("location");
        let seen_none = SeenSummary {
            location: None,
            ..seen()
        };
        assert_eq!(
            delete_still_matches(&seen_none, &no_location),
            DeleteCheck::Matches
        );
    }

    // --- moving an event, and putting a move back (C16a) ---

    /// What the user saw of `fresh()`'s time, in the mirror's terms.
    fn seen_times() -> SeenTimes {
        SeenTimes {
            start: "2026-10-12T09:00:00Z".into(),
            end: Some("2026-10-12T10:00:00Z".into()),
            all_day: false,
        }
    }

    /// R2 for a drop: only its start, end and kind count. A title or place changed in Google is no
    /// reason to refuse it.
    #[test]
    fn timing_still_matches_compares_only_times() {
        let mut renamed = fresh();
        renamed["summary"] = "Retro".into();
        renamed["location"] = "Low St".into();
        renamed["etag"] = "\"200\"".into();
        assert_eq!(
            timing_still_matches(&seen_times(), &renamed),
            DeleteCheck::Matches
        );
        let mut later = fresh();
        later["start"]["dateTime"] = "2026-10-12T10:30:00+01:00".into();
        assert_eq!(
            timing_still_matches(&seen_times(), &later),
            DeleteCheck::Changed(vec!["start".into()])
        );
        let mut longer = fresh();
        longer["end"]["dateTime"] = "2026-10-12T11:30:00+01:00".into();
        assert_eq!(
            timing_still_matches(&seen_times(), &longer),
            DeleteCheck::Changed(vec!["end".into()])
        );
        // All day on the same date: the kind changed.
        let seen_day = SeenTimes {
            start: "2026-10-12".into(),
            end: Some("2026-10-13".into()),
            all_day: true,
        };
        let mut day = fresh();
        day["start"] = json!({ "date": "2026-10-12" });
        day["end"] = json!({ "date": "2026-10-13" });
        assert_eq!(timing_still_matches(&seen_day, &day), DeleteCheck::Matches);
        let flipped = SeenTimes {
            all_day: false,
            ..seen_day
        };
        assert_eq!(
            timing_still_matches(&flipped, &day),
            DeleteCheck::Changed(vec!["start".into()])
        );
        let mut gone = fresh();
        gone["status"] = "cancelled".into();
        assert_eq!(
            timing_still_matches(&seen_times(), &gone),
            DeleteCheck::Gone
        );
        // A series itself (a rule, no start PM can mirror): never moved blind.
        let mut series = fresh();
        series["recurrence"] = json!(["RRULE:FREQ=WEEKLY"]);
        assert_eq!(
            timing_still_matches(&seen_times(), &series),
            DeleteCheck::Changed(vec!["start".into()])
        );
    }

    /// The fields an Undo puts back are the C6 set, the notifications and the colour (C16d, Q3), and
    /// nothing else `PATCHABLE` gained since: guests, what they may do, and how it repeats.
    #[test]
    fn undoable_is_the_core_fields_notifications_and_colour() {
        assert_eq!(
            UNDOABLE,
            [
                "summary",
                "location",
                "description",
                "start",
                "end",
                "transparency",
                "visibility",
                "reminders",
                "colorId"
            ]
        );
        assert!(UNDOABLE.iter().all(|k| PATCHABLE.contains(k)));
        for later in [
            "attendees",
            "guestsCanModify",
            "guestsCanInviteOthers",
            "guestsCanSeeOtherGuests",
            "recurrence",
        ] {
            assert!(!UNDOABLE.contains(&later), "{later}");
        }
        assert!(TIMING_KEYS.iter().all(|k| UNDOABLE.contains(k)));
        assert_eq!(TIMING_KEYS, ["start", "end"]);
    }

    /// An Undo goes ahead only while Google still holds what the write left: the keys it no longer
    /// holds, compared the way Google means them; a field the write didn't send never counts.
    #[test]
    fn still_holds_names_what_google_changed_since() {
        let after: Map<String, Value> = [
            ("start".to_string(), fresh()["start"].clone()),
            ("end".to_string(), fresh()["end"].clone()),
        ]
        .into_iter()
        .collect();
        let mut retitled = fresh();
        retitled["summary"] = "Retro".into();
        assert!(still_holds(&retitled, &after).is_empty());
        // The same instant written another way still holds.
        let mut rewritten = fresh();
        rewritten["start"]["dateTime"] = "2026-10-12T09:00:00Z".into();
        assert!(still_holds(&rewritten, &after).is_empty());
        let mut moved = fresh();
        moved["end"]["dateTime"] = "2026-10-12T12:00:00+01:00".into();
        assert_eq!(still_holds(&moved, &after), vec!["end".to_string()]);
    }

    /// The Undo's body: Google's own nodes from before, with every key the write added since sent as
    /// null (a zone, the other kind's keys); text that was absent cleared; defaults for busy or free
    /// and visibility; nothing else.
    #[test]
    fn undo_body_puts_googles_exact_nodes_back() {
        let before: Map<String, Value> = [
            (
                "start".to_string(),
                json!({ "dateTime": "2026-10-12T10:00:00+01:00" }),
            ),
            (
                "end".to_string(),
                json!({ "dateTime": "2026-10-12T11:00:00+01:00" }),
            ),
            ("summary".to_string(), Value::Null),
            ("transparency".to_string(), Value::Null),
            ("visibility".to_string(), json!("private")),
        ]
        .into_iter()
        .collect();
        let body = undo_body(&fresh(), &before).unwrap();
        assert_eq!(
            Value::Object(body),
            json!({
                "start": { "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": null },
                "end": { "dateTime": "2026-10-12T11:00:00+01:00", "timeZone": null },
                "summary": "",
                "transparency": "opaque",
                "visibility": "private",
            })
        );
        // Moved to all day, and back: the date goes as null.
        let mut day = fresh();
        day["start"] = json!({ "date": "2026-10-12" });
        day["end"] = json!({ "date": "2026-10-13" });
        let timed: Map<String, Value> = [
            ("start".to_string(), fresh()["start"].clone()),
            ("end".to_string(), fresh()["end"].clone()),
        ]
        .into_iter()
        .collect();
        let body = undo_body(&day, &timed).unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London",
                    "date": null })
        );
        // Nothing PM doesn't put back, and no start that was never there.
        let guests: Map<String, Value> =
            [("attendees".to_string(), json!([]))].into_iter().collect();
        assert!(undo_body(&fresh(), &guests).is_none());
        let nothing: Map<String, Value> =
            [("start".to_string(), Value::Null)].into_iter().collect();
        assert!(undo_body(&fresh(), &nothing).is_none());
    }

    /// C16d, Q3: notifications and the colour go back as a save sends them. The calendar's defaults
    /// where there were none of the event's own (over a list Google holds now, an empty one, since a
    /// PATCH merges the object), a list whole, a colour as it was or `null` for the calendar's; and
    /// notifications PM can't read put nothing back.
    #[test]
    fn undo_body_puts_notifications_and_colour_back() {
        let one = |key: &str, was: Value| -> Map<String, Value> {
            [(key.to_string(), was)].into_iter().collect()
        };
        let mut listed = fresh();
        listed["reminders"] =
            json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": 5 }] });
        listed["colorId"] = json!("5");
        let undo = |was: Map<String, Value>| Value::Object(undo_body(&listed, &was).unwrap());
        assert_eq!(
            undo(one("reminders", Value::Null)),
            json!({ "reminders": { "useDefault": true, "overrides": [] } })
        );
        assert_eq!(
            undo(one("reminders", json!({ "useDefault": true }))),
            json!({ "reminders": { "useDefault": true, "overrides": [] } })
        );
        // Over Google's defaults, the defaults alone.
        assert_eq!(
            Value::Object(undo_body(&fresh(), &one("reminders", Value::Null)).unwrap()),
            json!({ "reminders": { "useDefault": true } })
        );
        let list = json!({ "useDefault": false, "overrides": [
            { "method": "email", "minutes": 60 }, { "method": "popup", "minutes": 10 }
        ] });
        assert_eq!(
            undo(one("reminders", list.clone())),
            json!({ "reminders": list })
        );
        assert_eq!(
            undo(one("colorId", json!("11"))),
            json!({ "colorId": "11" })
        );
        assert_eq!(
            undo(one("colorId", Value::Null)),
            json!({ "colorId": null })
        );
        let unreadable = json!({ "useDefault": false, "overrides": [{ "method": "popup" }] });
        assert!(undo_body(&listed, &one("reminders", unreadable)).is_none());
    }

    // --- creating an event (C10) ---

    fn new_event(time: TimeDraft) -> NewEventDraft {
        NewEventDraft {
            summary: "Dentist".into(),
            time,
            location: "High St".into(),
            description: "Bring the form\nand a pen".into(),
            show_as: Some(ShowAs::Free),
            visibility: Some(Visibility::Private),
            recurrence: None,
            guests: Vec::new(),
            guests_can_modify: None,
            guests_can_invite_others: None,
            guests_can_see_other_guests: None,
            reminders: None,
            colour: None,
            meet: false,
            meet_request: None,
        }
    }

    /// `new_event` repeating by `pattern`, every time, until `ends`.
    fn repeating(time: TimeDraft, pattern: RepeatPattern, ends: RepeatEnds) -> NewEventDraft {
        NewEventDraft {
            recurrence: Some(RepeatRule {
                pattern,
                interval: 1,
                ends,
            }),
            ..new_event(time)
        }
    }

    fn weekly_on(days: &[DayCode]) -> RepeatPattern {
        RepeatPattern::Weekly {
            days: days.to_vec(),
        }
    }

    /// Every field a create may send, the rule, the guests, the notifications and the colour included
    /// (C11, C12d, C13), and nothing else.
    #[test]
    fn insert_body_sends_only_what_a_create_may() {
        let body = insert_body(&NewEventDraft {
            guests: vec![NewGuest {
                email: "b@example.org".into(),
                optional: false,
            }],
            guests_can_modify: Some(true),
            reminders: Some(own(&[("popup", 10)])),
            colour: Some(ColourDraft::Colour { id: "3".into() }),
            meet: true,
            meet_request: Some("r1".into()),
            ..repeating(
                timed("2026-10-26", "10:00", "2026-10-26", "11:00"),
                weekly_on(&[DayCode::Mon]),
                RepeatEnds::After { count: 10 },
            )
        })
        .unwrap();
        let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "attendees",
                "colorId",
                "conferenceData",
                "description",
                "end",
                "guestsCanModify",
                "location",
                "recurrence",
                "reminders",
                "start",
                "summary",
                "transparency",
                "visibility"
            ]
        );
        assert!(keys.iter().all(|k| CREATABLE.contains(k)));
        assert_eq!(body["summary"], "Dentist");
        assert_eq!(body["description"], "Bring the form\nand a pen");
        assert_eq!(body["transparency"], "transparent");
        assert_eq!(body["visibility"], "private");
        assert_eq!(
            body["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"])
        );
        assert!(!body.contains_key("id"), "the insert request writes the id");
        // No rule, no `recurrence` at all; no guests, no `attendees`, Google's own permissions
        // unsaid; no notifications or colour, which the calendar's then give.
        let once = insert_body(&new_event(timed(
            "2026-10-26",
            "10:00",
            "2026-10-26",
            "11:00",
        )))
        .unwrap();
        for absent in [
            "recurrence",
            "attendees",
            "guestsCanModify",
            "guestsCanInviteOthers",
            "guestsCanSeeOtherGuests",
            "reminders",
            "colorId",
            "eventLabelId",
            "conferenceData",
        ] {
            assert!(!once.contains_key(absent), "{absent}");
        }
    }

    /// C18b: a call is asked for exactly when the draft asks, under the request id the create
    /// command put in; asked for without one, nothing is built.
    #[test]
    fn insert_body_asks_for_a_call_only_when_the_draft_does() {
        let plain = new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"));
        let body = insert_body(&NewEventDraft {
            meet_request: Some("r1".into()),
            ..plain.clone()
        })
        .unwrap();
        assert!(
            !body.contains_key("conferenceData"),
            "an id without the ask"
        );
        let asked = NewEventDraft {
            meet: true,
            ..plain
        };
        let body = insert_body(&NewEventDraft {
            meet_request: Some("r1".into()),
            ..asked.clone()
        })
        .unwrap();
        assert_eq!(body["conferenceData"], call::meet_request_body("r1"));
        assert!(
            insert_body(&asked) == Err(InsertError::Draft(PatchError::MeetUnready)),
            "no id, no body"
        );
        assert_eq!(
            InsertError::Draft(PatchError::MeetUnready).to_string(),
            "PM couldn't get the video call ready, so nothing was sent. Try again."
        );
    }

    /// C11 §3.3's tripwire, restated for C12 (critiques C1-H4, C2-M4) and widened by C12d, C13 and
    /// C18 on purpose: a create sends a save's fields before guests, the rule, the guests' four, then
    /// the notifications and colour (never a label), and a call, exactly; so it may send whatever a
    /// save that makes a one-off repeat may.
    #[test]
    fn creatable_is_the_c11_list_guests_notifications_and_colour() {
        assert_eq!(
            CREATABLE,
            [
                "summary",
                "location",
                "description",
                "start",
                "end",
                "transparency",
                "visibility",
                "recurrence",
                "attendees",
                "guestsCanModify",
                "guestsCanInviteOthers",
                "guestsCanSeeOtherGuests",
                "reminders",
                "colorId",
                "conferenceData"
            ]
        );
        let mut creatable = CREATABLE.to_vec();
        let mut repeat = REPEAT_PATCHABLE.to_vec();
        creatable.sort_unstable();
        repeat.sort_unstable();
        assert_eq!(creatable, repeat);
    }

    fn guest(email: &str, optional: bool) -> NewGuest {
        NewGuest {
            email: email.into(),
            optional,
        }
    }

    /// C12d: a new event's guests go as new entries in the order added, `optional` only where it's
    /// so; nothing of a reply, a name or Google's own keys, which only Google writes.
    #[test]
    fn insert_body_sends_guests_as_new_entries() {
        let body = insert_body(&NewEventDraft {
            guests: vec![
                guest("b@example.org", false),
                guest(" a@example.org ", true),
            ],
            ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
        })
        .unwrap();
        assert!(
            body["attendees"]
                == json!([
                    { "email": "b@example.org" },
                    { "email": "a@example.org", "optional": true }
                ]),
            "both guests, trimmed and in order"
        );
    }

    /// An address that isn't one, or one listed twice, builds nothing, in words naming no address.
    #[test]
    fn insert_body_refuses_a_guest_google_would() {
        let with = |guests: Vec<NewGuest>| {
            insert_body(&NewEventDraft {
                guests,
                ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
            })
        };
        let not_one = with(vec![guest("bob at example", false)]);
        assert!(not_one == Err(InsertError::Guest(attendees::GuestError::NotAnAddress)));
        assert_eq!(
            InsertError::Guest(attendees::GuestError::NotAnAddress).to_string(),
            "One of the guests isn't an email address."
        );
        let twice = with(vec![
            guest("b@example.org", false),
            guest("B@Example.org", true),
        ]);
        assert!(twice == Err(InsertError::Guest(attendees::GuestError::Repeated)));
    }

    /// Each guest permission goes only where it isn't Google's default (C12 §3.2): guests may
    /// invite others and see the list, not change the event.
    #[test]
    fn a_creates_guest_permissions_go_only_where_they_arent_googles() {
        let with = |modify, invite, see| {
            insert_body(&NewEventDraft {
                guests_can_modify: modify,
                guests_can_invite_others: invite,
                guests_can_see_other_guests: see,
                ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
            })
            .unwrap()
        };
        let googles = with(Some(false), Some(true), Some(true));
        for key in [
            "guestsCanModify",
            "guestsCanInviteOthers",
            "guestsCanSeeOtherGuests",
        ] {
            assert!(!googles.contains_key(key), "{key}");
        }
        let changed = with(Some(true), Some(false), Some(false));
        assert_eq!(changed["guestsCanModify"], true);
        assert_eq!(changed["guestsCanInviteOthers"], false);
        assert_eq!(changed["guestsCanSeeOtherGuests"], false);
        let one = with(None, Some(false), None);
        assert_eq!(one["guestsCanInviteOthers"], false);
        assert!(
            !one.contains_key("guestsCanModify") && !one.contains_key("guestsCanSeeOtherGuests")
        );
    }

    /// A one-off made to repeat may send whatever a save may, and the rule (C11 §10): guests changed
    /// in the same save go too.
    #[test]
    fn repeat_patchable_is_patchable_and_the_rule() {
        let mut expected: Vec<&str> = PATCHABLE.to_vec();
        expected.push("recurrence");
        assert_eq!(REPEAT_PATCHABLE, expected.as_slice());
    }

    // --- guests (C12) ---

    fn me() -> Value {
        json!({ "email": "me@example.com", "self": true, "organizer": true,
                "responseStatus": "accepted" })
    }

    fn guest_a(reply: &str) -> Value {
        json!({ "email": "A@x.org", "responseStatus": reply, "comment": "late",
                "additionalGuests": 1, "id": "g1" })
    }

    fn with_guests(list: Vec<Value>) -> Value {
        let mut event = fresh();
        event["attendees"] = Value::from(list);
        event
    }

    fn adding(email: &str, to: GuestState) -> EventPatchDraft {
        EventPatchDraft {
            guests: Some(vec![GuestOp {
                email: email.into(),
                to,
            }]),
            ..Default::default()
        }
    }

    /// R1: the list sent is Google's own with the change, every key Google sent kept.
    #[test]
    fn a_guest_change_is_sent_as_googles_list_with_the_change() {
        let event = with_guests(vec![me(), guest_a("accepted")]);
        let add = adding("b@x.org", GuestState::Required);
        let body = build_patch(&event, &add).unwrap();
        assert!(
            Value::Object(body.clone())
                == json!({ "attendees": [me(), guest_a("accepted"), { "email": "b@x.org" }] }),
            "O1 through the save"
        );
        assert!(
            body["attendees"]
                == Value::from(attendees::apply(event.get("attendees"), add.guest_ops())),
            "exactly the ops on Google's list"
        );
        let off = adding("a@x.org", GuestState::Absent);
        assert!(
            Value::Object(build_patch(&event, &off).unwrap()) == json!({ "attendees": [me()] }),
            "O3 through the save"
        );
    }

    /// O7: a change Google already holds sends nothing, as a title typed back to itself doesn't.
    #[test]
    fn a_guest_change_google_already_holds_sends_nothing() {
        let event = with_guests(vec![me(), guest_a("accepted")]);
        for draft in [
            adding("A@X.ORG", GuestState::Required),
            adding("c@x.org", GuestState::Absent),
        ] {
            assert!(build_patch(&event, &draft).unwrap().is_empty());
        }
    }

    /// Each permission is sent only when it differs from Google's, its default read in.
    #[test]
    fn guest_permissions_are_sent_only_when_they_differ() {
        let perm = |modify, invite, see| EventPatchDraft {
            guests_can_modify: modify,
            guests_can_invite_others: invite,
            guests_can_see_other_guests: see,
            ..Default::default()
        };
        assert!(build_patch(&fresh(), &perm(None, Some(true), Some(true)))
            .unwrap()
            .is_empty());
        assert!(
            Value::Object(build_patch(&fresh(), &perm(Some(false), Some(false), None)).unwrap())
                == json!({ "guestsCanInviteOthers": false }),
            "only what differs"
        );
        let mut modifiable = fresh();
        modifiable["guestsCanModify"] = json!(true);
        assert!(build_patch(&modifiable, &perm(Some(true), None, None))
            .unwrap()
            .is_empty());
        assert!(
            Value::Object(build_patch(&fresh(), &perm(Some(true), None, Some(false))).unwrap())
                == json!({ "guestsCanModify": true, "guestsCanSeeOtherGuests": false }),
            "both"
        );
    }

    /// The plan's "rebase after someone else's RSVP": a reply that landed since the editor opened is
    /// no conflict, and the list sent carries it.
    #[test]
    fn someone_elses_reply_is_no_conflict() {
        let base = with_guests(vec![me(), guest_a("needsAction")]);
        let fresh = with_guests(vec![me(), guest_a("accepted")]);
        let body = build_patch(&fresh, &adding("b@x.org", GuestState::Required)).unwrap();
        assert_eq!(update_check(&base, &fresh, &body), UpdateCheck::Proceed);
        assert_eq!(body["attendees"][1]["responseStatus"], "accepted");
    }

    /// R2: a guest added, taken off or made optional in Google since the editor opened is a conflict.
    #[test]
    fn a_guest_added_in_google_meanwhile_is_a_conflict() {
        let base = with_guests(vec![me(), guest_a("accepted")]);
        let c = json!({ "email": "c@x.org" });
        let mut optional = guest_a("accepted");
        optional["optional"] = json!(true);
        for fresh in [
            with_guests(vec![me(), guest_a("accepted"), c]),
            with_guests(vec![me()]),
            with_guests(vec![me(), optional]),
        ] {
            let body = build_patch(&fresh, &adding("b@x.org", GuestState::Required)).unwrap();
            assert_eq!(
                update_check(&base, &fresh, &body),
                UpdateCheck::Conflict(vec!["attendees".into()])
            );
        }
    }

    /// After a lost answer, Google's copy holds the guests sent when it has the same people, whatever
    /// keys Google added to them.
    #[test]
    fn a_landed_guest_change_reads_as_applied() {
        let body = build_patch(
            &with_guests(vec![me(), guest_a("accepted")]),
            &adding("b@x.org", GuestState::Required),
        )
        .unwrap();
        let landed = with_guests(vec![
            me(),
            guest_a("declined"),
            json!({ "email": "b@x.org", "responseStatus": "needsAction", "id": "g2" }),
        ]);
        assert!(already_applied(&landed, &body));
        let without = with_guests(vec![me(), guest_a("accepted")]);
        assert!(!already_applied(&without, &body));
    }

    /// C12 §3.3: a flag read as text was "" either way, so any two compared equal; they compare as
    /// Google means them, its defaults filled in.
    #[test]
    fn booleans_compare_as_google_means_them() {
        let (on, off) = (json!(true), json!(false));
        assert!(!field_eq("guestsCanModify", Some(&on), None));
        assert!(field_eq("guestsCanModify", Some(&off), None));
        assert!(field_eq("guestsCanInviteOthers", Some(&on), None));
        assert!(!field_eq("guestsCanInviteOthers", Some(&off), None));
        assert!(!field_eq("guestsCanSeeOtherGuests", Some(&off), None));
        assert!(field_eq("guestsCanSeeOtherGuests", Some(&on), Some(&on)));
        // So a lost save of a flag isn't read as landed when Google doesn't hold it.
        let body: Map<String, Value> = [("guestsCanModify".to_string(), on.clone())]
            .into_iter()
            .collect();
        assert!(!already_applied(&fresh(), &body));
    }

    /// G6: a weekly event on its own weekday sends one rule line, and both nodes name the zone.
    #[test]
    fn a_repeating_insert_sends_one_rule_and_names_its_zone() {
        let body = insert_body(&repeating(
            timed("2026-10-13", "09:00", "2026-10-13", "10:00"),
            weekly_on(&[DayCode::Tue]),
            RepeatEnds::Never,
        ))
        .unwrap();
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]));
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-13T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(body["end"]["timeZone"], "Europe/London");
    }

    /// A27 (live test L-R10's case): Monday and Wednesday chosen on a Tuesday starts on the
    /// Wednesday, and the hour's length goes with it.
    #[test]
    fn a_repeating_insert_starts_on_the_first_day_its_rule_makes() {
        let body = insert_body(&repeating(
            timed("2026-10-13", "09:00", "2026-10-13", "10:00"),
            weekly_on(&[DayCode::Wed, DayCode::Mon]),
            RepeatEnds::OnDate {
                date: "2026-12-31".into(),
            },
        ))
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-14T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            body["end"],
            json!({ "dateTime": "2026-10-14T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        // The end date's last second in London, as UTC (GMT by then).
        assert_eq!(
            body["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261231T235959Z"])
        );
    }

    /// An all-day repeating event names days only (probe P-R1): no zone on either node.
    #[test]
    fn an_all_day_repeating_insert_sends_dates_only() {
        let body = insert_body(&repeating(
            TimeDraft::AllDay {
                first_day: "2026-10-26".into(),
                last_day: "2026-10-26".into(),
            },
            RepeatPattern::Daily,
            RepeatEnds::Never,
        ))
        .unwrap();
        assert_eq!(body["start"], json!({ "date": "2026-10-26" }));
        assert_eq!(body["end"], json!({ "date": "2026-10-27" }));
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=DAILY"]));
    }

    /// London skips 01:00-02:00 on Sunday 29-03-2026. A Sunday rule chosen at 01:30 on the Saturday
    /// would start in it; an end the move puts in it is refused as any end in a gap is.
    #[test]
    fn a_repeating_insert_in_a_gap_is_refused() {
        let gap = insert_body(&repeating(
            timed("2026-03-28", "01:30", "2026-03-28", "03:00"),
            weekly_on(&[DayCode::Sun]),
            RepeatEnds::Never,
        ));
        assert!(gap == Err(InsertError::Rule(SpecError::Gap(day(2026, 3, 29)))));
        assert_eq!(
            gap.unwrap_err().to_string(),
            "The first occurrence would be on 29-03-2026, when the clocks skip that time. Pick a \
             time outside that hour."
        );
        let end_gap = insert_body(&repeating(
            timed("2026-03-27", "23:00", "2026-03-28", "01:30"),
            weekly_on(&[DayCode::Sat]),
            RepeatEnds::Never,
        ));
        assert!(matches!(
            end_gap,
            Err(InsertError::Time(TimeError::Gap { .. }))
        ));
    }

    /// Google allows 730 occurrences (Help 37115): 731 builds nothing to send.
    #[test]
    fn a_rule_past_730_builds_nothing() {
        let draft = |count: u16| {
            repeating(
                timed("2026-10-26", "10:00", "2026-10-26", "11:00"),
                RepeatPattern::Daily,
                RepeatEnds::After { count },
            )
        };
        assert!(insert_body(&draft(731)) == Err(InsertError::Rule(SpecError::TooMany)));
        assert_eq!(
            insert_body(&draft(730)).unwrap()["recurrence"],
            json!(["RRULE:FREQ=DAILY;COUNT=730"])
        );
        // An end date PM can't read, or a time it can't, is said in its own words.
        let unread = NewEventDraft {
            recurrence: Some(RepeatRule {
                pattern: RepeatPattern::Daily,
                interval: 1,
                ends: RepeatEnds::OnDate {
                    date: "31-12-2026".into(),
                },
            }),
            ..draft(1)
        };
        assert!(insert_body(&unread) == Err(InsertError::Rule(SpecError::BadDate)));
        let bad_time = repeating(
            timed("2026-10-26", "late", "2026-10-26", "11:00"),
            RepeatPattern::Daily,
            RepeatEnds::Never,
        );
        assert!(
            insert_body(&bad_time) == Err(InsertError::Time(TimeError::BadTime("late".into())))
        );
    }

    fn weekly_rule(days: &[DayCode]) -> RepeatRule {
        RepeatRule {
            pattern: RepeatPattern::Weekly {
                days: days.to_vec(),
            },
            interval: 1,
            ends: RepeatEnds::Never,
        }
    }

    /// C11 §10: a one-off given a repeat, its time left alone, starts on the first day the rule
    /// makes at its own wall time and length, both nodes naming their zone even where Google's own
    /// had none; it repeats from there.
    #[test]
    fn a_repeat_given_to_a_one_off_starts_on_the_rules_first_day_with_its_zone() {
        let mut bare = fresh();
        bare["start"] = json!({ "dateTime": "2026-10-12T10:00:00+01:00" });
        bare["end"] = json!({ "dateTime": "2026-10-12T11:30:00+01:00" });
        let mut body = build_patch(&bare, &EventPatchDraft::default()).unwrap();
        let start = add_repeat(
            &mut body,
            &bare,
            None,
            &weekly_rule(&[DayCode::Wed]),
            Some("Europe/London"),
        )
        .unwrap();
        assert_eq!(start.date(), day(2026, 10, 14));
        assert_eq!(
            Value::Object(body),
            json!({
                "start": { "dateTime": "2026-10-14T10:00:00+01:00", "timeZone": "Europe/London" },
                "end": { "dateTime": "2026-10-14T11:30:00+01:00", "timeZone": "Europe/London" },
                "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=WE"],
            })
        );
        // A node with no zone, and no zone to read it in, is never read as UTC (R6), and says so of
        // the event, not this computer (C11d review 1).
        let mut body = Map::new();
        let refused = add_repeat(&mut body, &bare, None, &weekly_rule(&[DayCode::Wed]), None);
        assert!(refused == Err(InsertError::NoEventZone));
        assert!(body.is_empty(), "nothing built");
        assert_eq!(
            InsertError::NoEventZone.to_string(),
            "PM can't tell which time zone this event is in, so it can't make it repeat."
        );
    }

    /// C11d review 1: an all-day one-off given a repeat and a time clears its dates, as any save's
    /// switch does.
    #[test]
    fn a_repeat_given_with_a_switch_to_timed_clears_the_other_kind() {
        let mut all_day = fresh();
        all_day["start"] = json!({ "date": "2026-10-12" });
        all_day["end"] = json!({ "date": "2026-10-13" });
        let at_ten = timed("2026-10-12", "10:00", "2026-10-12", "11:00");
        let draft = EventPatchDraft {
            time: Some(at_ten.clone()),
            ..Default::default()
        };
        let mut body = build_patch(&all_day, &draft).unwrap();
        add_repeat(
            &mut body,
            &all_day,
            Some(&at_ten),
            &weekly_rule(&[DayCode::Mon]),
            Some("Europe/London"),
        )
        .unwrap();
        assert_eq!(
            body["start"],
            json!({
                "dateTime": "2026-10-12T10:00:00+01:00",
                "timeZone": "Europe/London",
                "date": null
            })
        );
        assert_eq!(
            body["end"],
            json!({
                "dateTime": "2026-10-12T11:00:00+01:00",
                "timeZone": "Europe/London",
                "date": null
            })
        );
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"]));
    }

    /// The draft's own time wins, and a switch to all day clears the timed keys, as any save's does.
    #[test]
    fn a_repeat_given_with_a_switch_to_all_day_clears_the_other_kind() {
        let all_day = TimeDraft::AllDay {
            first_day: "2026-10-12".into(),
            last_day: "2026-10-12".into(),
        };
        let draft = EventPatchDraft {
            time: Some(all_day.clone()),
            ..Default::default()
        };
        let mut body = build_patch(&fresh(), &draft).unwrap();
        add_repeat(
            &mut body,
            &fresh(),
            Some(&all_day),
            &weekly_rule(&[DayCode::Mon]),
            Some("Europe/London"),
        )
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "date": "2026-10-12", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            body["end"],
            json!({ "date": "2026-10-13", "dateTime": null, "timeZone": null })
        );
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"]));
        // A rule outside the limits builds nothing.
        let mut body = Map::new();
        let too_many = RepeatRule {
            ends: RepeatEnds::After { count: 731 },
            ..weekly_rule(&[DayCode::Mon])
        };
        assert!(
            add_repeat(&mut body, &fresh(), None, &too_many, Some("Europe/London"))
                == Err(InsertError::Rule(SpecError::TooMany))
        );
        assert!(body.is_empty());
    }

    /// Blank text is left out, so Google stores no empty strings; text with words in is sent as
    /// typed, spaces and all.
    #[test]
    fn an_empty_title_and_empty_text_are_left_out() {
        let draft = NewEventDraft {
            summary: "   ".into(),
            location: String::new(),
            description: "\n ".into(),
            ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
        };
        let body = insert_body(&draft).unwrap();
        for key in ["summary", "location", "description"] {
            assert!(!body.contains_key(key), "{key}");
        }
        let spaced = NewEventDraft {
            summary: " Dentist ".into(),
            ..draft
        };
        assert_eq!(insert_body(&spaced).unwrap()["summary"], " Dentist ");
    }

    /// A flight: each half carries its own zone and offset.
    #[test]
    fn a_timed_insert_names_its_zones() {
        let body = insert_body(&new_event(TimeDraft::Timed {
            start_date: "2026-10-26".into(),
            start_time: "10:00".into(),
            start_zone: "Europe/London".into(),
            end_date: "2026-10-26".into(),
            end_time: "13:00".into(),
            end_zone: "America/New_York".into(),
        }))
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-26T10:00:00+00:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            body["end"],
            json!({ "dateTime": "2026-10-26T13:00:00-04:00", "timeZone": "America/New_York" })
        );
    }

    #[test]
    fn an_all_day_insert_ends_the_day_after() {
        let body = insert_body(&new_event(TimeDraft::AllDay {
            first_day: "2026-10-26".into(),
            last_day: "2026-10-26".into(),
        }))
        .unwrap();
        assert_eq!(body["start"], json!({ "date": "2026-10-26" }));
        assert_eq!(body["end"], json!({ "date": "2026-10-27" }));
    }

    #[test]
    fn a_gap_or_an_end_before_start_builds_nothing() {
        let gap = insert_body(&new_event(timed(
            "2026-03-29",
            "01:30",
            "2026-03-29",
            "03:00",
        )));
        assert!(matches!(gap, Err(InsertError::Time(TimeError::Gap { .. }))));
        for end in ["10:00", "09:00"] {
            let backwards =
                insert_body(&new_event(timed("2026-10-26", "10:00", "2026-10-26", end)));
            assert!(
                matches!(backwards, Err(InsertError::Time(TimeError::EndBeforeStart))),
                "{end}"
            );
        }
    }

    #[test]
    fn default_visibility_and_no_show_as_are_left_out() {
        for visibility in [None, Some(Visibility::Default)] {
            let body = insert_body(&NewEventDraft {
                show_as: None,
                visibility,
                ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
            })
            .unwrap();
            assert!(!body.contains_key("transparency"));
            assert!(!body.contains_key("visibility"), "{visibility:?}");
        }
    }

    // --- notifications and colour (C13) ---

    fn reminder(method: &str, minutes: i64) -> Reminder {
        Reminder {
            method: method.into(),
            minutes,
        }
    }

    /// The event's own list, in this order.
    fn own(list: &[(&str, i64)]) -> RemindersDraft {
        RemindersDraft {
            use_default: false,
            overrides: list.iter().map(|(m, n)| reminder(m, *n)).collect(),
        }
    }

    fn defaults() -> RemindersDraft {
        RemindersDraft {
            use_default: true,
            overrides: Vec::new(),
        }
    }

    /// Google's copy holding `list` as its own notifications.
    fn holding(list: &[(&str, i64)]) -> Value {
        let mut event = unlabelled();
        event["reminders"] = reminders_value(&own(list));
        event
    }

    fn notifying(reminders: RemindersDraft) -> EventPatchDraft {
        EventPatchDraft {
            reminders: Some(reminders),
            ..Default::default()
        }
    }

    fn colouring(colour: ColourDraft) -> EventPatchDraft {
        EventPatchDraft {
            colour: Some(colour),
            ..Default::default()
        }
    }

    /// One of the eleven goes as `colorId`, alone, as text.
    #[test]
    fn a_colour_sends_color_id_and_never_a_label() {
        let body = build_patch(
            &unlabelled(),
            &colouring(ColourDraft::Colour { id: "11".into() }),
        )
        .unwrap();
        assert!(Value::Object(body) == serde_json::json!({ "colorId": "11" }));
    }

    /// "Calendar colour" clears a colour of its own with `null` (INFERRED, probe E15b) and sends
    /// nothing when there's none.
    #[test]
    fn calendar_colour_clears_the_colour_and_nothing_else() {
        let body = build_patch(&unlabelled(), &colouring(ColourDraft::Calendar)).unwrap();
        assert!(Value::Object(body) == serde_json::json!({ "colorId": null }));
        let mut plain = unlabelled();
        plain.as_object_mut().unwrap().remove("colorId");
        assert!(build_patch(&plain, &colouring(ColourDraft::Calendar))
            .unwrap()
            .is_empty());
    }

    /// R-LABEL: a label sets the colour Google draws, and PM doesn't change labels, so any colour
    /// onto a labelled copy is refused in words and builds nothing (C1-H1's case, an "all events"
    /// save onto a labelled series, reaches here).
    #[test]
    fn a_colour_on_a_labelled_event_is_refused_with_words() {
        for colour in [
            ColourDraft::Calendar,
            ColourDraft::Colour { id: "5".into() },
        ] {
            let refused = build_patch(&fresh(), &colouring(colour));
            assert!(refused == Err(PatchError::Colour(ColourError::Labelled)));
        }
        assert_eq!(
            PatchError::Colour(ColourError::Labelled).to_string(),
            "This event has a Google Calendar label, which sets its colour. Change it in Google \
             Calendar."
        );
        // Its notifications still go.
        let body = build_patch(&fresh(), &notifying(own(&[("popup", 10)]))).unwrap();
        assert!(body.contains_key("reminders") && !body.contains_key("colorId"));
    }

    /// Only "1" to "11" as Google writes them: nothing else reaches a body.
    #[test]
    fn an_unknown_colour_builds_nothing() {
        for id in ["0", "12", "05", "", "+5", " 5", "1.0", "eleven"] {
            assert!(!is_event_colour(id), "{id}");
            let refused = build_patch(
                &unlabelled(),
                &colouring(ColourDraft::Colour { id: id.into() }),
            );
            assert!(
                refused == Err(PatchError::Colour(ColourError::UnknownColour)),
                "{id}"
            );
        }
        for n in 1..=11 {
            assert!(is_event_colour(&n.to_string()), "{n}");
        }
    }

    #[test]
    fn the_same_colour_sends_nothing() {
        // `fresh` holds "5".
        assert!(build_patch(
            &unlabelled(),
            &colouring(ColourDraft::Colour { id: "5".into() })
        )
        .unwrap()
        .is_empty());
    }

    /// An event's own colour, and its label, stay as Google holds them: a PATCH that leaves a field
    /// out keeps it (events.patch), so a title save never clears either (live test L-C1).
    #[test]
    fn a_title_save_never_touches_colour_or_label() {
        let body = build_patch(
            &fresh(),
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!body.contains_key("colorId") && !body.contains_key("eventLabelId"));
        assert!(!body.contains_key("reminders"));
    }

    /// Sent whole, in the draft's order, and only when they differ in more than order.
    #[test]
    fn notifications_are_sent_whole_and_only_when_they_differ() {
        let held = holding(&[("popup", 10)]);
        let body = build_patch(&held, &notifying(own(&[("popup", 10), ("email", 60)]))).unwrap();
        assert!(
            Value::Object(body)
                == serde_json::json!({ "reminders": { "useDefault": false, "overrides": [
                    { "method": "popup", "minutes": 10 }, { "method": "email", "minutes": 60 }
                ] } }),
            "the whole list"
        );
        let both = holding(&[("email", 60), ("popup", 10)]);
        assert!(
            build_patch(&both, &notifying(own(&[("popup", 10), ("email", 60)])))
                .unwrap()
                .is_empty(),
            "the same two in the other order"
        );
        // Every notification removed: an empty list of the event's own.
        let none = build_patch(&held, &notifying(own(&[]))).unwrap();
        assert!(
            none["reminders"] == serde_json::json!({ "useDefault": false, "overrides": [] }),
            "none at all"
        );
    }

    /// The calendar's defaults: `useDefault` and, over a list Google holds, an empty one, since PATCH
    /// merges an object (INFERRED, probe E9h); nothing when they're already the defaults.
    #[test]
    fn the_defaults_send_use_default_and_empty_googles_list() {
        let body = build_patch(&holding(&[("popup", 10)]), &notifying(defaults())).unwrap();
        assert!(
            body["reminders"] == serde_json::json!({ "useDefault": true, "overrides": [] }),
            "Google's list emptied"
        );
        let silent = build_patch(&holding(&[]), &notifying(defaults())).unwrap();
        assert!(silent["reminders"] == serde_json::json!({ "useDefault": true }));
        for already in [unlabelled(), {
            let mut d = unlabelled();
            d["reminders"] = serde_json::json!({ "useDefault": true });
            d
        }] {
            assert!(build_patch(&already, &notifying(defaults()))
                .unwrap()
                .is_empty());
        }
        // And the empty list read back is the defaults landed.
        let landed = {
            let mut d = unlabelled();
            d["reminders"] = serde_json::json!({ "useDefault": true });
            d
        };
        assert!(already_applied(&landed, &body));
    }

    #[test]
    fn defaults_with_a_list_of_their_own_are_refused() {
        let both = RemindersDraft {
            use_default: true,
            overrides: vec![reminder("popup", 10)],
        };
        assert!(check_reminders(&both, &[]) == Err(ReminderError::DefaultWithOwn));
        assert!(
            build_patch(&unlabelled(), &notifying(both))
                == Err(PatchError::Reminders(ReminderError::DefaultWithOwn))
        );
    }

    #[test]
    fn a_sixth_notification_is_refused() {
        let five = own(&[
            ("popup", 0),
            ("popup", 10),
            ("popup", 30),
            ("email", 60),
            ("email", 1440),
        ]);
        assert!(check_reminders(&five, &[]).is_ok());
        let mut six = five.clone();
        six.overrides.push(reminder("popup", 5));
        assert!(check_reminders(&six, &[]) == Err(ReminderError::TooMany));
    }

    /// A new one outside 0 to four weeks is refused; one Google holds there goes back as it is.
    #[test]
    fn a_new_notification_outside_googles_range_is_refused_and_one_google_holds_is_kept() {
        for minutes in [40_321, -1] {
            assert!(
                check_reminders(&own(&[("popup", minutes)]), &[]) == Err(ReminderError::OutOfRange),
                "{minutes}"
            );
        }
        assert!(check_reminders(&own(&[("email", 40_320), ("popup", 0)]), &[]).is_ok());
        let held = [reminder("popup", -540)];
        assert!(check_reminders(&own(&[("popup", -540), ("popup", 10)]), &held).is_ok());
        let body = build_patch(
            &holding(&[("popup", -540)]),
            &notifying(own(&[("popup", -540), ("popup", 10)])),
        )
        .unwrap();
        assert_eq!(body["reminders"]["overrides"][0]["minutes"], -540);
    }

    #[test]
    fn an_old_method_google_holds_is_kept_and_a_new_one_is_refused() {
        let held = [reminder("sms", 30)];
        assert!(check_reminders(&own(&[("sms", 30), ("email", 15)]), &held).is_ok());
        assert!(
            check_reminders(&own(&[("sms", 30), ("sms", 15)]), &held)
                == Err(ReminderError::UnknownMethod)
        );
    }

    /// C1-L7: a new one the same as another on the list is refused; two Google holds go back.
    #[test]
    fn a_notification_on_the_list_twice_is_refused_unless_google_holds_both() {
        let twice = own(&[("popup", 10), ("popup", 10)]);
        assert!(check_reminders(&twice, &[]) == Err(ReminderError::Duplicate));
        assert!(check_reminders(&twice, &[reminder("popup", 10)]) == Err(ReminderError::Duplicate));
        let held = [reminder("popup", 10), reminder("popup", 10)];
        assert!(check_reminders(&twice, &held).is_ok());
        // The same time a different way is two notifications.
        assert!(check_reminders(&own(&[("popup", 10), ("email", 10)]), &[]).is_ok());
    }

    /// C1-M6: a list PM can't read whole reads as `None`, and a new list never goes onto it.
    #[test]
    fn unreadable_notifications_read_as_none_and_take_no_new_list() {
        let mut no_minutes = unlabelled();
        no_minutes["reminders"] =
            serde_json::json!({ "useDefault": false, "overrides": [{ "method": "popup" }] });
        let mut fraction = unlabelled();
        fraction["reminders"] = serde_json::json!({ "useDefault": false, "overrides":
            [{ "method": "popup", "minutes": 7.5 }] });
        let mut odd = unlabelled();
        odd["reminders"] = serde_json::json!("soon");
        for unreadable in [&no_minutes, &fraction, &odd] {
            assert!(reminders_of(unreadable).is_none());
            assert!(
                build_patch(unreadable, &notifying(own(&[("popup", 10)])))
                    == Err(PatchError::Reminders(ReminderError::Unreadable))
            );
        }
        // No `reminders` at all is the defaults; a list without `useDefault` isn't.
        assert!(reminders_of(&unlabelled()) == Some(defaults()));
        let mut bare = unlabelled();
        bare["reminders"] =
            serde_json::json!({ "overrides": [{ "method": "email", "minutes": 5 }] });
        assert!(reminders_of(&bare) == Some(own(&[("email", 5)])));
    }

    /// Everywhere a save compares them: in any order, an empty list left out alike, a changed
    /// minute a change.
    #[test]
    fn notifications_compare_as_a_set_not_a_list() {
        let ab = reminders_value(&own(&[("popup", 10), ("email", 60)]));
        let ba = reminders_value(&own(&[("email", 60), ("popup", 10)]));
        let moved = reminders_value(&own(&[("email", 60), ("popup", 15)]));
        assert!(field_eq("reminders", Some(&ab), Some(&ba)));
        assert!(!field_eq("reminders", Some(&ab), Some(&moved)));
        let empty = serde_json::json!({ "useDefault": false, "overrides": [] });
        let left_out = serde_json::json!({ "useDefault": false });
        assert!(reminders_eq(Some(&empty), Some(&left_out)));
        assert!(
            !reminders_eq(Some(&empty), None),
            "none of its own isn't the defaults"
        );
        // A reordered answer after a lost one reads as landed, and a changed one doesn't.
        let body: Map<String, Value> = [("reminders".to_string(), ab.clone())]
            .into_iter()
            .collect();
        let mut reordered = unlabelled();
        reordered["reminders"] = ba.clone();
        assert!(already_applied(&reordered, &body));
        let mut changed = unlabelled();
        changed["reminders"] = moved.clone();
        assert!(!already_applied(&changed, &body));
        // Google reordering since the editor opened is no conflict; a changed minute is.
        let mut base = unlabelled();
        base["reminders"] = ab;
        assert_eq!(update_check(&base, &reordered, &body), UpdateCheck::Proceed);
        assert_eq!(
            update_check(&base, &changed, &body),
            UpdateCheck::Conflict(vec!["reminders".into()])
        );
    }

    /// A colour saved while someone gave the event a label in Google would vanish under it: a
    /// conflict on the label, though the save never sends one.
    #[test]
    fn a_colour_write_conflicts_on_a_label_added_meanwhile() {
        let base = unlabelled();
        let body = build_patch(&base, &colouring(ColourDraft::Colour { id: "2".into() })).unwrap();
        assert_eq!(watched(&body), ["colorId", "eventLabelId"]);
        assert_eq!(
            update_check(&base, &fresh(), &body),
            UpdateCheck::Conflict(vec!["eventLabelId".into()])
        );
        // A title save doesn't watch it.
        let title = build_patch(
            &base,
            &EventPatchDraft {
                summary: Some("x".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(update_check(&base, &fresh(), &title), UpdateCheck::Proceed);
    }

    /// C2-L6: onto the series, from an editor that showed an occurrence, a notification either holds
    /// goes back as it is; one neither holds is checked as new.
    #[test]
    fn a_series_save_keeps_a_notification_the_occurrence_holds() {
        let master = holding(&[("popup", 10)]);
        let occurrence = holding(&[("sms", 30)]);
        let draft = notifying(own(&[("sms", 30), ("popup", 10)]));
        assert!(
            build_patch(&master, &draft)
                == Err(PatchError::Reminders(ReminderError::UnknownMethod)),
            "new to the series alone"
        );
        let body = series_patch(&master, &occurrence, &draft).unwrap();
        assert_eq!(body["reminders"]["overrides"][0]["method"], "sms");
        assert!(
            series_patch(&master, &occurrence, &notifying(own(&[("sms", 15)])))
                == Err(PatchError::Reminders(ReminderError::UnknownMethod))
        );
        // Held by both counts once: a second copy is new, and on the list twice.
        let both = holding(&[("popup", 10)]);
        assert!(
            series_patch(
                &master,
                &both,
                &notifying(own(&[("popup", 10), ("popup", 10)]))
            ) == Err(PatchError::Reminders(ReminderError::Duplicate))
        );
    }

    /// A new event's own list goes whole; the defaults, or none chosen, send nothing (Google gives
    /// the calendar's, INFERRED E9); a list Google wouldn't take builds nothing.
    #[test]
    fn a_create_sends_notifications_only_when_not_the_defaults() {
        let with = |reminders| {
            insert_body(&NewEventDraft {
                reminders,
                ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
            })
        };
        let body = with(Some(own(&[("email", 30)]))).unwrap();
        assert!(
            body["reminders"]
                == serde_json::json!({ "useDefault": false, "overrides": [
                    { "method": "email", "minutes": 30 }
                ] })
        );
        let none = with(Some(own(&[]))).unwrap();
        assert!(none["reminders"] == serde_json::json!({ "useDefault": false, "overrides": [] }));
        for quiet in [None, Some(defaults())] {
            assert!(!with(quiet).unwrap().contains_key("reminders"));
        }
        assert!(
            with(Some(own(&[("sms", 30)])))
                == Err(InsertError::Draft(PatchError::Reminders(
                    ReminderError::UnknownMethod
                )))
        );
        assert!(
            with(Some(RemindersDraft {
                use_default: true,
                overrides: vec![reminder("popup", 5)],
            })) == Err(InsertError::Draft(PatchError::Reminders(
                ReminderError::DefaultWithOwn
            )))
        );
    }

    /// One of the eleven goes as `colorId`; the calendar's own, or none, sends nothing; anything
    /// else builds nothing.
    #[test]
    fn a_create_with_a_colour_sends_one_key() {
        let with = |colour| {
            insert_body(&NewEventDraft {
                colour,
                ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
            })
        };
        let body = with(Some(ColourDraft::Colour { id: "7".into() })).unwrap();
        assert_eq!(body["colorId"], "7");
        assert!(!body.contains_key("eventLabelId"));
        for none in [None, Some(ColourDraft::Calendar)] {
            assert!(!with(none).unwrap().contains_key("colorId"));
        }
        assert!(
            with(Some(ColourDraft::Colour { id: "12".into() }))
                == Err(InsertError::Draft(PatchError::Colour(
                    ColourError::UnknownColour
                )))
        );
    }

    /// Every refusal has plain words: "notifications", never "reminders".
    #[test]
    fn every_draft_refusal_has_words() {
        for (e, words) in [
            (
                PatchError::Reminders(ReminderError::TooMany),
                "Google allows up to 5 notifications on an event.",
            ),
            (
                PatchError::Reminders(ReminderError::DefaultWithOwn),
                "An event uses its calendar's default notifications or its own, not both.",
            ),
            (
                PatchError::Reminders(ReminderError::OutOfRange),
                "Google allows notifications up to 4 weeks before the event.",
            ),
            (
                PatchError::Reminders(ReminderError::UnknownMethod),
                "PM can only add notifications and emails.",
            ),
            (
                PatchError::Reminders(ReminderError::Duplicate),
                "The same notification is on the list twice. Remove one.",
            ),
            (
                PatchError::Reminders(ReminderError::Unreadable),
                "PM can't read one of this event's notifications, so it can't change them. \
                 Nothing was saved.",
            ),
            (
                PatchError::Colour(ColourError::UnknownColour),
                "That isn't one of Google Calendar's colours, so nothing was saved.",
            ),
        ] {
            assert_eq!(e.to_string(), words);
            assert_eq!(InsertError::Draft(e).to_string(), words);
            assert!(!words.to_lowercase().contains("reminder"));
        }
    }
}
