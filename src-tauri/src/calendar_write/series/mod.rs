// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Changing a repeating Google event (#884, C8): "this event", "this and following" and "all
//! events", each worked out as an ordered list of writes before any is sent, with what each write's
//! answer has to show and how to undo it. Pure, like the rest of the core: C9 fetches, sends and
//! lands; nothing here can reach Google.
//!
//! - [`scope`]: which of the three choices a change can offer.
//! - [`plan`]: the writes themselves, and the rules for reading their answers.
//! - [`diff`]: a re-fetch of a series turned into mirror rows added and removed.
//! - [`run`]: a plan sent step by step, its answers read by the rules in [`plan`] (C9a), with no I/O.
//!
//! Everything starts from fresh copies of the event and its series ([`FreshEvent`]), never from the
//! mirror (rule R1), and a change made in Google since the user opened the event is a conflict,
//! never overwritten (R2). The design and its critique are in `docs/calendar-c8-design-2026-10-16.md`.
//!
//! Google calls the series itself the "master" (it holds the `recurrence` rule) and each date it
//! makes an "instance", known for good by its `originalStartTime` (documented as the identity that
//! survives a move); instance ids are opaque and are never read for meaning.
//!
//! CodeQL guardrail (DECISIONS, 2026-10-10): nothing here uses `Vec::remove`, `insert` or
//! `swap_remove` on fetched data, or puts anything from a Google reply in a log or panic message.

pub mod diff;
pub mod plan;
pub mod run;
pub mod scope;

use std::fmt;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use chrono_tz::Tz;
use serde_json::Value;

use crate::calendar_recur::expand::{crate_agrees, is_occurrence};
use crate::calendar_recur::lines::{self, Kind, TextOnly};
use crate::calendar_recur::remap::RetimeError;
use crate::calendar_recur::{Point, RecurError, SeriesStart};

use super::dto::ReadOnlyReason;
use super::time::TimeError;

/// A copy of an event as Google just returned it. Planners take only this, never a mirror row: the
/// mirror clips text on purpose and holds no series facts (R1). Made by the command layer from a
/// GET's reply and nowhere else.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshEvent(Value);

impl FreshEvent {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn from_google(event: Value) -> Self {
        FreshEvent(event)
    }

    pub fn json(&self) -> &Value {
        &self.0
    }
}

/// Every stored exception of a series (occurrences changed or cancelled on their own), as Google
/// lists them by iCal UID, and nothing else: never an `instances` listing, which holds every
/// occurrence and would have a cut cancel each one. `complete` only when the listing ran to its end:
/// planners that move or cut a series refuse an incomplete one, since an exception they can't see
/// would be lost or brought back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exceptions {
    pub items: Vec<Value>,
    pub complete: bool,
}

/// `events.instances` of a series with cancelled ones (`showDeleted`), unwindowed, every page: the
/// occurrences Google counts, for rebasing a COUNT (A7).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub items: Vec<Value>,
    pub complete: bool,
}

/// How long each occurrence lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    /// An exact length (RFC 5545: a series keeps its DTSTART-to-DTEND duration).
    Timed(TimeDelta),
    /// Whole days.
    AllDay(i64),
}

/// A series' master, read from a fresh copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Master {
    pub id: String,
    pub etag: String,
    pub recurrence: Vec<String>,
    /// Its start in the zone it repeats in (A27: Google's `dateTime` read as an instant and put into
    /// `start.timeZone`, never its digits taken as wall time).
    pub start: SeriesStart,
    /// The first occurrence exactly: a start in the second pass of a repeated hour is that one, not
    /// the first (AS15).
    pub first: Point,
    pub length: Length,
    /// The zone its end is written in (a flight can end in another).
    pub end_zone: Tz,
    pub organizer_self: bool,
    pub event_type: Option<String>,
    pub locked: bool,
    /// Anyone besides the user on its guest list.
    pub guests: bool,
}

impl Master {
    pub fn read(fresh: &FreshEvent) -> Result<Master, SeriesRefusal> {
        let it = fresh.json();
        let text = |key: &str| it.get(key).and_then(Value::as_str).map(str::to_string);
        let id = text("id")
            .filter(|id| super::plan::is_plain_segment(id))
            .ok_or(SeriesRefusal::Unreadable)?;
        let etag = text("etag").ok_or(SeriesRefusal::Unreadable)?;
        let recurrence: Vec<String> = it
            .get("recurrence")
            .and_then(Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .filter(|lines: &Vec<String>| !lines.is_empty())
            .ok_or(SeriesRefusal::Unreadable)?;
        let (start, first, length, end_zone) = match (it.get("start"), it.get("end")) {
            (Some(s), Some(e)) if s.get("date").is_some() => {
                let first = date_of(s).ok_or(SeriesRefusal::Unreadable)?;
                let last = date_of(e).ok_or(SeriesRefusal::Unreadable)?;
                (
                    SeriesStart::AllDay(first),
                    Point::Day(first),
                    Length::AllDay((last - first).num_days().max(1)),
                    Tz::UTC,
                )
            }
            (Some(s), Some(e)) => {
                // Google requires a zone on a repeating event, and PM never guesses one (R6).
                let zone = zone_of(s).ok_or(SeriesRefusal::NoZone)?;
                let at = instant_of(s).ok_or(SeriesRefusal::Unreadable)?;
                let until = instant_of(e).ok_or(SeriesRefusal::Unreadable)?;
                (
                    SeriesStart::Timed {
                        local: at.with_timezone(&zone).naive_local(),
                        zone,
                    },
                    Point::At(at),
                    Length::Timed(until - at),
                    zone_of(e).unwrap_or(zone),
                )
            }
            _ => return Err(SeriesRefusal::Unreadable),
        };
        Ok(Master {
            id,
            etag,
            recurrence,
            start,
            first,
            length,
            end_zone,
            organizer_self: it
                .get("organizer")
                .and_then(|o| o.get("self"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            event_type: text("eventType"),
            locked: it.get("locked").and_then(Value::as_bool).unwrap_or(false),
            guests: !attendee_set(it.get("attendees")).is_empty(),
        })
    }

    /// Why nothing about the series may change, from its own facts. The calendar's role and the
    /// account's editing switch are the command layer's (gate.rs); a master can also be an invitation
    /// or a special kind of event, which the gate reads from the occurrence. Its guests are the
    /// planners' to weigh ([`plan::SeriesInput::guests_allowed`]).
    pub fn read_only(&self) -> Option<ReadOnlyReason> {
        if !matches!(self.event_type.as_deref(), None | Some("default")) {
            return Some(ReadOnlyReason::SpecialType);
        }
        if !self.organizer_self {
            return Some(ReadOnlyReason::NotOrganizer);
        }
        if self.locked {
            return Some(ReadOnlyReason::Locked);
        }
        None
    }
}

fn instant_of(node: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(node.get("dateTime")?.as_str()?.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn date_of(node: &Value) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(node.get("date")?.as_str()?, "%Y-%m-%d").ok()
}

fn zone_of(node: &Value) -> Option<Tz> {
    node.get("timeZone")?.as_str()?.parse().ok()
}

/// A start, end or `originalStartTime` node as a point in a series: an instant, or a day.
pub fn point_of(node: Option<&Value>) -> Option<Point> {
    let node = node?;
    if let Some(day) = date_of(node) {
        return Some(Point::Day(day));
    }
    instant_of(node).map(Point::At)
}

/// Where an instance sits in its series for good: its `originalStartTime`.
pub fn original_of(instance: &Value) -> Option<Point> {
    point_of(instance.get("originalStartTime"))
}

/// What one instance is next to its series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceKind {
    /// As the series makes it.
    Plain,
    /// Cancelled on its own (Google keeps only its id, series and original start for sure).
    Cancelled,
    /// Changed on its own. `carry`: fields PM can put on the same occurrence of a new series;
    /// `other`: fields it can't (guests, attachments, a video call, a label…).
    Modified {
        carry: Vec<&'static str>,
        other: Vec<&'static str>,
    },
}

/// The fields of an occurrence PM can carry onto another series, "time" standing for start and end.
pub const CARRY: &[&str] = &[
    "summary",
    "description",
    "location",
    "colorId",
    "transparency",
    "reminders",
    "time",
];

/// Fields an occurrence can change on its own that PM can't carry yet. Visibility among them: Google
/// spreads an occurrence's stricter visibility to its whole series and ignores a looser one, so it
/// can't be put on one occurrence of a new series (C8b review G8).
const OTHER: &[&str] = &[
    "visibility",
    "attendees",
    "attachments",
    "conferenceData",
    "extendedProperties",
    "eventLabelId",
    "guestsCanModify",
    "guestsCanInviteOthers",
    "guestsCanSeeOtherGuests",
];

/// Whether two values of `key` say the same, Google's defaults filled in. A carried field an
/// instance doesn't have reads as its default (an occurrence whose own description was cleared has
/// none), and so does its guest list (Google leaves out an empty one); any other `OTHER` field it
/// doesn't have reads as the master's (so a series with attachments doesn't make every occurrence
/// look changed). INFERRED, for C9a's probe: instances list every field their series has.
fn same_field(key: &str, instance: Option<&Value>, master: Option<&Value>) -> bool {
    let default = |v: Option<&Value>, d: Value| v.filter(|v| !v.is_null()).cloned().unwrap_or(d);
    match key {
        "transparency" => default(instance, "opaque".into()) == default(master, "opaque".into()),
        "visibility" => default(instance, "default".into()) == default(master, "default".into()),
        "reminders" => {
            let d = serde_json::json!({ "useDefault": true });
            default(instance, d.clone()) == default(master, d)
        }
        "summary" | "description" | "location" | "colorId" | "eventLabelId" => {
            default(instance, "".into()) == default(master, "".into())
        }
        "attendees" => attendee_set(instance) == attendee_set(master),
        _ if instance.is_none() => true,
        "attachments" => {
            let urls = |v: Option<&Value>| {
                let mut u: Vec<String> = v
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.get("fileUrl").and_then(Value::as_str).map(str::to_string))
                    .collect();
                u.sort_unstable();
                u
            };
            urls(instance) == urls(master)
        }
        "conferenceData" => {
            let id = |v: Option<&Value>| v.and_then(|c| c.get("conferenceId")).cloned();
            id(instance) == id(master)
        }
        "extendedProperties" => without_pm_notes(instance) == without_pm_notes(master),
        _ => instance == master,
    }
}

/// The private notes PM itself puts on a series: which series a new one was split off and by what
/// plan, and the fence a "Remove the new series" writes (C9). Bookkeeping, not a change anyone made:
/// an occurrence holding a copy from before one was written isn't changed on its own.
pub const PM_NOTES: &[&str] = &["pmSplitOf", "pmSplitPlan", "pmFence"];

/// `extendedProperties` without [`PM_NOTES`], an emptied map read as none.
fn without_pm_notes(v: Option<&Value>) -> Value {
    let mut out = serde_json::Map::new();
    for (scope, props) in v.and_then(Value::as_object).into_iter().flatten() {
        let kept: serde_json::Map<String, Value> = props
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| scope != "private" || !PM_NOTES.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !kept.is_empty() {
            out.insert(scope.clone(), Value::Object(kept));
        }
    }
    Value::Object(out)
}

/// Guests as (address, optional), ignoring each one's reply (an RSVP isn't the organiser's change)
/// and the user's own entry.
pub(super) fn attendee_set(v: Option<&Value>) -> Vec<(String, bool)> {
    let mut set: Vec<(String, bool)> = v
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|a| !a.get("self").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|a| {
            let email = a.get("email")?.as_str()?.to_ascii_lowercase();
            Some((
                email,
                a.get("optional").and_then(Value::as_bool).unwrap_or(false),
            ))
        })
        .collect();
    set.sort_unstable();
    set
}

/// What `instance` is next to its `master` (T13). Its time is compared as instants (dates for an
/// all-day series), never as JSON: Google writes both in the calendar's zone, and an
/// `originalStartTime` may have no `timeZone` (AS8).
pub fn classify_instance(master_json: &Value, master: &Master, instance: &Value) -> InstanceKind {
    if instance.get("status").and_then(Value::as_str) == Some("cancelled") {
        return InstanceKind::Cancelled;
    }
    let mut carry: Vec<&'static str> = CARRY
        .iter()
        .copied()
        .filter(|k| *k != "time" && !same_field(k, instance.get(*k), master_json.get(*k)))
        .collect();
    if !same_time(master, instance) {
        carry.push("time");
    }
    let mut other: Vec<&'static str> = OTHER
        .iter()
        .copied()
        .filter(|k| !same_field(k, instance.get(*k), master_json.get(*k)))
        .collect();
    // A guest list Google cut short can't be compared, so it's never read as the series'.
    if instance.get("attendeesOmitted").and_then(Value::as_bool) == Some(true)
        && !other.contains(&"attendees")
    {
        other.push("attendees");
    }
    if carry.is_empty() && other.is_empty() {
        InstanceKind::Plain
    } else {
        InstanceKind::Modified { carry, other }
    }
}

/// Whether the instance starts where its series puts it and lasts as long.
fn same_time(master: &Master, instance: &Value) -> bool {
    let (Some(original), Some(start), Some(end)) = (
        original_of(instance),
        point_of(instance.get("start")),
        point_of(instance.get("end")),
    ) else {
        return false;
    };
    if start != original {
        return false;
    }
    match (master.length, start, end) {
        (Length::AllDay(days), Point::Day(s), Point::Day(e)) => (e - s).num_days() == days,
        (Length::Timed(exact), Point::At(s), Point::At(e)) => {
            // Google may keep either the exact length or the wall-clock one across a clock change
            // (unprobed): both read as the series' own.
            let zone = master.start.zone();
            let wall = |t: DateTime<Utc>| t.with_timezone(&zone).naive_local();
            let master_wall = match master.first {
                Point::At(first) => wall(first + exact) - wall(first),
                Point::Day(_) => exact,
            };
            e - s == exact || wall(e) - wall(s) == master_wall
        }
        _ => false,
    }
}

/// How many of the series' occurrences come before `at`, as Google counts them (A7): every listed
/// instance whose original start is before it, cancelled ones included and extra dates (RDATEs the
/// rule doesn't make) left out, together with any EXDATE'd rule occurrence Google doesn't list.
/// The crate must agree, or PM won't rebase a COUNT on the number.
pub fn google_before(master: &Master, listing: &Listing, at: Point) -> Result<u32, RecurError> {
    if !listing.complete {
        return Err(RecurError::CannotCount);
    }
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    let zone = master.start.zone();
    let key = |p: &Point| match p {
        Point::At(t) => t.timestamp().to_string(),
        Point::Day(d) => d.to_string(),
    };
    let mut seen = std::collections::BTreeSet::new();
    for item in &listing.items {
        let Some(p) = original_of(item) else {
            return Err(RecurError::CannotCount);
        };
        if p.cmp_in(&at, zone).is_lt() && is_occurrence(&facts, &master.start, p)? {
            seen.insert(key(&p));
        }
    }
    for line in &parsed {
        if let Kind::ExDate(dates) = &line.kind {
            for value in &dates.values {
                let p = dates
                    .point(value, &master.start)
                    .ok_or(RecurError::CannotCount)?;
                if p.cmp_in(&at, zone).is_lt() && is_occurrence(&facts, &master.start, p)? {
                    seen.insert(key(&p));
                }
            }
        }
    }
    let k = u32::try_from(seen.len()).map_err(|_| RecurError::CannotCount)?;
    if !crate_agrees(&facts, &master.start, at, k) {
        return Err(RecurError::CannotCount);
    }
    Ok(k)
}

/// Why PM won't plan a change to a series. Shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesRefusal {
    Recur(RecurError),
    Retime(RetimeError),
    Time(TimeError),
    /// Google's copy of the series lacks something every series has.
    Unreadable,
    /// A timed series with no time zone.
    NoZone,
    /// The occurrence doesn't belong to this series.
    NotInSeries,
    /// PM couldn't see all of the series' exceptions.
    Incomplete,
    /// PM doesn't have the series as it was when the editor opened, to check nobody changed it.
    NoBase,
    /// An occurrence after the split has guests, attachments, a call, a label or a visibility of its
    /// own.
    CannotCarry(NaiveDate),
    /// Its visibility applies to every occurrence (Google), so it can't change for one.
    VisibilityIsSeriesWide,
    /// The series already holds this; only this occurrence differs.
    OnlyThisDiffers,
    /// This occurrence has a time of its own (moved or resized) and the draft doesn't move the
    /// series' start, so saving the series would leave this occurrence's time as it is.
    OnlyThisTimeDiffers,
    /// On this day the series' time falls in the hour the clocks skip, so the rest of it can't
    /// start there.
    SplitInGap(NaiveDate),
    /// A whole series moved into another zone.
    ZoneChange,
}

impl fmt::Display for SeriesRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeriesRefusal::Recur(e) => e.fmt(f),
            SeriesRefusal::Retime(e) => e.fmt(f),
            SeriesRefusal::Time(e) => e.fmt(f),
            SeriesRefusal::Unreadable => {
                write!(f, "PM couldn't read this series' details from Google.")
            }
            SeriesRefusal::NoZone => write!(
                f,
                "Google's copy of this series has no time zone, so PM won't guess one."
            ),
            SeriesRefusal::NotInSeries => {
                write!(
                    f,
                    "This occurrence doesn't belong to the series PM fetched."
                )
            }
            SeriesRefusal::Incomplete => write!(
                f,
                "PM couldn't see every occurrence of this series that was changed on its own."
            ),
            SeriesRefusal::NoBase => write!(
                f,
                "PM no longer has the series as it was when you opened it. Open it again."
            ),
            SeriesRefusal::CannotCarry(day) => write!(
                f,
                "The occurrence on {} has its own guests, attachments, video call, label or who can \
                 see it, which PM can't move to the new series.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::VisibilityIsSeriesWide => write!(
                f,
                "Who can see an event applies to every occurrence of a repeating event, so it \
                 can't change for this one only."
            ),
            SeriesRefusal::OnlyThisDiffers => write!(
                f,
                "The series already has that; only this occurrence is different. Choose \"This \
                 event\" to change it."
            ),
            SeriesRefusal::OnlyThisTimeDiffers => write!(
                f,
                "This occurrence has a time of its own, so changing the whole series here wouldn't \
                 change it. Choose \"This event\" to change it, and change the rest of the series on \
                 its own."
            ),
            SeriesRefusal::SplitInGap(day) => write!(
                f,
                "On {} the series' time falls in the hour the clocks skip, so PM can't start the \
                 rest of the series there. Change this occurrence on its own.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::ZoneChange => write!(
                f,
                "PM can't move a whole series into another time zone. Change this occurrence on \
                 its own."
            ),
        }
    }
}

impl From<RecurError> for SeriesRefusal {
    fn from(e: RecurError) -> Self {
        SeriesRefusal::Recur(e)
    }
}

impl From<RetimeError> for SeriesRefusal {
    fn from(e: RetimeError) -> Self {
        SeriesRefusal::Retime(e)
    }
}

impl From<TimeError> for SeriesRefusal {
    fn from(e: TimeError) -> Self {
        SeriesRefusal::Time(e)
    }
}

/// Whether the series' rule is one PM can only show as text.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9, editing repeating events"
    )
)]
pub fn text_only(master: &Master) -> Option<TextOnly> {
    lines::series_rule(&lines::parse(&master.recurrence)).err()
}

/// Google-shaped series and occurrences for the series tests: a weekly 09:00–10:00 "Standup" in
/// London from Monday 05-10-2026, as Google's API returns them (times in the calendar's zone, here
/// also London).
#[cfg(test)]
pub(crate) mod fixtures {
    use super::plan::{
        plan_series_delete, plan_series_edit, Fresh, NotPlanned, Opened, SeriesInput, SeriesPlan,
    };
    use super::scope::Scope;
    use super::*;
    use crate::calendar_write::dto::{EventPatchDraft, Notify, SeenSummary, TimeDraft};
    use serde_json::json;

    pub const CALENDAR: &str = "me@example.com";

    pub fn master_with(recurrence: &[&str], start: &str, end: &str) -> Value {
        json!({
            "kind": "calendar#event",
            "id": "abc",
            "etag": "\"m1\"",
            "status": "confirmed",
            "iCalUID": "abc@google.com",
            "summary": "Standup",
            "description": "Daily notes",
            "updated": "2026-10-01T10:00:00.000Z",
            "start": { "dateTime": start, "timeZone": "Europe/London" },
            "end": { "dateTime": end, "timeZone": "Europe/London" },
            "recurrence": recurrence,
            "organizer": { "email": "me@example.com", "self": true },
            "eventType": "default",
            "reminders": { "useDefault": true },
        })
    }

    pub fn master(recurrence: &[&str]) -> Value {
        master_with(
            recurrence,
            "2026-10-05T09:00:00+01:00",
            "2026-10-05T10:00:00+01:00",
        )
    }

    /// The occurrence whose original start is `original` (RFC 3339), now at `start`–`end`.
    pub fn moved(original: &str, start: &str, end: &str) -> Value {
        let at = DateTime::parse_from_rfc3339(original)
            .unwrap()
            .with_timezone(&Utc);
        json!({
            "kind": "calendar#event",
            "id": format!("abc_{}", at.format("%Y%m%dT%H%M%SZ")),
            "etag": format!("\"i{}\"", at.timestamp()),
            "status": "confirmed",
            "summary": "Standup",
            "description": "Daily notes",
            "updated": "2026-10-01T10:00:00.000Z",
            "recurringEventId": "abc",
            "originalStartTime": { "dateTime": original, "timeZone": "Europe/London" },
            "start": { "dateTime": start, "timeZone": "Europe/London" },
            "end": { "dateTime": end, "timeZone": "Europe/London" },
            "organizer": { "email": "me@example.com", "self": true },
            "reminders": { "useDefault": true },
        })
    }

    /// The occurrence at `start` (RFC 3339), as the series makes it: an hour long.
    pub fn plain(start: &str) -> Value {
        let at = DateTime::parse_from_rfc3339(start).unwrap();
        let end = (at + TimeDelta::hours(1)).to_rfc3339();
        moved(start, start, &end)
    }

    pub fn cancelled(original: &str) -> Value {
        let at = DateTime::parse_from_rfc3339(original)
            .unwrap()
            .with_timezone(&Utc);
        json!({
            "kind": "calendar#event",
            "id": format!("abc_{}", at.format("%Y%m%dT%H%M%SZ")),
            "etag": format!("\"c{}\"", at.timestamp()),
            "status": "cancelled",
            "recurringEventId": "abc",
            "originalStartTime": { "dateTime": original, "timeZone": "Europe/London" },
        })
    }

    pub fn with(mut event: Value, key: &str, value: Value) -> Value {
        event[key] = value;
        event
    }

    pub fn fresh(v: Value) -> FreshEvent {
        FreshEvent::from_google(v)
    }

    pub fn complete(items: Vec<Value>) -> Exceptions {
        Exceptions {
            items,
            complete: true,
        }
    }

    pub fn point(s: &str) -> Point {
        Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    /// The new series' id in every test plan.
    pub const TAIL: &str = "0123456789abcdef0123456789abcdef";

    pub fn weekly() -> Value {
        master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"])
    }

    pub fn monday_19() -> Value {
        plain("2026-10-19T09:00:00+01:00")
    }

    pub fn title(s: &str) -> EventPatchDraft {
        EventPatchDraft {
            summary: Some(s.into()),
            ..Default::default()
        }
    }

    pub fn at(time: &str, date: &str) -> EventPatchDraft {
        let end = format!("{:02}:00", time[..2].parse::<u32>().unwrap() + 1);
        EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: date.into(),
                start_time: time.into(),
                start_zone: "Europe/London".into(),
                end_date: date.into(),
                end_time: end,
                end_zone: "Europe/London".into(),
            }),
            ..Default::default()
        }
    }

    /// One series as C9 hands it over: the series, the occurrence picked, and its exceptions.
    pub struct Case {
        pub master: FreshEvent,
        pub instance: FreshEvent,
        pub opened_instance: FreshEvent,
        pub opened_master: FreshEvent,
        pub exceptions: Exceptions,
        pub instances: Option<Listing>,
        pub sibling: bool,
        pub guests_allowed: bool,
    }

    impl Case {
        pub fn new(master: Value, instance: Value) -> Case {
            Case {
                master: fresh(master.clone()),
                instance: fresh(instance.clone()),
                opened_instance: fresh(instance),
                opened_master: fresh(master),
                exceptions: complete(Vec::new()),
                instances: None,
                sibling: false,
                guests_allowed: false,
            }
        }

        pub fn exceptions(mut self, items: Vec<Value>) -> Case {
            self.exceptions = complete(items);
            self
        }

        /// As C12 will run it, a series with guests allowed.
        pub fn guests(mut self) -> Case {
            self.guests_allowed = true;
            self
        }

        pub fn input(&self) -> SeriesInput<'_> {
            SeriesInput {
                calendar: CALENDAR,
                opened: Opened {
                    instance: &self.opened_instance,
                    master: Some(&self.opened_master),
                },
                fresh: Fresh {
                    instance: &self.instance,
                    master: &self.master,
                },
                exceptions: &self.exceptions,
                instances: self.instances.as_ref(),
                sibling: self.sibling,
                guests_allowed: self.guests_allowed,
            }
        }

        pub fn delete(&self, scope: Scope) -> Result<SeriesPlan, NotPlanned> {
            plan_series_delete(scope, &self.input(), &seen_of(&self.instance), Notify::None)
        }

        pub fn edit(&self, scope: Scope, draft: EventPatchDraft) -> Result<SeriesPlan, NotPlanned> {
            plan_series_edit(scope, &self.input(), &draft, TAIL, Notify::None)
        }
    }

    /// What the user saw of an occurrence: its mirror row's summary and times.
    pub fn seen_of(instance: &FreshEvent) -> SeenSummary {
        let row = crate::calendar::parse_event("", instance.json()).unwrap();
        SeenSummary {
            summary: row.summary,
            start: row.start,
            end: row.end,
            all_day: row.all_day,
            location: row.location,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use chrono_tz::Europe::London;
    use serde_json::json;

    fn read(v: Value) -> Master {
        Master::read(&fresh(v)).unwrap()
    }

    /// A27: a master's start is Google's instant put into its own zone, whatever offset Google wrote
    /// it with (replies come in the calendar's zone).
    #[test]
    fn a_master_reads_its_start_in_its_own_zone() {
        let m = read(master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            // 09:00 London written as New York's 04:00.
            "2026-10-05T04:00:00-04:00",
            "2026-10-05T05:00:00-04:00",
        ));
        assert_eq!(
            m.start,
            SeriesStart::Timed {
                local: NaiveDate::from_ymd_opt(2026, 10, 5)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
                zone: London,
            }
        );
        assert_eq!(m.length, Length::Timed(TimeDelta::hours(1)));
        assert_eq!(m.read_only(), None);
        // AS15: the second 01:30 of 25-10-2026 is kept as that instant, not the first.
        let second = read(master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=SU"],
            "2026-10-25T01:30:00+00:00",
            "2026-10-25T02:30:00+00:00",
        ));
        assert_eq!(
            second.first,
            Point::At(
                DateTime::parse_from_rfc3339("2026-10-25T01:30:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        let all_day = read(json!({
            "id": "abc", "etag": "\"m1\"", "recurrence": ["RRULE:FREQ=YEARLY"],
            "start": { "date": "2026-03-01" }, "end": { "date": "2026-03-02" },
            "organizer": { "self": true },
        }));
        assert_eq!(
            all_day.start,
            SeriesStart::AllDay(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap())
        );
        assert_eq!(all_day.length, Length::AllDay(1));
    }

    #[test]
    fn a_rule_pm_only_shows_as_text_says_so() {
        assert_eq!(
            text_only(&read(master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]))),
            None
        );
        assert_eq!(
            text_only(&read(master(&["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"]))),
            Some(TextOnly::Part("BYSETPOS".into()))
        );
    }

    #[test]
    fn a_master_pm_cant_read_or_change_says_why() {
        let no_zone = json!({
            "id": "abc", "etag": "\"m1\"", "recurrence": ["RRULE:FREQ=DAILY"],
            "start": { "dateTime": "2026-10-05T09:00:00+01:00" },
            "end": { "dateTime": "2026-10-05T10:00:00+01:00" },
        });
        assert_eq!(Master::read(&fresh(no_zone)), Err(SeriesRefusal::NoZone));
        let no_rule = with(master(&["RRULE:FREQ=DAILY"]), "recurrence", json!([]));
        assert_eq!(
            Master::read(&fresh(no_rule)),
            Err(SeriesRefusal::Unreadable)
        );
        let sneaky = with(master(&["RRULE:FREQ=DAILY"]), "id", json!(".."));
        assert_eq!(Master::read(&fresh(sneaky)), Err(SeriesRefusal::Unreadable));
        let theirs = read(with(
            master(&["RRULE:FREQ=DAILY"]),
            "organizer",
            json!({ "email": "boss@example.com" }),
        ));
        assert_eq!(theirs.read_only(), Some(ReadOnlyReason::NotOrganizer));
        let focus = read(with(
            master(&["RRULE:FREQ=DAILY"]),
            "eventType",
            json!("focusTime"),
        ));
        assert_eq!(focus.read_only(), Some(ReadOnlyReason::SpecialType));
        let locked = read(with(master(&["RRULE:FREQ=DAILY"]), "locked", json!(true)));
        assert_eq!(locked.read_only(), Some(ReadOnlyReason::Locked));
    }

    /// T13.
    #[test]
    fn an_instance_is_plain_cancelled_or_changed_and_how() {
        let mj = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        let m = read(mj.clone());
        let kind = |i: Value| classify_instance(&mj, &m, &i);
        assert_eq!(
            kind(plain("2026-10-12T09:00:00+01:00")),
            InstanceKind::Plain
        );
        // The same occurrence written in another zone's offset is still where its series puts it.
        assert_eq!(
            kind(moved(
                "2026-10-12T09:00:00+01:00",
                "2026-10-12T04:00:00-04:00",
                "2026-10-12T05:00:00-04:00"
            )),
            InstanceKind::Plain
        );
        assert_eq!(
            kind(cancelled("2026-10-19T09:00:00+01:00")),
            InstanceKind::Cancelled
        );
        assert_eq!(
            kind(moved(
                "2026-10-19T09:00:00+01:00",
                "2026-10-20T09:00:00+01:00",
                "2026-10-20T10:00:00+01:00"
            )),
            InstanceKind::Modified {
                carry: vec!["time"],
                other: vec![]
            }
        );
        // Longer than the series: its time is its own too.
        assert_eq!(
            kind(moved(
                "2026-10-19T09:00:00+01:00",
                "2026-10-19T09:00:00+01:00",
                "2026-10-19T11:00:00+01:00"
            )),
            InstanceKind::Modified {
                carry: vec!["time"],
                other: vec![]
            }
        );
        assert_eq!(
            kind(with(
                plain("2026-10-26T09:00:00Z"),
                "summary",
                json!("Standup (room 4)")
            )),
            InstanceKind::Modified {
                carry: vec!["summary"],
                other: vec![]
            }
        );
        // A description cleared on its own counts; a guest or an attachment of its own is "other".
        let cleared = plain("2026-10-26T09:00:00Z");
        let mut cleared = cleared;
        cleared.as_object_mut().unwrap().remove("description");
        assert_eq!(
            kind(cleared),
            InstanceKind::Modified {
                carry: vec!["description"],
                other: vec![]
            }
        );
        assert_eq!(
            kind(with(
                plain("2026-10-26T09:00:00Z"),
                "attachments",
                json!([{ "fileUrl": "https://drive.google.com/x" }])
            )),
            InstanceKind::Modified {
                carry: vec![],
                other: vec!["attachments"]
            }
        );
        // A guest's own reply isn't a change made to the occurrence.
        let guests = json!([{ "email": "a@example.com", "responseStatus": "needsAction" }]);
        let mj2 = with(mj.clone(), "attendees", guests);
        let replied = with(
            plain("2026-10-26T09:00:00Z"),
            "attendees",
            json!([{ "email": "A@example.com", "responseStatus": "accepted" }]),
        );
        assert_eq!(classify_instance(&mj2, &m, &replied), InstanceKind::Plain);
        // A field the series has and the occurrence doesn't list isn't a change of the occurrence's.
        let mj3 = with(mj, "conferenceData", json!({ "conferenceId": "x" }));
        assert_eq!(
            classify_instance(&mj3, &m, &plain("2026-10-26T09:00:00Z")),
            InstanceKind::Plain
        );
    }

    /// C9 design review M9: a fence or split note PM put on the series since an occurrence was
    /// stored doesn't make that occurrence read as changed; a property of anyone else's does.
    #[test]
    fn pms_own_notes_never_make_an_occurrence_look_changed() {
        let props = |private: Value| json!({ "private": private, "shared": { "app": "x" } });
        let mj = with(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]),
            "extendedProperties",
            props(json!({ "other": "1", "pmFence": "f", "pmSplitOf": "a", "pmSplitPlan": "p" })),
        );
        let m = read(mj.clone());
        let older = with(
            plain("2026-10-26T09:00:00Z"),
            "extendedProperties",
            props(json!({ "other": "1" })),
        );
        assert_eq!(classify_instance(&mj, &m, &older), InstanceKind::Plain);
        // Only PM's notes in the private map: emptied, it reads as no private map at all.
        let bare = with(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]),
            "extendedProperties",
            json!({ "private": { "pmFence": "f" } }),
        );
        let bare_instance = with(
            plain("2026-10-26T09:00:00Z"),
            "extendedProperties",
            json!({}),
        );
        assert_eq!(
            classify_instance(&bare, &read(bare.clone()), &bare_instance),
            InstanceKind::Plain
        );
        let theirs = with(
            plain("2026-10-26T09:00:00Z"),
            "extendedProperties",
            props(json!({ "other": "2" })),
        );
        assert_eq!(
            classify_instance(&mj, &m, &theirs),
            InstanceKind::Modified {
                carry: vec![],
                other: vec!["extendedProperties"]
            }
        );
        // A shared property named like a note is someone else's.
        let shared = with(
            plain("2026-10-26T09:00:00Z"),
            "extendedProperties",
            json!({ "private": { "other": "1" }, "shared": { "app": "x", "pmFence": "f" } }),
        );
        assert_ne!(classify_instance(&mj, &m, &shared), InstanceKind::Plain);
    }

    /// A7: Google's count of what comes before an occurrence, cancelled ones and EXDATE'd starts
    /// included, checked against the crate.
    #[test]
    fn googles_count_before_an_occurrence() {
        let m = read(master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10",
            "EXDATE;TZID=Europe/London:20261012T090000",
        ]));
        let at = |s: &str| Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
        let split = at("2026-11-02T09:00:00Z");
        // Google lists 05-10, a cancelled 19-10 and 26-10, and leaves out the EXDATE'd 12-10.
        let listing = Listing {
            items: vec![
                plain("2026-10-05T09:00:00+01:00"),
                cancelled("2026-10-19T09:00:00+01:00"),
                plain("2026-10-26T09:00:00Z"),
                plain("2026-11-02T09:00:00Z"),
            ],
            complete: true,
        };
        assert_eq!(google_before(&m, &listing, split), Ok(4));
        // A listing that didn't run to its end can't be counted.
        let partial = Listing {
            complete: false,
            ..listing.clone()
        };
        assert_eq!(
            google_before(&m, &partial, split),
            Err(RecurError::CannotCount)
        );
        // One missing (not cancelled, not EXDATE'd): Google and the crate disagree.
        let short = Listing {
            items: vec![
                plain("2026-10-05T09:00:00+01:00"),
                plain("2026-10-26T09:00:00Z"),
            ],
            complete: true,
        };
        assert_eq!(
            google_before(&m, &short, split),
            Err(RecurError::CannotCount)
        );
    }
}
