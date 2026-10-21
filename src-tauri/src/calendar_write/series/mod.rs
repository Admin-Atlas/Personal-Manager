// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Changing a repeating Google event (#884, C8): "this event", "this and following" and "all
//! events", each worked out as an ordered list of writes before any is sent, with what each write's
//! answer has to show and how to undo it. Pure, like the rest of the core: C9 fetches, sends and
//! lands; nothing here can reach Google.
//!
//! - [`scope`]: which of the three choices a change can offer.
//! - [`plan`]: the writes themselves, and the rules for reading their answers.
//! - [`rule`]: a series rebuilt from a new rule, or made one event that doesn't repeat (C11).
//! - [`diff`]: a re-fetch of a series turned into mirror rows added and removed.
//! - [`run`]: a plan sent step by step, its answers read by the rules in [`plan`] (C9a), with no I/O.
//! - [`journal`]: what PM keeps of a "this and following" change while it runs, and how a record left
//!   from before a restart reads against Google's copies (C9a-3b-2).
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
pub mod journal;
pub mod plan;
pub mod rule;
pub mod run;
pub mod scope;

use std::fmt;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;
use serde_json::Value;

use crate::calendar_recur::expand::{crate_agrees, is_occurrence, last_occurrence};
use crate::calendar_recur::lines::{self, Kind, TextOnly};
use crate::calendar_recur::remap::RetimeError;
use crate::calendar_recur::spec::SpecError;
use crate::calendar_recur::{Point, RecurError, SeriesStart};

use super::dto::ReadOnlyReason;
use super::time::TimeError;

/// A copy of an event as Google just returned it. Planners take only this, never a mirror row: the
/// mirror clips text on purpose and holds no series facts (R1). Made by the command layer from a
/// GET's reply and nowhere else.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshEvent(Value);

impl FreshEvent {
    pub fn from_google(event: Value) -> Self {
        FreshEvent(event)
    }

    pub fn json(&self) -> &Value {
        &self.0
    }
}

/// Every stored exception of a series (occurrences changed or cancelled on their own), as Google
/// lists them by iCal UID, and nothing else: never an `instances` listing, which holds every
/// occurrence and would have a cut cancel each one. `complete` only when the listing ran to its end
/// and its cross-check ([`Exceptions::cross_checked`]) found nothing missing: planners that move or
/// cut a series refuse an incomplete one, since an exception they can't see would be lost or brought
/// back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exceptions {
    pub items: Vec<Value>,
    pub complete: bool,
}

impl Exceptions {
    /// A listing's items in series order (by original start, the master, which has none, first), so
    /// a plan built from them, and the token its choice row carries, doesn't depend on the order
    /// Google listed them in (C9 design review L3).
    pub fn sorted(mut items: Vec<Value>, complete: bool) -> Exceptions {
        items.sort_by_cached_key(|item| {
            let at = match original_of(item) {
                None => (0, 0, 0),
                Some(Point::At(t)) => (1, t.timestamp(), t.timestamp_subsec_nanos()),
                Some(Point::Day(d)) => (1, d.and_time(NaiveTime::MIN).and_utc().timestamp(), 0),
            };
            let id = item.get("id").and_then(Value::as_str).unwrap_or("");
            (at, id.to_string())
        });
        Exceptions { items, complete }
    }

    /// The listing checked against a stretch of the series' own occurrences: `window` is an
    /// `instances` listing with cancelled ones. Nothing has proved that a listing by iCal UID holds
    /// every exception (C9a's probe R0); if it held only some, a split would skip a carry and an
    /// "all events" move would bring back a deleted occurrence. So an occurrence there that's plainly
    /// changed on its own (cancelled, moved, resized or retitled) but missing from the listing marks
    /// it incomplete, as does a window that didn't run to its end, and the planners that move or cut
    /// the series then refuse (C9 design §0.8, review M3/L4).
    pub fn cross_checked(mut self, master_json: &Value, master: &Master, window: &Listing) -> Self {
        let own = |item: &&Value| {
            item.get("recurringEventId").and_then(Value::as_str) == Some(master.id.as_str())
        };
        let mut listed: Vec<Point> = self
            .items
            .iter()
            .filter(own)
            .filter_map(original_of)
            .collect();
        // A date the rule itself leaves out (an EXDATE) is no stored exception, whether or not
        // Google lists it as a cancelled occurrence.
        listed.extend(excluded(&master.recurrence, &master.start));
        let missed = window
            .items
            .iter()
            .filter(own)
            .filter(|item| plainly_changed(master_json, master, item))
            .any(|item| original_of(item).is_none_or(|p| !listed.contains(&p)));
        if missed || !window.complete {
            self.complete = false;
        }
        self
    }
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

/// Dev builds only, for C9a's probe R0: the fields the series has that an uncancelled occurrence in
/// `instances` lacks. [`same_field`] reads a carried field an occurrence lacks as its default and any
/// other as the series' (INFERRED: instances list every field their series has), so each field named
/// here is one that guess got wrong.
#[cfg(debug_assertions)]
pub fn fields_missing_vs_master(master_json: &Value, instances: &[Value]) -> Vec<&'static str> {
    let has = |v: &Value, key: &str| v.get(key).is_some_and(|f| !f.is_null());
    let uncancelled: Vec<&Value> = instances
        .iter()
        .filter(|i| i.get("status").and_then(Value::as_str) != Some("cancelled"))
        .collect();
    CARRY
        .iter()
        .chain(OTHER)
        .copied()
        .filter(|key| *key != "time" && has(master_json, key))
        .filter(|key| uncancelled.iter().any(|i| !has(i, key)))
        .collect()
}

/// The private notes PM itself puts on a series: which series a new one was split off, at which
/// occurrence and by what plan, and the fence a "Remove the new series" writes (C9). Bookkeeping, not
/// a change anyone made: an occurrence holding a copy from before one was written isn't changed on its
/// own.
pub const PM_NOTES: &[&str] = &["pmSplitOf", "pmSplitAt", "pmSplitPlan", "pmFence"];

/// A point as PM writes it down (a split's `pmSplitAt` note, the journal): a day as `YYYY-MM-DD`, an
/// instant as RFC 3339 in UTC.
pub fn point_text(point: Point) -> String {
    match point {
        Point::Day(day) => day.format("%Y-%m-%d").to_string(),
        Point::At(t) => t.to_rfc3339(),
    }
}

/// [`point_text`] read back, `None` when PM can't.
pub fn point_from_text(text: &str) -> Option<Point> {
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(Point::Day(day));
    }
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| Point::At(t.with_timezone(&Utc)))
}

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

/// Whether an occurrence is changed on its own in a way nothing can be mistaken for: cancelled, at a
/// time of its own, retitled, or with guests where its series has none. The other fields lean on
/// [`same_field`]'s unprobed defaults, and a false alarm in [`Exceptions::cross_checked`] refuses a
/// good plan.
fn plainly_changed(master_json: &Value, master: &Master, instance: &Value) -> bool {
    instance.get("status").and_then(Value::as_str) == Some("cancelled")
        || !same_time(master, instance)
        || !same_field(
            "summary",
            instance.get("summary"),
            master_json.get("summary"),
        )
        || (guests_on(instance) && !guests_on(master_json))
}

/// Whether an event has guests besides the user, or a guest list Google cut short.
pub fn guests_on(event: &Value) -> bool {
    !attendee_set(event.get("attendees")).is_empty()
        || event.get("attendeesOmitted").and_then(Value::as_bool) == Some(true)
}

/// The dates a series' own `recurrence` leaves out (its EXDATEs), where each falls in it.
pub(super) fn excluded(recurrence: &[String], start: &SeriesStart) -> Vec<Point> {
    lines::parse(recurrence)
        .iter()
        .filter_map(|l| match &l.kind {
            Kind::ExDate(dates) => Some(
                dates
                    .values
                    .iter()
                    .filter_map(|v| dates.point(v, start))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect()
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
    /// An occurrence the change would reach has guests of its own (until C12 brings Send / Don't
    /// send).
    GuestsElsewhere(NaiveDate),
    /// The series' description is formatted (HTML), and the draft would save it as plain text.
    FormattedDescription,
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
    /// Every occurrence between all-day and timed, or into another zone, and to another day, in one
    /// save: rebuilt from its own rule, which names its days, it can't move them too (C11 §12.4).
    KindAndDay,
    /// A part PM split off this series from this day is still there, and the series still repeats
    /// past it ([`parts_overlap`]).
    SplitOverlap(NaiveDate),
    /// PM couldn't list the parts it split off this series.
    PartsUnread,
    /// A repeat rule from the editor that can't be saved: a limit, a first day in a skipped hour, no
    /// occurrence (C11).
    Spec(SpecError),
    /// A new rule can't apply to one occurrence.
    RuleIsSeriesWide,
    /// A later part of this series starts on this day, and the new rule would repeat on or past it
    /// (C11 critique 1 H2).
    RepeatsPastPart(NaiveDate),
    /// A later part of this series whose start PM can't read.
    LaterPartUnread,
    /// The series is itself a part of one split earlier, and the new rule would repeat past this day,
    /// where its own rule ends now ([`LaterPart::Through`]).
    RepeatsPastItsEnd(NaiveDate),
    /// Stopping it repeating would keep no occurrence PM can name.
    NothingToKeep,
    /// The occurrence that would stay once it stops repeating has something of its own PM can't keep.
    KeptHasOwn(NaiveDate),
    /// The occurrence deleted on this day would move with the days it repeats on, and PM can't read
    /// the old rule to tell whether the day it moves to had an occurrence of its own (C11c review).
    DeletedDayUnread(NaiveDate),
}

impl fmt::Display for SeriesRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeriesRefusal::Recur(e) => e.fmt(f),
            SeriesRefusal::Retime(e) => e.fmt(f),
            SeriesRefusal::Time(e) => e.fmt(f),
            // The words the dialog's rows show: no "series", "split", "instance", "exception" or
            // "master" (C9b-1 critique 2 M4); a test walks every one.
            SeriesRefusal::Unreadable => {
                write!(f, "PM couldn't read this repeating event's details from Google.")
            }
            SeriesRefusal::NoZone => write!(
                f,
                "Google's copy of this repeating event has no time zone, so PM won't guess one."
            ),
            SeriesRefusal::NotInSeries => {
                write!(
                    f,
                    "This occurrence doesn't belong to the repeating event PM fetched."
                )
            }
            SeriesRefusal::Incomplete => write!(
                f,
                "PM couldn't see every occurrence of this repeating event that was changed on its \
                 own."
            ),
            SeriesRefusal::NoBase => write!(
                f,
                "PM no longer has this repeating event as it was when you opened it. Open it again."
            ),
            SeriesRefusal::CannotCarry(day) => write!(
                f,
                "The occurrence on {} has its own guests, attachments, video call, label or who can \
                 see it, which PM can't move to the new part.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::GuestsElsewhere(day) => write!(
                f,
                "The occurrence on {} has guests of its own, and PM can't change events with \
                 guests yet. Choose “This event” to change only this one.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::FormattedDescription => write!(
                f,
                "This repeating event's description has formatting, which saving it here as plain \
                 text would remove. Change this occurrence on its own instead."
            ),
            SeriesRefusal::VisibilityIsSeriesWide => write!(
                f,
                "Who can see an event applies to every occurrence of a repeating event, so it \
                 can't change for this one only."
            ),
            SeriesRefusal::OnlyThisDiffers => write!(
                f,
                "Every occurrence already has that; only this one is different. Choose “This \
                 event” to change it."
            ),
            SeriesRefusal::OnlyThisTimeDiffers => write!(
                f,
                "This occurrence has a time of its own, so changing every occurrence here wouldn't \
                 change it. Choose “This event” to change it, and change the others in a separate \
                 save."
            ),
            SeriesRefusal::SplitInGap(day) => write!(
                f,
                "On {} this repeating event's time falls in the hour the clocks skip, so PM can't \
                 start the new part there. Change this occurrence on its own.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::KindAndDay => write!(
                f,
                "PM can't move every occurrence to another day and change them between all-day and \
                 timed, or into another time zone, in one save. Change the day in one save and the \
                 rest in another."
            ),
            SeriesRefusal::SplitOverlap(day) => write!(
                f,
                "An earlier “this and following” change made a new part of this repeating event \
                 from {}, and this part still repeats past that day, so it may show twice. Check it \
                 in Google Calendar first.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::PartsUnread => write!(
                f,
                "PM couldn't check for a new part an earlier “this and following” change made of \
                 this repeating event, so it won't change more than this occurrence now. Try again \
                 in a moment."
            ),
            SeriesRefusal::Spec(e) => e.fmt(f),
            SeriesRefusal::RuleIsSeriesWide => write!(
                f,
                "Changing how it repeats can't apply to just this occurrence."
            ),
            SeriesRefusal::RepeatsPastPart(day) => write!(
                f,
                "PM can't make it repeat past {}, where a later part of it starts. Pick an end \
                 date before then.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::LaterPartUnread => write!(
                f,
                "PM can't tell where a later part of this repeating event starts, so it won't \
                 change how it repeats."
            ),
            SeriesRefusal::RepeatsPastItsEnd(day) => write!(
                f,
                "An earlier “this and following” change made this repeating event from part of \
                 another, and PM can't check the parts after it, so it won't make it repeat past {}. \
                 Pick an end date on or before then.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::NothingToKeep => write!(
                f,
                "Every occurrence of its repeat up to this one was deleted, so PM can't tell which \
                 one should stay. Change it in Google Calendar."
            ),
            SeriesRefusal::KeptHasOwn(day) => write!(
                f,
                "The occurrence on {}, the one that would stay, has its own colour, reminders, \
                 attachments, video call, label or who can see it, which PM can't keep when it \
                 stops repeating.",
                day.format("%d-%m-%Y")
            ),
            SeriesRefusal::DeletedDayUnread(day) => write!(
                f,
                "PM can't read how this event repeats well enough to tell where the occurrence \
                 deleted on {} would move to. Change the days it repeats on in Google Calendar, or \
                 keep this occurrence on its own day.",
                day.format("%d-%m-%Y")
            ),
        }
    }
}

impl From<SpecError> for SeriesRefusal {
    fn from(e: SpecError) -> Self {
        SeriesRefusal::Spec(e)
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
pub fn text_only(master: &Master) -> Option<TextOnly> {
    lines::series_rule(&lines::parse(&master.recurrence)).err()
}

/// Whether a part PM split off a series still overlaps it (C9 design review scope-M4): the backstop
/// for a "this and following" change whose record went, a restore or a lost settings value, say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parts {
    Clear,
    /// From this day, the first a live part starts on where the series still repeats.
    Overlap(NaiveDate),
    /// The parts, or the occurrences they're checked against, weren't listed whole.
    Unread,
}

/// Read `parts` (the live events PM's note says were split off `master`) against the series' own
/// occurrences in `window` (an `instances` listing with cancelled ones) and its stored `exceptions`.
/// A part overlaps when the series still has a live occurrence at or after the occurrence it was split
/// at (its `pmSplitAt` note): a cut ends the original before that point, wherever the new part was
/// moved to, and anything kept before it (an extra date earlier that day) isn't the overlap; an uncut
/// original still holds the split occurrence itself (review rounds 4-6). A part from before the note
/// is measured from its start. An occurrence changed on its own that a change couldn't move doesn't
/// count. Outside the window nothing is checked, as for the exceptions' cross-check. A part's own
/// exceptions, which may carry the notes too, aren't parts (C9a-3b critique L4).
pub fn parts_overlap(
    parts: &Listing,
    master: &Master,
    window: &Listing,
    exceptions: &Exceptions,
) -> Parts {
    let (series, zone) = (master.id.as_str(), master.start.zone());
    if !parts.complete {
        return Parts::Unread;
    }
    let live = |item: &&Value| item.get("status").and_then(Value::as_str) != Some("cancelled");
    let own = |item: &&Value| item.get("recurringEventId").and_then(Value::as_str) == Some(series);
    let Some(first) = split_points(parts, master)
        .into_iter()
        .min_by(|a, b| a.cmp_in(b, zone))
    else {
        return Parts::Clear;
    };
    if !window.complete {
        return Parts::Unread;
    }
    let stored: Vec<Point> = exceptions
        .items
        .iter()
        .filter(own)
        .filter_map(original_of)
        .collect();
    let still = window
        .items
        .iter()
        .filter(own)
        .filter(live)
        .filter_map(original_of)
        .filter(|p| !stored.contains(p))
        .any(|p| p.cmp_in(&first, zone) != std::cmp::Ordering::Less);
    if still {
        Parts::Overlap(first.day_in(zone))
    } else {
        Parts::Clear
    }
}

/// Each live part PM split off `master`, at its split point: its `pmSplitAt` note, or (a part from
/// before the note) its start. A part's own exceptions, which may carry the notes too, aren't parts.
fn split_points(parts: &Listing, master: &Master) -> Vec<Point> {
    let series = master.id.as_str();
    parts
        .items
        .iter()
        .filter(|p| p.get("status").and_then(Value::as_str) != Some("cancelled"))
        .filter(|p| {
            p.get("recurringEventId").is_none()
                && p.get("id").and_then(Value::as_str) != Some(series)
        })
        .filter(|p| {
            p.pointer("/extendedProperties/private/pmSplitOf")
                .and_then(Value::as_str)
                == Some(series)
        })
        .filter_map(|p| {
            p.pointer("/extendedProperties/private/pmSplitAt")
                .and_then(Value::as_str)
                .and_then(point_from_text)
                .or_else(|| point_of(p.get("start")))
        })
        .collect()
}

/// Where the earliest later part of a series starts: what a new rule for it mustn't repeat on or past,
/// or the two would show the same days (C11 critique 1 H2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LaterPart {
    /// None: the series may repeat as far as its rule says.
    #[default]
    None,
    /// The earliest later part's split point.
    At(Point),
    /// The series is itself a part of one split earlier, whose other parts PM doesn't read: it may
    /// repeat up to this day, its last occurrence's now, and no further (C11c review). Its rule ends
    /// before any later part, as every split leaves it, unless it is made to repeat on.
    Through(NaiveDate),
    /// There's a later part PM can't place: its listing didn't run to its end, or Google's id for it
    /// doesn't name its start.
    Unread,
}

/// The later parts of `master`: those PM split off it (`parts`, as [`parts_overlap`] reads them), and
/// those Google split off it, which the mirror's `series` ids name `<id>_R<start>` (a later stamp than
/// `master`'s own, when it is one of Google's parts itself; its original has no stamp).
///
/// A series that is itself a part (Google's `_R` id, or PM's note: `noted_part`) may have later parts
/// of the series it came from, which neither names: PM's, or Google's split of the original after a
/// part of PM's. Every split leaves the part before it ending first, so such a series is held to where
/// its own rule ends now ([`LaterPart::Through`]), and one that repeats for ever has none after it.
pub fn later_part(
    parts: &Listing,
    master: &Master,
    noted_part: bool,
    series: &[String],
) -> LaterPart {
    if !parts.complete {
        return LaterPart::Unread;
    }
    let zone = master.start.zone();
    let through = if noted_part || google_part_of(&master.id).1.is_some() {
        match own_end(master) {
            Ok(end) => end,
            Err(_) => return LaterPart::Unread,
        }
    } else {
        None
    };
    let mut points = split_points(parts, master);
    let (base, own) = google_part_of(&master.id);
    let own = match own {
        Some(stamp) => match google_stamp(stamp) {
            Some(p) => Some(p),
            None => return LaterPart::Unread,
        },
        None => None,
    };
    for id in series.iter().filter(|id| **id != master.id) {
        let (other_base, stamp) = google_part_of(id);
        let Some(stamp) = stamp.filter(|_| other_base == base) else {
            continue;
        };
        let Some(p) = google_stamp(stamp) else {
            return LaterPart::Unread;
        };
        if own.is_none_or(|own| p.cmp_in(&own, zone).is_gt()) {
            points.push(p);
        }
    }
    let first = points.into_iter().min_by(|a, b| a.cmp_in(b, zone));
    match (first, through) {
        (Some(p), Some(day)) if p.day_in(zone) > day => LaterPart::Through(day),
        (Some(p), _) => LaterPart::At(p),
        (None, Some(day)) => LaterPart::Through(day),
        (None, None) => LaterPart::None,
    }
}

/// The day `master`'s last occurrence falls on now, its rule's or an extra date's, `None` for a rule
/// that never ends; an error when PM can't read the rule or walk it.
fn own_end(master: &Master) -> Result<Option<NaiveDate>, RecurError> {
    let parsed = lines::parse(&master.recurrence);
    let facts = match lines::series_rule(&parsed) {
        Ok((_, facts)) => facts,
        // A rule PM can only show as text still says whether it ends: one that repeats for ever
        // has none after it (C11c review). A line PM can't read at all may be where it ends.
        Err(_) if never_ends(&parsed) => return Ok(None),
        Err(why) => return Err(RecurError::TextOnly(why)),
    };
    if facts.count.is_none() && facts.until.is_none() {
        return Ok(None);
    }
    let zone = master.start.zone();
    // Its start counts even off its own rule (RFC 5545), and the rule's occurrences come after it.
    let mut last = last_occurrence(&facts, &master.start)?.unwrap_or(master.first);
    for line in &parsed {
        if let Kind::RDate(dates) = &line.kind {
            for value in &dates.values {
                let p = dates
                    .point(value, &master.start)
                    .ok_or(RecurError::CannotCount)?;
                if p.cmp_in(&last, zone).is_gt() {
                    last = p;
                }
            }
        }
    }
    Ok(Some(last.day_in(zone)))
}

/// Every line read, and one of its rules repeats for ever: no COUNT, no UNTIL.
fn never_ends(parsed: &[lines::Line]) -> bool {
    parsed.iter().all(|l| l.kind != Kind::Unreadable)
        && parsed
            .iter()
            .any(|l| matches!(&l.kind, Kind::Rule(rule) if !rule.ends()))
}

/// A series id as Google's split names it: its base, and the stamp after `_R` when it has one.
fn google_part_of(id: &str) -> (&str, Option<&str>) {
    match id.split_once("_R") {
        Some((base, stamp)) => (base, Some(stamp)),
        None => (id, None),
    }
}

/// The start a Google split's `_R` stamp names: `YYYYMMDDTHHMMSS` (UTC, a `Z` or not), or a date.
fn google_stamp(stamp: &str) -> Option<Point> {
    let stamp = stamp.trim_end_matches('Z');
    if let Ok(at) = NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S") {
        return Some(Point::At(at.and_utc()));
    }
    NaiveDate::parse_from_str(stamp, "%Y%m%d")
        .ok()
        .map(Point::Day)
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
        pub later: LaterPart,
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
                later: LaterPart::None,
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
                later: self.later,
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

    /// C9 design review L3: however Google orders the listing, a plan sees it in series order.
    #[test]
    fn a_listing_is_read_in_series_order() {
        let mj = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        let listed = |items: Vec<Value>| {
            Exceptions::sorted(items, true)
                .items
                .iter()
                .map(|i| i["id"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        let a = cancelled("2026-10-19T09:00:00+01:00");
        let b = plain("2026-10-12T09:00:00+01:00");
        let c = plain("2026-10-26T09:00:00Z");
        let order = listed(vec![a.clone(), mj.clone(), c.clone(), b.clone()]);
        assert_eq!(order, listed(vec![c, b, mj, a]));
        assert_eq!(
            order,
            [
                "abc",
                "abc_20261012T080000Z",
                "abc_20261019T080000Z",
                "abc_20261026T090000Z"
            ]
        );
    }

    /// C9 design §0.8, review M3/L4: an occurrence plainly changed on its own that the listing by
    /// iCal UID doesn't hold, cancelled ones included, makes it incomplete; so does a window that
    /// didn't run to its end. One changed in a way only `same_field`'s defaults see doesn't.
    #[test]
    fn a_change_the_listing_misses_makes_it_incomplete() {
        let mj = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        let m = read(mj.clone());
        let moved_19 = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-20T09:00:00+01:00",
            "2026-10-20T10:00:00+01:00",
        );
        let window = |items: Vec<Value>| Listing {
            items,
            complete: true,
        };
        let checked = |listed: Vec<Value>, seen: Vec<Value>| {
            complete(listed)
                .cross_checked(&mj, &m, &window(seen))
                .complete
        };
        let plain_12 = plain("2026-10-12T09:00:00+01:00");
        // Everything changed is listed: still complete.
        assert!(checked(
            vec![mj.clone(), moved_19.clone()],
            vec![plain_12.clone(), moved_19.clone()]
        ));
        // A moved, a cancelled, a retitled and a resized one the listing lacks.
        let retitled = with(
            plain("2026-10-26T09:00:00Z"),
            "summary",
            json!("Standup (room 4)"),
        );
        let longer = moved(
            "2026-10-26T09:00:00Z",
            "2026-10-26T09:00:00Z",
            "2026-10-26T11:00:00Z",
        );
        for missing in [
            moved_19.clone(),
            cancelled("2026-10-19T09:00:00+01:00"),
            retitled,
            longer,
        ] {
            assert!(
                !checked(vec![mj.clone()], vec![plain_12.clone(), missing.clone()]),
                "{missing}"
            );
        }
        // A date the rule leaves out (EXDATE) is no stored exception, however Google lists it.
        let skipped_19 = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261019T090000",
        ]);
        assert!(
            complete(vec![skipped_19.clone()])
                .cross_checked(
                    &skipped_19,
                    &read(skipped_19.clone()),
                    &window(vec![cancelled("2026-10-19T09:00:00+01:00")])
                )
                .complete
        );
        // An occurrence with guests of its own, in a series with none, is changed on its own.
        let invited = with(
            plain_12.clone(),
            "attendees",
            json!([{ "email": "a@example.com" }]),
        );
        assert!(!checked(vec![mj.clone()], vec![invited.clone()]));
        assert!(checked(vec![mj.clone(), invited.clone()], vec![invited]));
        // Another series' occurrences aren't this one's to account for.
        let theirs = with(moved_19.clone(), "recurringEventId", json!("other"));
        assert!(checked(vec![mj.clone()], vec![theirs]));
        // A description of its own is for the planners' carry; it isn't plain enough to refuse on.
        let described = with(plain_12.clone(), "description", json!("Own notes"));
        assert!(checked(vec![mj.clone()], vec![described]));
        // A window cut short can't vouch for the listing.
        let short = Listing {
            items: vec![plain_12],
            complete: false,
        };
        assert!(
            !complete(vec![mj.clone()])
                .cross_checked(&mj, &m, &short)
                .complete
        );
        // An incomplete listing stays incomplete.
        let partial = Exceptions {
            items: vec![mj.clone()],
            complete: false,
        };
        assert!(!partial.cross_checked(&mj, &m, &window(Vec::new())).complete);
    }

    /// scope-M4: a part PM split off overlaps its original only while the original still has an
    /// occurrence at or after the one it was split at.
    #[test]
    fn a_part_overlaps_only_where_the_original_still_repeats() {
        let whole = |items: Vec<Value>| Listing {
            items,
            complete: true,
        };
        // Split at Monday 19-10 09:00 London, whatever the part's start was moved to.
        let part = |start: &str| {
            json!({
                "id": "pmtail",
                "status": "confirmed",
                "start": { "dateTime": start, "timeZone": "Europe/London" },
                "extendedProperties": { "private": {
                    "pmSplitOf": "abc",
                    "pmSplitAt": "2026-10-19T08:00:00+00:00",
                    "pmSplitPlan": "p",
                } },
            })
        };
        let parts = whole(vec![part("2026-10-19T10:00:00+01:00")]);
        let october = whole(vec![
            plain("2026-10-12T09:00:00+01:00"),
            plain("2026-10-19T09:00:00+01:00"),
            plain("2026-10-26T09:00:00Z"),
        ]);
        let none = complete(Vec::new());
        let weekly_m = Master::read(&fresh(master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]))).unwrap();
        let read = |parts: &Listing, window: &Listing, exceptions: &Exceptions| {
            parts_overlap(parts, &weekly_m, window, exceptions)
        };
        let day = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        // Uncut: the original still holds 19-10 and 26-10.
        assert_eq!(read(&parts, &october, &none), Parts::Overlap(day));
        // Cut: it ends on 12-10, before the part.
        let cut = whole(vec![plain("2026-10-12T09:00:00+01:00")]);
        assert_eq!(read(&parts, &cut, &none), Parts::Clear);
        // Uncut, its last occurrence in the window the split day's 09:00, the part retimed to 10:00
        // that day: both show that day (review round 5).
        let last_day = whole(vec![
            plain("2026-10-12T09:00:00+01:00"),
            plain("2026-10-19T09:00:00+01:00"),
        ]);
        assert_eq!(read(&parts, &last_day, &none), Parts::Overlap(day));
        // An extra date earlier on the split day stays on the cut original, wherever the part was
        // moved to: before the split point, so no overlap (review rounds 4-6).
        let extra = whole(vec![
            plain("2026-10-12T09:00:00+01:00"),
            plain("2026-10-19T07:00:00+01:00"),
        ]);
        for start in [
            "2026-10-19T10:00:00+01:00",
            "2026-10-19T06:00:00+01:00",
            "2026-10-15T09:00:00+01:00",
        ] {
            let moved_part = whole(vec![part(start)]);
            assert_eq!(read(&moved_part, &extra, &none), Parts::Clear, "{start}");
        }
        // Moved to a later day, the original uncut with nothing after the split occurrence in the
        // window (a yearly event, say, or the last of a COUNT): it still shows that occurrence.
        let later = whole(vec![part("2026-10-21T09:00:00+01:00")]);
        assert_eq!(read(&later, &last_day, &none), Parts::Overlap(day));
        assert_eq!(read(&later, &cut, &none), Parts::Clear);
        // A part from before the note is measured from its start.
        let mut legacy = part("2026-10-19T10:00:00+01:00");
        legacy["extendedProperties"]["private"]
            .as_object_mut()
            .unwrap()
            .remove("pmSplitAt");
        let legacy = whole(vec![legacy]);
        assert_eq!(read(&legacy, &october, &none), Parts::Overlap(day));
        assert_eq!(read(&legacy, &extra, &none), Parts::Clear);
        // An occurrence a change couldn't move, left on the original, isn't the rule repeating; nor
        // is a cancelled one.
        let left = moved(
            "2026-10-26T09:00:00Z",
            "2026-10-26T11:00:00Z",
            "2026-10-26T12:00:00Z",
        );
        let with_left = whole(vec![
            plain("2026-10-12T09:00:00+01:00"),
            left.clone(),
            cancelled("2026-11-02T09:00:00Z"),
        ]);
        assert_eq!(
            read(&parts, &with_left, &complete(vec![left.clone()])),
            Parts::Clear
        );
        assert_eq!(read(&parts, &with_left, &none), Parts::Overlap(day));
        // A part's own exception carries the note too, and a cancelled part is no part.
        let exception = with(
            part("2026-10-05T10:00:00+01:00"),
            "recurringEventId",
            json!("pmtail"),
        );
        let gone = with(
            part("2026-10-05T10:00:00+01:00"),
            "status",
            json!("cancelled"),
        );
        assert_eq!(
            read(&whole(vec![exception, gone]), &october, &none),
            Parts::Clear
        );
        // No parts at all needs no window; a part, a whole one.
        let short = Listing {
            items: october.items.clone(),
            complete: false,
        };
        assert_eq!(read(&whole(Vec::new()), &short, &none), Parts::Clear);
        assert_eq!(read(&parts, &short, &none), Parts::Unread);
        let unread = Listing {
            items: Vec::new(),
            complete: false,
        };
        assert_eq!(read(&unread, &october, &none), Parts::Unread);
        // Another series' note isn't this one's.
        let other = with(
            part("2026-10-19T10:00:00+01:00"),
            "extendedProperties",
            json!({ "private": { "pmSplitOf": "xyz" } }),
        );
        assert_eq!(read(&whole(vec![other]), &october, &none), Parts::Clear);
    }

    /// C11 critique 1 H2: where a new rule must stop, the earliest later part's start, whether PM
    /// split it off (its note) or Google did (its `_R` id names it).
    #[test]
    fn a_later_part_is_where_a_new_rule_must_stop() {
        let parts = |items: Vec<Value>| Listing {
            items,
            complete: true,
        };
        let part = |id: &str, at: &str| {
            json!({
                "id": id,
                "status": "confirmed",
                "start": { "dateTime": "2026-10-19T10:00:00+01:00" },
                "extendedProperties": { "private": { "pmSplitOf": "abc", "pmSplitAt": at } },
            })
        };
        let weekly_m = Master::read(&fresh(master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]))).unwrap();
        let none: Vec<String> = Vec::new();
        assert_eq!(
            later_part(&parts(Vec::new()), &weekly_m, false, &none),
            LaterPart::None
        );
        // The earlier of two of PM's, a cancelled one none.
        let two = parts(vec![
            part("p2", "2026-11-02T09:00:00+00:00"),
            part("p1", "2026-10-19T08:00:00+00:00"),
            with(
                part("p0", "2026-10-12T08:00:00+00:00"),
                "status",
                json!("cancelled"),
            ),
        ]);
        assert_eq!(
            later_part(&two, &weekly_m, false, &none),
            LaterPart::At(point("2026-10-19T08:00:00Z"))
        );
        // Google's own, from its id; another series' and the master's own aren't.
        let series = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            later_part(
                &parts(Vec::new()),
                &weekly_m,
                false,
                &series(&["abc", "xyz_R20261019T080000", "abc_R20261102T090000"])
            ),
            LaterPart::At(point("2026-11-02T09:00:00Z"))
        );
        assert_eq!(
            later_part(&two, &weekly_m, false, &series(&["abc_R20261012T080000Z"])),
            LaterPart::At(point("2026-10-12T08:00:00Z"))
        );
        assert_eq!(
            later_part(
                &parts(Vec::new()),
                &weekly_m,
                false,
                &series(&["abc_R20261102"])
            ),
            LaterPart::At(Point::Day(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()))
        );
        // A Google part itself: only a later stamp is a later part, and its original none.
        let google_part = with(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]),
            "id",
            json!("abc_R20261019T080000"),
        );
        let google_part = Master::read(&fresh(google_part)).unwrap();
        assert_eq!(
            later_part(
                &parts(Vec::new()),
                &google_part,
                false,
                &series(&["abc", "abc_R20261005T080000", "abc_R20261019T080000"])
            ),
            LaterPart::None
        );
        assert_eq!(
            later_part(
                &parts(Vec::new()),
                &google_part,
                false,
                &series(&["abc", "abc_R20261102T090000"])
            ),
            LaterPart::At(point("2026-11-02T09:00:00Z"))
        );
        // A stamp PM can't read, or parts not listed whole: it can't say.
        assert_eq!(
            later_part(
                &parts(Vec::new()),
                &weekly_m,
                false,
                &series(&["abc_Rsoon"])
            ),
            LaterPart::Unread
        );
        let short = Listing {
            items: Vec::new(),
            complete: false,
        };
        assert_eq!(
            later_part(&short, &weekly_m, false, &none),
            LaterPart::Unread
        );
    }

    /// C11c review: a series that is itself a part, by PM's note or Google's `_R` id, may have later
    /// parts of the series it came from that neither names, so it's held to where its own rule ends
    /// now: its last occurrence's day, by its UNTIL or its COUNT, or a later extra date. One that
    /// repeats for ever has none after it, and an original is held only by its own parts.
    #[test]
    fn a_part_is_held_to_where_it_ends_now() {
        let read = |recurrence: &[&str], id: &str| {
            Master::read(&fresh(with(master(recurrence), "id", json!(id)))).unwrap()
        };
        let parts = |items: Vec<Value>| Listing {
            items,
            complete: true,
        };
        let none: Vec<String> = Vec::new();
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        // Cut before Monday 02-11, as the split that made a later part left it: Monday 26-10.
        let until = ["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261102T085959Z"];
        for (m, noted) in [
            (read(&until, "p1"), true),
            (read(&until, "abc_R20261005T080000"), false),
        ] {
            assert_eq!(
                later_part(&parts(Vec::new()), &m, noted, &none),
                LaterPart::Through(day(26))
            );
        }
        // An original with the same rule is held only by its own parts.
        assert_eq!(
            later_part(&parts(Vec::new()), &read(&until, "abc"), false, &none),
            LaterPart::None
        );
        // Four times from 05-10, its last Monday 26-10; an extra date after it, Wednesday 28-10.
        let counted = read(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4"], "p1");
        assert_eq!(
            later_part(&parts(Vec::new()), &counted, true, &none),
            LaterPart::Through(day(26))
        );
        let extra = read(
            &[until[0], "RDATE;TZID=Europe/London:20261028T090000"],
            "p1",
        );
        assert_eq!(
            later_part(&parts(Vec::new()), &extra, true, &none),
            LaterPart::Through(day(28))
        );
        // A part that repeats for ever has none.
        let forever = read(&["RRULE:FREQ=WEEKLY;BYDAY=MO"], "p1");
        assert_eq!(
            later_part(&parts(Vec::new()), &forever, true, &none),
            LaterPart::None
        );
        // Its own later part counts where it's earlier than where it ends, never after.
        let own = |at: &str| {
            json!({
                "id": "p9",
                "status": "confirmed",
                "start": { "dateTime": at },
                "extendedProperties": { "private": { "pmSplitOf": "p1", "pmSplitAt": at } },
            })
        };
        let p1 = read(&until, "p1");
        assert_eq!(
            later_part(
                &parts(vec![own("2026-10-19T08:00:00+00:00")]),
                &p1,
                true,
                &none
            ),
            LaterPart::At(point("2026-10-19T08:00:00Z"))
        );
        assert_eq!(
            later_part(
                &parts(vec![own("2026-11-09T09:00:00+00:00")]),
                &p1,
                true,
                &none
            ),
            LaterPart::Through(day(26))
        );
        // A rule PM can't walk can't say where it ends.
        let custom = read(&["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2;COUNT=3"], "p1");
        assert_eq!(
            later_part(&parts(Vec::new()), &custom, true, &none),
            LaterPart::Unread
        );
        // But one that repeats for ever says so, walked or not: a part of either kind has none
        // after it (C11c review); unless a line PM can't read at all may be where it ends.
        let custom_forever = ["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"];
        for (m, noted) in [
            (read(&custom_forever, "p1"), true),
            (read(&custom_forever, "abc_R20261005T080000"), false),
        ] {
            assert_eq!(
                later_part(&parts(Vec::new()), &m, noted, &none),
                LaterPart::None
            );
        }
        let unreadable = read(
            &[
                custom_forever[0],
                "EXDATE;TZID=Mars/Olympus:20261013T090000",
            ],
            "p1",
        );
        assert_eq!(
            later_part(&parts(Vec::new()), &unreadable, true, &none),
            LaterPart::Unread
        );
    }
}
