// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A repeating event rebuilt from a rule in the editor's set (#884, C11): a new rule for every
//! occurrence or from one on, and "Does not repeat". Where it starts is the rule's first day on or
//! after the day asked ([`sync_start`]), its times are written with their zone, and an occurrence that
//! was deleted stays deleted wherever the new rule still has its day. C11d routes a move between
//! all-day and timed, or into another zone, through the same rebuild.
//!
//! Beside the C9 guards every series plan keeps (R2, guests, formatting, the overlap refusal), two of
//! the C11 critiques' own:
//! - "Does not repeat" for every occurrence keeps the first one still there, with its own changes,
//!   never a deleted one (critique 1 H1);
//! - no rebuilt rule repeats on or past a later part of the series, which would show those days
//!   twice (critique 1 H2).
//!
//! Like the rest of the core, nothing here can reach Google, and nothing from a Google reply goes into
//! a log or panic message.

use chrono::{Datelike, NaiveDate, TimeDelta};
use chrono_tz::Tz;
use serde_json::{Map, Value};

use crate::calendar_recur::expand::{first_kept, is_occurrence, occurrences_from, sync_start};
use crate::calendar_recur::lines::{self, Kind, RuleFacts, TextOnly};
use crate::calendar_recur::remap::{exdate_lines, RetimeError};
use crate::calendar_recur::spec::{
    day_phrase, rule_from_spec, spec_of, Ends, Pattern, RecurrenceSpec, SpecError, YearlyOn,
};
use crate::calendar_recur::split::remaining_count;
use crate::calendar_recur::{rfc_instant, Point, RecurError, SeriesStart};

use super::super::dto::{EventPatchDraft, Notify, RepeatDraft, TimeDraft};
use super::super::patch::{build_patch, drop_other_kind, is_gone, when_eq};
use super::super::plan::is_plain_segment;
use super::super::time;
use super::diff::{Expect, Removes};
use super::plan::{
    all_plan, cancel_exception, cancelled_points, cut_step, draft_fields, exceptions_from,
    insert_step, keeps_formatting, last_kept, no_guests_from, not_carried, own_exceptions,
    rest_start, snapshot, split_unchanged_since_opened, start_of, tail_body,
    unchanged_since_opened, when_nodes_in, NotPlanned, Op, Recheck, SeriesInput, SeriesPlan,
    SeriesWarning, Shows, SplitCut, Stage, Step, Undo, TAIL_WATCH,
};
use super::scope::{RuleChange, Scope};
use super::{
    classify_instance, excluded, original_of, point_of, InstanceKind, LaterPart, Length, Master,
    SeriesRefusal,
};

/// What `draft` asks of `master`'s rule, as the choice reads it ([`super::scope::Change`]): `None`
/// when it leaves the rule alone or asks for the very rule the series has. A rule PM can't read is a
/// new one, which its planner refuses in words.
pub fn rule_change(draft: &EventPatchDraft, master: &Master) -> Option<RuleChange> {
    match &draft.recurrence {
        None => None,
        Some(RepeatDraft::Once) => Some(RuleChange::Stop),
        Some(RepeatDraft::Rule { rule }) => match rule.to_spec() {
            Ok(spec) if same_rule(&spec, master) => None,
            _ => Some(RuleChange::New),
        },
    }
}

/// What a planner is asked to make of the series' rule.
pub(super) enum RuleAsk {
    New(RecurrenceSpec),
    /// "Does not repeat".
    Stop,
}

/// The rule change `draft` asks for, read as [`rule_change`] reads it, so the rows and the plans
/// agree on what is one (C11 critique 1 L12): `None` for none, a rule PM can't read refused.
pub(super) fn rule_ask(
    draft: &EventPatchDraft,
    master: &Master,
) -> Result<Option<RuleAsk>, NotPlanned> {
    Ok(match &draft.recurrence {
        None => None,
        Some(RepeatDraft::Once) => Some(RuleAsk::Stop),
        Some(RepeatDraft::Rule { rule }) => {
            let spec = rule.to_spec().map_err(SeriesRefusal::Spec)?;
            (!same_rule(&spec, master)).then_some(RuleAsk::New(spec))
        }
    })
}

/// Whether `spec` is the series' own rule, read as the editor's Repeat row reads it.
fn same_rule(spec: &RecurrenceSpec, master: &Master) -> bool {
    spec_of(&master.recurrence, &master.start).as_ref() == Some(spec)
}

/// What a series is asked to become.
pub(super) struct RebuildAsk<'a> {
    pub spec: &'a RecurrenceSpec,
    /// The day it should start on (the rule may move it later), at its wall time, kind and zone.
    pub start: SeriesStart,
    pub length: Length,
    /// The zone its end is written in (a flight can end in another).
    pub end_zone: Tz,
    /// The old series' deleted occurrences (its EXDATEs and cancelled occurrences), as points in it.
    pub deleted: &'a [Point],
    /// The old series' start, where `deleted` and `extra` were read.
    pub old_start: &'a SeriesStart,
    /// The old series' rule, read; `None` when PM can only show it as text.
    pub old_rule: Option<RuleFacts>,
    /// How many days the change moves every occurrence (a day move with the new rule, C11 critique
    /// 2 M4), counted in `days`: a deleted day the new rule doesn't make is looked for that much
    /// later ([`Landing`]).
    pub shift: i64,
    /// How an old occurrence's day reads in the series rebuilt ([`Days`]).
    pub days: Days,
    /// Where the occurrence opened is in the old series.
    pub opened_at: Point,
    /// The day the draft puts the occurrence opened on, in `start`'s terms: where the new rule makes
    /// it, that slot is the opened occurrence's alone (C11c review), unless it is an extra date kept,
    /// which stays where it is.
    pub opened: NaiveDate,
    /// The old series' RDATE lines, as Google wrote them.
    pub extra: &'a [String],
}

/// A series rebuilt.
pub(super) struct Rebuilt {
    /// Where it starts: the rule's first day, the asked wall time.
    pub start: SeriesStart,
    pub first: Point,
    /// Google's `start` and `end`; a timed node always names its `timeZone` (G6).
    pub nodes: (Value, Value),
    /// The RRULE, then any EXDATE line, then any RDATE line kept.
    pub lines: Vec<String>,
    /// The new rule, read.
    pub facts: RuleFacts,
    /// The extra dates (RDATEs) kept, where each still is an occurrence.
    pub extra: Vec<Point>,
    /// The start isn't on the day asked.
    pub moved: bool,
    /// How an old occurrence's day reads in it ([`Days`]).
    pub days: Days,
    /// Where the draft puts the occurrence opened, when the new rule makes that slot: nothing else
    /// lands there ([`Landing`]).
    pub opened: Option<Point>,
}

/// The series `ask` describes, refused in words where it can't be written: a rule outside the
/// limits, a first day whose time the clocks skip, an extra date it can't keep.
pub(super) fn rebuild(ask: &RebuildAsk) -> Result<Rebuilt, NotPlanned> {
    let synced = sync_start(ask.spec, &ask.start).map_err(SeriesRefusal::Spec)?;
    let first = synced
        .point()
        .ok_or(SeriesRefusal::Spec(SpecError::Gap(synced.date())))?;
    let rule = rule_from_spec(ask.spec, &synced).map_err(SeriesRefusal::Spec)?;
    let parsed = lines::parse(std::slice::from_ref(&rule));
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    // An extra date names a moment of the old series: it stays only where that is still one of
    // this series' times, as a move refuses it today.
    let same_times = ask.shift == 0 && same_wall_time(ask.old_start, &synced);
    if !ask.extra.is_empty() && !same_times {
        return Err(RetimeError::HasExtraDates.into());
    }
    // So each one kept is still an occurrence where it was.
    let mut extra_points = Vec::new();
    for line in lines::parse(ask.extra) {
        if let Kind::RDate(dates) = &line.kind {
            for value in &dates.values {
                extra_points.push(
                    dates
                        .point(value, ask.old_start)
                        .ok_or(RecurError::CannotCount)?,
                );
            }
        }
    }
    // An extra date opened and kept stays where it is, as its own RDATE: it takes no slot of the new
    // rule's, so an occurrence deleted there stays deleted (C11c review).
    let opened = match on_its_day(&synced, ask.opened) {
        Some(q) if !extra_points.contains(&ask.opened_at) && is_occurrence(&facts, &synced, q)? => {
            Some(q)
        }
        _ => None,
    };
    // A deleted day stays deleted wherever the new rule still makes it (from its start on), and a
    // deleted extra date wherever it's kept (RM1). An EXDATE neither makes is never written: Google
    // refuses one that "doesn't point to a valid instance".
    let landing = Landing {
        facts: &facts,
        start: &synced,
        old: ask.old_rule.as_ref().map(|f| (f, ask.old_start)),
        shift: ask.shift,
        days: ask.days,
        opened,
    };
    let mut deleted = Vec::new();
    for p in ask.deleted {
        if extra_points.contains(p) {
            deleted.push(*p);
            continue;
        }
        if let Some(q) = landing.at(*p)? {
            deleted.push(q);
        }
    }
    let mut out = vec![rule];
    out.extend(exdate_lines(&deleted).into_iter().map(|l| l.raw));
    out.extend(ask.extra.iter().cloned());
    let nodes = when_nodes_in(synced.zone(), ask.end_zone, first, ask.length)?;
    Ok(Rebuilt {
        moved: synced.date() != ask.start.date(),
        start: synced,
        first,
        nodes,
        lines: out,
        facts,
        extra: extra_points,
        days: ask.days,
        opened,
    })
}

/// How the old series' days read in a series rebuilt from it (C11c review): an occurrence's wall day
/// in the old series' own zone, moved on as far as the occurrence opened moves between the two zones'
/// walls. Both rules repeat by wall day in their own zone, so each occurrence keeps its place in the
/// week whatever either zone's clocks do, and a move into another zone that keeps every moment moves
/// no day. Across a change between timed and all-day, the wall day is the day.
#[derive(Clone, Copy)]
pub(super) struct Days {
    /// The old series' zone.
    zone: Tz,
    /// How many days on the new series' wall is from the old's: 0 unless both are timed.
    offset: i64,
}

impl Days {
    /// The days of a series moved from `old` to `new`, measured at `opened`, where the occurrence
    /// opened is in the old series.
    fn new(old: &SeriesStart, new: &SeriesStart, opened: Point) -> Days {
        let offset = match (old, new) {
            (SeriesStart::Timed { zone: was, .. }, SeriesStart::Timed { zone: is, .. }) => {
                (opened.day_in(*is) - opened.day_in(*was)).num_days()
            }
            _ => 0,
        };
        Days {
            zone: old.zone(),
            offset,
        }
    }

    /// The day the old occurrence at `p` is on, in the new series' days.
    fn of(self, p: Point) -> Option<NaiveDate> {
        moved_day(p.day_in(self.zone), self.offset)
    }

    /// The old series' own day for `day`, one of the new series'.
    fn back(self, day: NaiveDate) -> Option<NaiveDate> {
        moved_day(day, -self.offset)
    }
}

/// The day a yearly rule "on its own date" should start on: the first day on or after `from` with
/// `like`'s month and day (29 February: the next leap year's). So "Annually on 13 October", picked on
/// the 13-10 occurrence of a series that began on 05-10, repeats on 13 October as its words said.
pub(super) fn yearly_anchor(from: NaiveDate, like: NaiveDate) -> NaiveDate {
    // A leap year comes within eight years, a century's skipped one included.
    (from.year()..=from.year() + 8)
        .filter_map(|y| NaiveDate::from_ymd_opt(y, like.month(), like.day()))
        .find(|d| *d >= from)
        .unwrap_or(like)
}

/// `day` moved `shift` days.
fn moved_day(day: NaiveDate, shift: i64) -> Option<NaiveDate> {
    day.checked_add_signed(TimeDelta::try_days(shift)?)
}

/// `start` moved to `day`: its wall time, kind and zone kept.
fn on_day(start: &SeriesStart, day: NaiveDate) -> SeriesStart {
    match *start {
        SeriesStart::Timed { local, zone } => SeriesStart::Timed {
            local: day.and_time(local.time()),
            zone,
        },
        SeriesStart::AllDay(_) => SeriesStart::AllDay(day),
    }
}

/// Where the series starting at `start` puts an occurrence on `day`, should its rule make one: its
/// wall time read as RFC 5545 reads a skipped hour, or the day.
fn on_its_day(start: &SeriesStart, day: NaiveDate) -> Option<Point> {
    match *start {
        SeriesStart::Timed { local, zone } => {
            rfc_instant(zone, day.and_time(local.time())).map(Point::At)
        }
        SeriesStart::AllDay(_) => Some(Point::Day(day)),
    }
}

/// Where the old series' occurrences, deleted or changed on their own, land in a series rebuilt under
/// a new rule (C11c review).
struct Landing<'a> {
    /// The rebuilt series' rule and start.
    facts: &'a RuleFacts,
    start: &'a SeriesStart,
    /// The old series' rule, read, and its start; `None` when PM can only show the rule as text.
    old: Option<(&'a RuleFacts, &'a SeriesStart)>,
    /// How many days the change moves every occurrence, in `days`.
    shift: i64,
    /// How an occurrence's day reads in the new series ([`Days`]).
    days: Days,
    /// Where the draft puts the occurrence opened: never another's.
    opened: Option<Point>,
}

impl Landing<'_> {
    /// Where the old occurrence at `p` is: on its own day where the new rule still makes it, so it
    /// stays as the user left it there; else, when the change moves the day, that many days on, but
    /// only where the old series had no occurrence of its own, since which of the two it is would
    /// then be a guess; else nowhere. Never where the draft puts the occurrence opened, which the user
    /// put there. An old rule PM can't read can't say whether the day moved to had one of its own:
    /// refused in words where the new rule makes it, rather than guessed, naming the day deleted as
    /// the series has it, in its own zone.
    fn at(&self, p: Point) -> Result<Option<Point>, NotPlanned> {
        let day = self.days.of(p).ok_or(SeriesRefusal::Unreadable)?;
        let landed = match self.made(day)? {
            Some(q) => Some(q),
            None => match moved_day(day, self.shift) {
                None => None,
                Some(moved) => match self.old {
                    Some((facts, start)) if self.had(facts, start, moved)? => None,
                    Some(_) => self.made(moved)?,
                    None => match self.made(moved)? {
                        Some(_) => {
                            let own = p.day_in(self.days.zone);
                            return Err(SeriesRefusal::DeletedDayUnread(own).into());
                        }
                        None => None,
                    },
                },
            },
        };
        Ok(landed.filter(|q| Some(*q) != self.opened))
    }

    /// Whether the old series, from `start` under `facts`, had an occurrence of its own on `day`, one
    /// of the new series' days: read on its own day in its own zone ([`Days::back`]).
    fn had(
        &self,
        facts: &RuleFacts,
        start: &SeriesStart,
        day: NaiveDate,
    ) -> Result<bool, RecurError> {
        match self.days.back(day) {
            Some(own) => has_day(facts, start, own),
            None => Ok(true),
        }
    }

    /// The rebuilt series' occurrence on `day`, if its rule makes one.
    fn made(&self, day: NaiveDate) -> Result<Option<Point>, RecurError> {
        match on_its_day(self.start, day) {
            Some(q) if is_occurrence(self.facts, self.start, q)? => Ok(Some(q)),
            _ => Ok(None),
        }
    }
}

/// Whether the series from `start` under `facts` has an occurrence on `day`, read in its own zone. A
/// day PM can't place counts as one, so nothing lands there on a guess.
fn has_day(facts: &RuleFacts, start: &SeriesStart, day: NaiveDate) -> Result<bool, RecurError> {
    let zone = start.zone();
    let from = match start {
        SeriesStart::AllDay(_) => Point::Day(day),
        SeriesStart::Timed { .. } => {
            match rfc_instant(zone, day.and_time(chrono::NaiveTime::MIN)) {
                Some(t) => Point::At(t),
                None => return Ok(true),
            }
        }
    };
    Ok(occurrences_from(facts, start, from, 1)?
        .first()
        .is_some_and(|q| q.day_in(zone) == day))
}

/// Where one event that doesn't repeat starts, at `start`: a time the clocks skip that day is refused
/// as a one-off's save refuses it, never nudged (R6).
fn one_event_at(start: &SeriesStart) -> Result<Point, NotPlanned> {
    start.point().ok_or_else(|| match *start {
        SeriesStart::Timed { local, zone } => SeriesRefusal::Time(time::TimeError::Gap {
            zone: zone.name().to_string(),
            wall: local.format("%d-%m-%Y %H:%M").to_string(),
        })
        .into(),
        SeriesStart::AllDay(_) => SeriesRefusal::Unreadable.into(),
    })
}

/// Two starts of the same kind, zone and wall time.
fn same_wall_time(a: &SeriesStart, b: &SeriesStart) -> bool {
    match (a, b) {
        (SeriesStart::AllDay(_), SeriesStart::AllDay(_)) => true,
        (SeriesStart::Timed { local: x, zone: y }, SeriesStart::Timed { local: v, zone: w }) => {
            x.time() == v.time() && y == w
        }
        _ => false,
    }
}

/// A rebuilt rule mustn't repeat on or past a later part of the series (C11 critique 1 H2): those
/// days would show twice, and the overlap backstop would then refuse every change past this one.
/// Measured in the rebuilt series' own terms: from the later part's instant for a timed one, from its
/// day for an all-day one, so an equal day across kinds counts (critique 1 L3). A series that is
/// itself a part, held to where its rule ends now, mustn't repeat on any day after it (C11c review).
fn clear_of_later(later: LaterPart, rebuilt: &Rebuilt, old_zone: Tz) -> Result<(), NotPlanned> {
    let (at, refusal) = match later {
        LaterPart::None => return Ok(()),
        LaterPart::Unread => return Err(SeriesRefusal::LaterPartUnread.into()),
        LaterPart::At(at) => (at, SeriesRefusal::RepeatsPastPart(at.day_in(old_zone))),
        LaterPart::Through(last) => {
            let after = last.succ_opt().ok_or(SeriesRefusal::LaterPartUnread)?;
            (Point::Day(after), SeriesRefusal::RepeatsPastItsEnd(last))
        }
    };
    let day = at.day_in(old_zone);
    let from = match (rebuilt.start, at) {
        (SeriesStart::AllDay(_), _) => Point::Day(day),
        (SeriesStart::Timed { .. }, Point::At(_)) => at,
        (SeriesStart::Timed { zone, .. }, Point::Day(d)) => {
            match rfc_instant(zone, d.and_time(chrono::NaiveTime::MIN)) {
                Some(t) => Point::At(t),
                None => return Err(SeriesRefusal::LaterPartUnread.into()),
            }
        }
    };
    if occurrences_from(&rebuilt.facts, &rebuilt.start, from, 1)?.is_empty() {
        Ok(())
    } else {
        Err(refusal.into())
    }
}

/// Where the draft puts the occurrence, when it changes its time: its start as typed (its day, wall
/// time, kind and zone), its length, and the zone of its end. Read as a move reads it, but another
/// zone is no refusal here: a new rule rebuilds the series in it (C11 §9.4.3).
struct Asked {
    start: SeriesStart,
    length: Length,
    end_zone: Tz,
}

fn asked_time(draft: &EventPatchDraft, instance: &Value) -> Result<Option<Asked>, NotPlanned> {
    let Some(draft_time) = &draft.time else {
        return Ok(None);
    };
    let when = time::resolve_against(draft_time, instance.get("start"), instance.get("end"))?;
    if when_eq(instance.get("start"), Some(&when.start))
        && when_eq(instance.get("end"), Some(&when.end))
    {
        return Ok(None);
    }
    let length = match (point_of(Some(&when.start)), point_of(Some(&when.end))) {
        (Some(Point::At(s)), Some(Point::At(e))) => Length::Timed(e - s),
        (Some(Point::Day(s)), Some(Point::Day(e))) => Length::AllDay((e - s).num_days()),
        _ => return Err(SeriesRefusal::Unreadable.into()),
    };
    let end_zone = match draft_time {
        TimeDraft::Timed { end_zone, .. } => time::parse_zone(end_zone)?,
        TimeDraft::AllDay { .. } => Tz::UTC,
    };
    Ok(Some(Asked {
        start: time::series_start(draft_time)?,
        length,
        end_zone,
    }))
}

/// Google's `start` and `end` for `first`, `length` long, its start in `zone` and its end in
/// `end_zone`; `null` for the other kind's keys when that isn't `master`'s kind, since a PATCH merges
/// the nodes and would keep both.
fn nodes_for_master(
    master: &Master,
    zone: Tz,
    end_zone: Tz,
    first: Point,
    length: Length,
) -> Result<(Value, Value), NotPlanned> {
    let (mut start, mut end) = when_nodes_in(zone, end_zone, first, length)?;
    let master_all_day = matches!(master.start, SeriesStart::AllDay(_));
    if matches!(first, Point::Day(_)) != master_all_day {
        drop_other_kind(&mut start);
        drop_other_kind(&mut end);
    }
    Ok((start, end))
}

/// An occurrence's own `start` and `end`, written as PM writes every node: in its own zone (else the
/// series'), with its length.
fn own_nodes(item: &Value, master: &Master) -> Result<(Point, Tz, Tz, Length), NotPlanned> {
    let (start, end) = (point_of(item.get("start")), point_of(item.get("end")));
    let zone_of = |key: &str| {
        item.get(key)
            .and_then(|n| n.get("timeZone"))
            .and_then(Value::as_str)
            .and_then(|z| z.parse::<Tz>().ok())
    };
    let zone = zone_of("start").unwrap_or(master.start.zone());
    let end_zone = zone_of("end").unwrap_or(zone);
    match (start, end) {
        (Some(Point::At(s)), Some(Point::At(e))) => {
            Ok((Point::At(s), zone, end_zone, Length::Timed(e - s)))
        }
        (Some(Point::Day(s)), Some(Point::Day(e))) => Ok((
            Point::Day(s),
            Tz::UTC,
            Tz::UTC,
            Length::AllDay((e - s).num_days()),
        )),
        _ => Err(SeriesRefusal::Unreadable.into()),
    }
}

/// The series' deleted occurrences: its EXDATEs and its cancelled ones, from a complete listing.
fn deleted_points(master: &Master, input: &SeriesInput) -> Result<Vec<Point>, NotPlanned> {
    let mut deleted = excluded(&master.recurrence, &master.start);
    deleted.extend(cancelled_points(master, input.exceptions)?);
    Ok(deleted)
}

/// A rule is replaced whole, so every line of it must be one PM can read: an EXDATE it can't would be
/// dropped, and the occurrence it deleted would come back.
fn every_line_readable(master: &Master) -> Result<(), NotPlanned> {
    if lines::parse(&master.recurrence)
        .iter()
        .any(|l| l.kind == Kind::Unreadable)
    {
        return Err(RecurError::TextOnly(TextOnly::Unreadable).into());
    }
    Ok(())
}

/// The occurrence's own copy as an exception of the series: changed on its own, not cancelled.
fn changed_copy<'a>(master: &Master, input: &SeriesInput<'a>, at: Point) -> Option<&'a Value> {
    own_exceptions(master, input.exceptions)
        .find(|item| original_of(item) == Some(at) && !is_gone(item))
}

/// The first occurrence still there: the series' first, or the rule's first after it that isn't
/// deleted, up to `original` (the one the user opened, which is there) (C11 critique 1 H1).
fn first_live(master: &Master, original: Point, deleted: &[Point]) -> Result<Point, NotPlanned> {
    let kept = |p: &Point| !deleted.contains(p);
    if kept(&master.first) {
        return Ok(master.first);
    }
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    first_kept(&facts, &master.start, original, kept)?
        .ok_or_else(|| SeriesRefusal::NothingToKeep.into())
}

/// "All events" with a new rule, or with none (C11 §9.4): the master rebuilt from it, one PATCH.
pub(super) fn plan_all_rule(
    input: &SeriesInput,
    master: &Master,
    original: Point,
    draft: &EventPatchDraft,
    ask: RuleAsk,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    input.opened.master.ok_or(SeriesRefusal::NoBase)?;
    no_guests_from(master, input, None)?;
    keeps_formatting(master_json, draft)?;
    every_line_readable(master)?;
    // The draft's time is read against the occurrence as Google holds it now, and a new rule moves
    // the times whatever the draft says: it must still be where the user saw it (R2).
    unchanged_since_opened(master, input, &["start", "end"])?;
    let text = EventPatchDraft {
        time: None,
        recurrence: None,
        ..draft.clone()
    };
    let mut body = build_patch(master_json, &text)?;
    let zone = master.start.zone();
    let asked = asked_time(draft, instance)?;
    let (wall, length, end_zone) = match &asked {
        Some(a) => (a.start, a.length, a.end_zone),
        None => (master.start, master.length, master.end_zone),
    };
    // A move of this occurrence to another day moves every occurrence as far, as a move of the
    // series does (C11 critique 2 M4), measured from where it first was, in the days of the series
    // it moves into ([`Days`], C11c review).
    let out_of_range = || NotPlanned::from(SeriesRefusal::Unreadable);
    let days = Days::new(&master.start, &wall, original);
    let from = days.of(original).ok_or_else(out_of_range)?;
    let shift = asked
        .as_ref()
        .map_or(0, |a| (a.start.date() - from).num_days());
    let first_day = days.of(master.first).ok_or_else(out_of_range)?;
    let deleted = deleted_points(master, input)?;
    let mut warnings = Vec::new();
    let undo = Some(Undo::RestoreMaster {
        calendar: input.calendar.to_string(),
        event: master.id.clone(),
        body: snapshot(master_json, &["start", "end", "recurrence"]),
    });
    match ask {
        RuleAsk::New(spec) => {
            let asked_day = moved_day(from, shift).ok_or_else(out_of_range)?;
            let shifted = moved_day(first_day, shift).ok_or_else(out_of_range)?;
            let day = match spec.pattern {
                // From the first day, or from where the move puts it when that's earlier.
                Pattern::Yearly(YearlyOn::Date) => yearly_anchor(first_day.min(shifted), asked_day),
                _ => shifted,
            };
            let extra: Vec<String> = lines::parse(&master.recurrence)
                .into_iter()
                .filter(|l| matches!(l.kind, Kind::RDate(_)))
                .map(|l| l.raw)
                .collect();
            let rebuilt = rebuild(&RebuildAsk {
                spec: &spec,
                start: on_day(&wall, day),
                length,
                end_zone,
                deleted: &deleted,
                old_start: &master.start,
                old_rule: lines::series_rule(&lines::parse(&master.recurrence))
                    .ok()
                    .map(|(_, facts)| facts),
                shift,
                days,
                opened_at: original,
                opened: asked_day,
                extra: &extra,
            })?;
            clear_of_later(input.later, &rebuilt, zone)?;
            let (start, end) = nodes_for_master(
                master,
                rebuilt.start.zone(),
                end_zone,
                rebuilt.first,
                length,
            )?;
            put_times(&mut body, master_json, start, end);
            let same = rebuilt.start == master.start
                && lines::same_recurrence(&rebuilt.lines, &master.recurrence, &master.start);
            if !same {
                body.insert("recurrence".into(), Value::from(rebuilt.lines.clone()));
            }
            // A day moves only when it moves both ways of reading it: in the new series' days, as
            // `shift` counts it (a move into another zone at the same moment moves none), and on
            // the series' own wall, where the dialog reads its dates (the same time on the same
            // day in another zone moves none either, though the zones' days differ at that
            // moment; C11d review 4). Worded in the series' own wall days. A new rule that names
            // no day (daily) has none to move: where it starts says it all.
            let own_from = original.day_in(zone);
            if shift != 0
                && asked_day != own_from
                && day_phrase(&rebuilt.lines, &rebuilt.start).is_some()
            {
                warnings.push(SeriesWarning::RepeatDayChanges {
                    from: own_from,
                    to: asked_day,
                });
            }
            let own_first = master.first.day_in(zone);
            if rebuilt.start.date() != first_day && rebuilt.start.date() != own_first {
                warnings.push(SeriesWarning::StartMoved {
                    from: own_first,
                    to: rebuilt.start.date(),
                });
            }
            if length != master.length {
                warnings.push(SeriesWarning::LengthChanges);
            }
            if body.is_empty() {
                return Err(NotPlanned::NoChange);
            }
            all_plan(input, master, body, warnings, undo, notify, rebuilt.start)
        }
        RuleAsk::Stop => {
            // The first occurrence still there stays (Q2), never a deleted one, with its own changes
            // under the draft's (C11 critique 1 H1).
            let kept = first_live(master, original, &deleted)?;
            let mut own_time = None;
            if let Some(item) = changed_copy(master, input, kept) {
                if let InstanceKind::Modified { carry, other } =
                    classify_instance(master_json, master, item)
                {
                    // The series' PATCH can carry its text and times, never the rest.
                    if !other.is_empty()
                        || carry.iter().any(|f| matches!(*f, "colorId" | "reminders"))
                    {
                        return Err(SeriesRefusal::KeptHasOwn(kept.day_in(zone)).into());
                    }
                    let draft_keys: Vec<&str> = draft_fields(draft, master_json)
                        .iter()
                        .map(|(k, _)| *k)
                        .collect();
                    for field in carry {
                        if field == "time" {
                            own_time = Some(item);
                        } else if !draft_keys.contains(&field) {
                            let mine = item.get(field).cloned().unwrap_or(match field {
                                "summary" | "description" | "location" => Value::from(""),
                                _ => Value::Null,
                            });
                            body.insert(field.to_string(), mine);
                        }
                    }
                }
            }
            stop_body(StopAsk {
                input,
                master,
                body,
                kept,
                own_time,
                asked: asked.as_ref(),
                shift,
                days,
                warnings,
                undo,
                notify,
            })
        }
    }
}

/// `start` and `end` into `body` where they differ from the series' own.
fn put_times(body: &mut Map<String, Value>, master_json: &Value, start: Value, end: Value) {
    if !when_eq(master_json.get("start"), Some(&start)) {
        body.insert("start".into(), start);
    }
    if !when_eq(master_json.get("end"), Some(&end)) {
        body.insert("end".into(), end);
    }
}

/// What the "Does not repeat" PATCH of every occurrence is made from.
struct StopAsk<'a> {
    input: &'a SeriesInput<'a>,
    master: &'a Master,
    /// Its text so far: the draft's, and the kept occurrence's own under it.
    body: Map<String, Value>,
    /// The occurrence that stays, where it sits in the series.
    kept: Point,
    /// Its own copy, when it has a time of its own.
    own_time: Option<&'a Value>,
    asked: Option<&'a Asked>,
    shift: i64,
    /// The days `shift` is counted in ([`Days`]).
    days: Days,
    warnings: Vec<SeriesWarning>,
    undo: Option<Undo>,
    notify: Notify,
}

/// The master as one event, at the kept occurrence: at the draft's time, moved as far as the draft
/// moved this occurrence; else at its own time; else where the series puts it. No rule is sent
/// (`null`; probe P-R2 says whether Google wants `[]`, and the reply is read either way).
fn stop_body(ask: StopAsk) -> Result<SeriesPlan, NotPlanned> {
    let StopAsk {
        input,
        master,
        mut body,
        kept,
        own_time,
        asked,
        shift,
        days,
        mut warnings,
        undo,
        notify,
    } = ask;
    let master_json = input.fresh.master.json();
    let zone = master.start.zone();
    // The kept occurrence's day, read as the draft's: in another zone, the same place in the week.
    let kept_day = days.of(kept).ok_or(SeriesRefusal::Unreadable)?;
    let (first, start_zone, end_zone, length) = match (asked, own_time) {
        (Some(a), _) => {
            let day = moved_day(kept_day, shift).ok_or(SeriesRefusal::Unreadable)?;
            let start = on_day(&a.start, day);
            (one_event_at(&start)?, start.zone(), a.end_zone, a.length)
        }
        (None, Some(item)) => own_nodes(item, master)?,
        (None, None) => (kept, zone, master.end_zone, master.length),
    };
    let (start, end) = nodes_for_master(master, start_zone, end_zone, first, length)?;
    let rule_start = start_of(&start).unwrap_or(master.start);
    put_times(&mut body, master_json, start, end);
    body.insert("recurrence".into(), Value::Null);
    warnings.push(SeriesWarning::BecomesOneEvent {
        day: rule_start.date(),
    });
    // The draft moved this occurrence to another day, so the one kept moves as far: no other line
    // says it leaves its own day (`shift` is the draft's, 0 without one). As for a new rule, a day
    // moves only when it moves on the series' own wall too, where the dialog reads its dates, and
    // it's worded in those days (C11d review 5).
    let own_kept = kept.day_in(zone);
    if shift != 0 && rule_start.date() != own_kept {
        warnings.push(SeriesWarning::StartMoved {
            from: own_kept,
            to: rule_start.date(),
        });
    }
    let mut plan = all_plan(input, master, body, warnings, undo, notify, rule_start)?;
    // Every other occurrence goes, as its own words say; but what Google does with the ones changed
    // on their own is P-R2's to show, and their landing keeps any it still shows, so the hedge stays
    // for them. Never for the one kept: this PATCH carries its changes.
    let others = own_exceptions(master, input.exceptions)
        .filter(|item| original_of(item) != Some(kept))
        .filter(|item| {
            matches!(
                classify_instance(master_json, master, item),
                InstanceKind::Modified { .. }
            )
        })
        .count();
    plan.warnings
        .retain(|w| !matches!(w, SeriesWarning::ExceptionsMayReset { .. }));
    if others > 0 {
        plan.warnings.push(SeriesWarning::ExceptionsMayReset {
            count: u32::try_from(others).unwrap_or(u32::MAX),
            complete: input.exceptions.complete,
        });
    }
    Ok(plan)
}

/// "This and following" with a new rule, or with none (C11 §9.5): a new part rebuilt from it from
/// this occurrence on, or one event in its place, inserted first, then the original cut, then each
/// occurrence changed on its own carried where the new rule still has its day, and cancelled on the
/// original.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_split_rule(
    input: &SeriesInput,
    master: &Master,
    at: Point,
    SplitCut { cut, moved, before }: SplitCut,
    draft: &EventPatchDraft,
    tail_id: &str,
    ask: RuleAsk,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    let opened = input.opened.master.ok_or(SeriesRefusal::NoBase)?;
    keeps_formatting(master_json, draft)?;
    // An id PM minted for this plan, never the series' own (C8b review ADV-5).
    if !is_plain_segment(tail_id) || tail_id == master.id {
        return Err(SeriesRefusal::Unreadable.into());
    }
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    let zone = master.start.zone();
    let rest = rest_start(master, at)?;
    let exceptions = exceptions_from(master, input.exceptions, at)?;
    no_guests_from(master, input, Some(at))?;
    if draft.time.is_some() {
        // Read against the occurrence as Google holds it now: it must still be where the user saw it
        // (R2; C8b reviews ADV-1).
        unchanged_since_opened(master, input, &["start", "end"])?;
    }
    let asked = asked_time(draft, instance)?;
    // The new part starts where the draft put this occurrence, else where the series has it.
    let (start, length, end_zone) = match &asked {
        Some(a) => (a.start, a.length, a.end_zone),
        None => (rest, master.length, master.end_zone),
    };
    let at_day = at.day_in(zone);
    // In the days of the part the rest moves into, as for every occurrence ([`Days`], C11c review).
    let days = Days::new(&master.start, &start, at);
    let shift = (start.date() - days.of(at).ok_or(SeriesRefusal::Unreadable)?).num_days();
    // Deleted from here on: its cancelled occurrences, and the EXDATEs the cut hands the rest; its
    // extra dates are the rest's too.
    let mut deleted: Vec<Point> = exceptions
        .iter()
        .filter(|(item, _)| classify_instance(master_json, master, item) == InstanceKind::Cancelled)
        .map(|(_, p)| *p)
        .collect();
    let mut extra = Vec::new();
    for line in &moved {
        match &line.kind {
            Kind::ExDate(dates) => {
                for value in &dates.values {
                    deleted.push(
                        dates
                            .point(value, &master.start)
                            .ok_or(RecurError::CannotCount)?,
                    );
                }
            }
            Kind::RDate(_) => extra.push(line.raw.clone()),
            _ => {}
        }
    }
    let touched = draft_fields(draft, master_json);
    let draft_keys: Vec<&str> = touched.iter().map(|(k, _)| *k).collect();
    let mut warnings = Vec::new();
    let mut lost: u32 = 0;
    // The new part's body and kind, and where each occurrence changed on its own lands in it.
    let (body, all_day, landing) = match ask {
        RuleAsk::New(spec) => {
            // An "after n times" still as the series has it is the series' own count, rebased on what
            // is left of it; only a count the user set is written as set (C11 critique 1 M4, Q3).
            let seed = spec_of(&master.recurrence, &master.start).map(|s| s.ends);
            let spec = match (spec.ends, seed, facts.count, before) {
                (Ends::After(n), Some(Ends::After(m)), Some(count), Some(k)) if n == m => {
                    let left = u16::try_from(remaining_count(count, k)?)
                        .map_err(|_| RecurError::CannotCount)?;
                    RecurrenceSpec {
                        ends: Ends::After(left),
                        ..spec
                    }
                }
                _ => spec,
            };
            let rebuilt = rebuild(&RebuildAsk {
                spec: &spec,
                start,
                length,
                end_zone,
                deleted: &deleted,
                old_start: &master.start,
                old_rule: Some(facts.clone()),
                shift,
                days,
                opened_at: at,
                opened: start.date(),
                extra: &extra,
            })?;
            // The rest can't start where the part before it still reaches (A29): both days read in
            // the series' own zone, whatever zone the rest is moved into, so an equal day counts (an
            // all-day series has no zone of its own: the rest's own date).
            let head = last_kept(
                master,
                &facts,
                at,
                &cancelled_points(master, input.exceptions)?,
            )?;
            let first_day = match master.start {
                SeriesStart::Timed { .. } => rebuilt.first.day_in(zone),
                SeriesStart::AllDay(_) => rebuilt.start.date(),
            };
            if head.is_some_and(|h| first_day <= h.day_in(zone)) {
                return Err(RetimeError::Overlaps(rebuilt.start.date()).into());
            }
            clear_of_later(input.later, &rebuilt, zone)?;
            if rebuilt.moved {
                warnings.push(SeriesWarning::StartMoved {
                    from: at_day,
                    to: rebuilt.start.date(),
                });
            }
            let all_day = matches!(rebuilt.start, SeriesStart::AllDay(_));
            let body = tail_body(
                master,
                master_json,
                at,
                &touched,
                rebuilt.nodes.clone(),
                Some(rebuilt.lines.clone()),
            );
            (body, all_day, Some(rebuilt))
        }
        RuleAsk::Stop => {
            // One event in place of the rest: this occurrence, its own changes travelling with it
            // under the draft's (C11 §9.5.7).
            let own = exceptions.iter().find(|(_, p)| *p == at).map(|(i, _)| *i);
            let mut fields: Vec<(&str, Value)> = Vec::new();
            let mut own_time = None;
            if let Some(item) = own {
                match classify_instance(master_json, master, item) {
                    InstanceKind::Modified { other, .. } if !other.is_empty() => {
                        return Err(SeriesRefusal::CannotCarry(at_day).into());
                    }
                    InstanceKind::Modified { carry, .. } => {
                        for field in carry {
                            if field == "time" {
                                own_time = Some(item);
                            } else if !draft_keys.contains(&field) {
                                let value = item.get(field).cloned().unwrap_or(match field {
                                    "summary" | "description" | "location" => Value::from(""),
                                    _ => Value::Null,
                                });
                                fields.push((field, value));
                            }
                        }
                    }
                    InstanceKind::Plain | InstanceKind::Cancelled => {}
                }
            }
            fields.extend(touched.iter().cloned());
            let (first, start_zone, end_zone, length) = match (&asked, own_time) {
                (Some(a), _) => (
                    one_event_at(&a.start)?,
                    a.start.zone(),
                    a.end_zone,
                    a.length,
                ),
                (None, Some(item)) => own_nodes(item, master)?,
                (None, None) => (at, zone, master.end_zone, master.length),
            };
            let nodes = when_nodes_in(start_zone, end_zone, first, length)?;
            let one_day = first.day_in(start_zone);
            warnings.push(SeriesWarning::BecomesOneEvent { day: one_day });
            let all_day = matches!(first, Point::Day(_));
            (
                tail_body(master, master_json, at, &fields, nodes, None),
                all_day,
                None,
            )
        }
    };
    let stops = landing.is_none();
    split_unchanged_since_opened(master, input, opened, &touched)?;
    let mut steps = vec![
        insert_step(input, master, tail_id, body, notify),
        cut_step(master, input.calendar, master_json, cut, TAIL_WATCH, notify),
    ];
    // Each stored exception from here on: carried onto the new part's same occurrence where the new
    // rule still has its day (Q3), then cancelled on the original, only once the carry is done (DS3).
    // Changes on a day the new rule skips go, and are counted; a part that doesn't repeat carries
    // none, and every later change goes.
    for (item, p) in &exceptions {
        let kind = classify_instance(master_json, master, item);
        if kind == InstanceKind::Cancelled {
            continue;
        }
        let split_itself = *p == at;
        let requires = match &landing {
            // One event in place of the rest: every later occurrence goes, changed or not, as its
            // own words say, so none is counted apart.
            None => None,
            Some(rebuilt) => {
                let carry = match kind {
                    InstanceKind::Modified { other, .. } if !other.is_empty() => {
                        return Err(SeriesRefusal::CannotCarry(p.day_in(zone)).into());
                    }
                    InstanceKind::Modified { carry, .. } => carry,
                    InstanceKind::Plain | InstanceKind::Cancelled => Vec::new(),
                };
                let moved_time = split_itself && asked.is_some();
                let carried =
                    carried_fields(item, &carry, split_itself, &draft_keys, moved_time, all_day);
                // An extra date kept is still an occurrence where it was (RM1); the occurrence split
                // at goes where the draft put it; any later one where the new rule still has its own
                // day, else where the day move takes it, never onto the one split at ([`Landing`]).
                let target = if rebuilt.extra.contains(p) {
                    Some(*p)
                } else if split_itself {
                    rebuilt.opened
                } else {
                    Landing {
                        facts: &rebuilt.facts,
                        start: &rebuilt.start,
                        old: Some((&facts, &master.start)),
                        shift,
                        days: rebuilt.days,
                        opened: rebuilt.opened,
                    }
                    .at(*p)?
                };
                match (carried.is_empty(), target) {
                    (true, _) => None,
                    (false, None) => {
                        lost = lost.saturating_add(1);
                        None
                    }
                    (false, Some(target)) => {
                        steps.push(Step {
                            op: Op::Carry {
                                calendar: input.calendar.to_string(),
                                series: tail_id.to_string(),
                                original: target,
                                body: carried.clone(),
                            },
                            notify,
                            stage: Stage::Cleanup,
                            requires: None,
                            conflict: Recheck::Never,
                            shows: Shows::Fields {
                                sent: carried,
                                old: Map::new(),
                            },
                            gone_is_done: false,
                            undo: None,
                        });
                        Some(steps.len() - 1)
                    }
                }
            }
        };
        steps.push(cancel_exception(
            input.calendar,
            item,
            *p,
            notify,
            requires,
        )?);
    }
    if lost > 0 {
        warnings.push(SeriesWarning::ChangesLost { count: lost });
    }
    if !stops {
        if length != master.length {
            warnings.push(SeriesWarning::LengthChanges);
        }
        warnings.extend(not_carried(master_json));
        warnings.push(SeriesWarning::NewIdentity);
    }
    if input.sibling {
        warnings.push(SeriesWarning::SplitSibling);
    }
    let mut expect = vec![Expect::EndsBefore {
        series: master.id.clone(),
        at,
        left: Vec::new(),
    }];
    expect.push(if stops {
        Expect::Event {
            id: tail_id.to_string(),
        }
    } else {
        Expect::Present {
            series: tail_id.to_string(),
        }
    });
    Ok(SeriesPlan {
        scope: Scope::Following,
        steps,
        warnings,
        expect,
        removes: Removes::From {
            series: master.id.clone(),
            at,
            left: Vec::new(),
        },
        start: master.start,
        rule_start: master.start,
        guests: None,
        chosen: None,
    })
}

/// What an occurrence changed on its own carries onto the new part's same occurrence: its `carry`
/// fields, but on the occurrence split at the draft's own fields (and its time, when the draft moved
/// it: `moved_time`). Its time between timed and all-day goes with the other kind's keys as `null`,
/// as Google merges nodes (C8b review G4; C11 critique 1 L9: carried across kinds as before).
fn carried_fields(
    item: &Value,
    carry: &[&'static str],
    split_itself: bool,
    draft_keys: &[&str],
    moved_time: bool,
    all_day: bool,
) -> Map<String, Value> {
    let mut carried = Map::new();
    for field in carry {
        if split_itself && draft_keys.contains(field) {
            continue;
        }
        match *field {
            "time" => {
                if moved_time {
                    continue;
                }
                for k in ["start", "end"] {
                    let mut node = item.get(k).cloned().unwrap_or(Value::Null);
                    if node.get("date").is_some() != all_day {
                        drop_other_kind(&mut node);
                    }
                    carried.insert(k.to_string(), node);
                }
            }
            _ => {
                let value = item.get(*field).cloned().unwrap_or(match *field {
                    "summary" | "description" | "location" => Value::from(""),
                    _ => Value::Null,
                });
                carried.insert((*field).to_string(), value);
            }
        }
    }
    carried
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::super::{Exceptions, Listing};
    use super::*;
    use crate::calendar_recur::lines::Applied;
    use crate::calendar_recur::wire::{DayCode, NthCode, RepeatEnds, RepeatPattern, RepeatRule};
    use chrono::NaiveDateTime;
    use chrono_tz::Europe::London;
    use serde_json::json;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn wall(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        day(y, m, d).and_hms_opt(h, min, 0).unwrap()
    }

    fn london(y: i32, m: u32, d: u32, h: u32) -> SeriesStart {
        SeriesStart::Timed {
            local: wall(y, m, d, h, 0),
            zone: London,
        }
    }

    fn rule(pattern: RepeatPattern, interval: u16, ends: RepeatEnds) -> RepeatRule {
        RepeatRule {
            pattern,
            interval,
            ends,
        }
    }

    fn weekly_on(days: &[DayCode]) -> RepeatRule {
        rule(
            RepeatPattern::Weekly {
                days: days.to_vec(),
            },
            1,
            RepeatEnds::Never,
        )
    }

    fn daily() -> RepeatRule {
        rule(RepeatPattern::Daily, 1, RepeatEnds::Never)
    }

    /// A draft asking for `rule` from now on.
    fn repeat(rule: RepeatRule) -> EventPatchDraft {
        EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule { rule }),
            ..Default::default()
        }
    }

    /// "Does not repeat".
    fn stop() -> EventPatchDraft {
        EventPatchDraft {
            recurrence: Some(RepeatDraft::Once),
            ..Default::default()
        }
    }

    fn body(plan: &SeriesPlan, step: usize) -> &Map<String, Value> {
        match &plan.steps[step].op {
            Op::Insert { body, .. } | Op::Patch { body, .. } | Op::Carry { body, .. } => body,
            _ => panic!("step {step} has no body"),
        }
    }

    fn timed_node(at: &str) -> Value {
        json!({ "dateTime": at, "timeZone": "Europe/London" })
    }

    fn spec_of_rule(rule: &RepeatRule) -> RecurrenceSpec {
        rule.to_spec().unwrap()
    }

    /// The old series every rebuild here replaces: weekly on Monday from 05-10 09:00 in London.
    fn old() -> SeriesStart {
        london(2026, 10, 5, 9)
    }

    /// `ask` for `spec` from `start` an hour long, the old series starting at `old` (weekly on
    /// Monday), with `deleted`, no extra dates and no shift, the occurrence opened its first, put on
    /// the new first day.
    fn ask<'a>(
        spec: &'a RecurrenceSpec,
        start: SeriesStart,
        deleted: &'a [Point],
        old: &'a SeriesStart,
    ) -> RebuildAsk<'a> {
        let mondays = lines::parse(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        let first = old.point().unwrap();
        RebuildAsk {
            spec,
            start,
            length: Length::Timed(TimeDelta::hours(1)),
            end_zone: London,
            deleted,
            old_start: old,
            old_rule: lines::series_rule(&mondays).ok().map(|(_, facts)| facts),
            shift: 0,
            days: Days::new(old, &start, first),
            opened_at: first,
            opened: start.date(),
            extra: &[],
        }
    }

    // --- rebuilding a series from a rule ---

    /// A deleted Monday stays deleted under a daily rule, which still makes it.
    #[test]
    fn rebuild_keeps_deleted_days_the_new_rule_still_has() {
        let (spec, old) = (spec_of_rule(&daily()), old());
        let deleted = [point("2026-10-19T08:00:00Z")];
        let rebuilt = rebuild(&ask(&spec, london(2026, 10, 5, 9), &deleted, &old)).unwrap();
        assert_eq!(
            rebuilt.lines,
            ["RRULE:FREQ=DAILY", "EXDATE:20261019T080000Z"]
        );
        assert!(!rebuilt.moved);
    }

    /// An EXDATE the new rule doesn't make is never written: Google refuses one that names nothing.
    #[test]
    fn rebuild_drops_exdates_off_the_new_rule() {
        let (spec, old) = (spec_of_rule(&weekly_on(&[DayCode::Tue])), old());
        let deleted = [point("2026-10-19T08:00:00Z")];
        let rebuilt = rebuild(&ask(&spec, london(2026, 10, 5, 9), &deleted, &old)).unwrap();
        assert_eq!(rebuilt.lines, ["RRULE:FREQ=WEEKLY;BYDAY=TU"]);
        assert_eq!(rebuilt.start, london(2026, 10, 6, 9));
        assert!(rebuilt.moved);
    }

    /// At a new time, a deleted day stays deleted at that time.
    #[test]
    fn rebuild_moves_exdates_to_the_new_time() {
        let (spec, old) = (spec_of_rule(&daily()), old());
        let deleted = [point("2026-10-19T08:00:00Z"), point("2026-11-02T09:00:00Z")];
        let rebuilt = rebuild(&ask(&spec, london(2026, 10, 5, 10), &deleted, &old)).unwrap();
        assert_eq!(
            rebuilt.lines,
            [
                "RRULE:FREQ=DAILY",
                "EXDATE:20261019T090000Z,20261102T100000Z"
            ]
        );
        // C11c review: a day the change moves every occurrence by leaves a deleted day the new rule
        // still makes where it was: the user deleted that Monday, not the Tuesday after it.
        let shifted = RebuildAsk {
            shift: 1,
            ..ask(&spec, london(2026, 10, 6, 9), &deleted, &old)
        };
        assert_eq!(
            rebuild(&shifted).unwrap().lines,
            [
                "RRULE:FREQ=DAILY",
                "EXDATE:20261019T080000Z,20261102T090000Z"
            ]
        );
        // A rule without the day moves the deletion with the day: Mondays made Tuesdays.
        let tuesdays = spec_of_rule(&weekly_on(&[DayCode::Tue]));
        let moved_on = RebuildAsk {
            shift: 1,
            ..ask(&tuesdays, london(2026, 10, 6, 9), &deleted, &old)
        };
        assert_eq!(
            rebuild(&moved_on).unwrap().lines,
            [
                "RRULE:FREQ=WEEKLY;BYDAY=TU",
                "EXDATE:20261020T080000Z,20261103T090000Z"
            ]
        );
        // Never onto a day the old series had an occurrence of its own: which one was deleted would be
        // a guess. Mondays moved two days on, made Wednesdays, from a series with Wednesdays too.
        let both = lines::parse(&["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"]);
        let wednesdays = spec_of_rule(&weekly_on(&[DayCode::Wed]));
        let guessed = RebuildAsk {
            shift: 2,
            old_rule: lines::series_rule(&both).ok().map(|(_, facts)| facts),
            ..ask(&wednesdays, london(2026, 10, 7, 9), &deleted, &old)
        };
        assert_eq!(
            rebuild(&guessed).unwrap().lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=WE"]
        );
        // When PM can't read the old rule, it can't tell whether the day moved to had one of its own:
        // refused in words where the new rule makes that day (C11c review), never dropped.
        let unread = RebuildAsk {
            shift: 1,
            old_rule: None,
            ..ask(&tuesdays, london(2026, 10, 6, 9), &deleted, &old)
        };
        assert!(matches!(
            rebuild(&unread),
            Err(NotPlanned::Refused(SeriesRefusal::DeletedDayUnread(d))) if d == day(2026, 10, 19)
        ));
        // Where it doesn't make the day moved to either, the deletion has nowhere to go.
        let thursdays = spec_of_rule(&weekly_on(&[DayCode::Thu]));
        let nowhere = RebuildAsk {
            shift: 1,
            old_rule: None,
            ..ask(&thursdays, london(2026, 10, 8, 9), &deleted, &old)
        };
        assert_eq!(
            rebuild(&nowhere).unwrap().lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=TH"]
        );
    }

    /// An extra date (RDATE) is kept as Google wrote it while the series' times stay; moved, it's
    /// refused as a move refuses it.
    #[test]
    fn rebuild_keeps_extra_dates_only_when_times_stay() {
        let (spec, old) = (
            spec_of_rule(&weekly_on(&[DayCode::Mon, DayCode::Wed])),
            old(),
        );
        let extra = ["RDATE;TZID=Europe/London:20261023T090000".to_string()];
        let kept = rebuild(&RebuildAsk {
            extra: &extra,
            ..ask(&spec, london(2026, 10, 5, 9), &[], &old)
        })
        .unwrap();
        assert_eq!(
            kept.lines,
            [
                "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
                "RDATE;TZID=Europe/London:20261023T090000"
            ]
        );
        for moved in [
            RebuildAsk {
                extra: &extra,
                ..ask(&spec, london(2026, 10, 5, 10), &[], &old)
            },
            RebuildAsk {
                extra: &extra,
                shift: 1,
                ..ask(&spec, london(2026, 10, 5, 9), &[], &old)
            },
            RebuildAsk {
                extra: &extra,
                length: Length::AllDay(1),
                ..ask(&spec, SeriesStart::AllDay(day(2026, 10, 5)), &[], &old)
            },
        ] {
            assert!(matches!(
                rebuild(&moved),
                Err(NotPlanned::Refused(SeriesRefusal::Retime(
                    RetimeError::HasExtraDates
                )))
            ));
        }
    }

    /// RM1: a deleted extra date kept as Google wrote it is still an occurrence where it was, so it
    /// stays deleted there, whatever the new rule makes; and only there, never at the rule's time.
    #[test]
    fn rebuild_keeps_a_deleted_extra_date_deleted() {
        let old = old();
        // Friday 23-10 at the series' time, Friday 30-10 (London on GMT again) at 14:00.
        let extra = ["RDATE;TZID=Europe/London:20261023T090000,20261030T140000".to_string()];
        let deleted = [point("2026-10-23T08:00:00Z"), point("2026-10-30T14:00:00Z")];
        for (rule, first) in [
            (
                weekly_on(&[DayCode::Mon, DayCode::Wed]),
                "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
            ),
            (daily(), "RRULE:FREQ=DAILY"),
        ] {
            let spec = spec_of_rule(&rule);
            let rebuilt = rebuild(&RebuildAsk {
                extra: &extra,
                ..ask(&spec, london(2026, 10, 5, 9), &deleted, &old)
            })
            .unwrap();
            assert_eq!(
                rebuilt.lines,
                [
                    first,
                    "EXDATE:20261023T080000Z,20261030T140000Z",
                    "RDATE;TZID=Europe/London:20261023T090000,20261030T140000"
                ]
            );
        }
    }

    /// A first day whose time the clocks skip is refused, never moved to another hour.
    #[test]
    fn rebuild_refuses_a_gap_start() {
        // London skips 01:00-02:00 on Sunday 29-03-2026; "Weekly on Sunday" asked on the Saturday
        // starts that Sunday.
        let (spec, old) = (spec_of_rule(&weekly_on(&[DayCode::Sun])), old());
        let start = SeriesStart::Timed {
            local: wall(2026, 3, 28, 1, 30),
            zone: London,
        };
        assert!(matches!(
            rebuild(&ask(&spec, start, &[], &old)),
            Err(NotPlanned::Refused(SeriesRefusal::Spec(SpecError::Gap(d)))) if d == day(2026, 3, 29)
        ));
    }

    /// A timed start names its zone on both nodes (G6), its end in its own.
    #[test]
    fn rebuild_names_the_zone_on_a_timed_start() {
        let (spec, old) = (spec_of_rule(&daily()), old());
        let rebuilt = rebuild(&RebuildAsk {
            end_zone: chrono_tz::America::New_York,
            length: Length::Timed(TimeDelta::hours(8)),
            ..ask(&spec, london(2026, 10, 5, 9), &[], &old)
        })
        .unwrap();
        assert_eq!(rebuilt.nodes.0, timed_node("2026-10-05T09:00:00+01:00"));
        assert_eq!(
            rebuilt.nodes.1,
            json!({ "dateTime": "2026-10-05T12:00:00-04:00", "timeZone": "America/New_York" })
        );
    }

    /// An all-day one is written as dates, its UNTIL a date.
    #[test]
    fn rebuild_writes_dates_for_an_all_day_one() {
        let spec = spec_of_rule(&rule(
            RepeatPattern::Daily,
            1,
            RepeatEnds::OnDate {
                date: "2026-12-31".into(),
            },
        ));
        let (deleted, old) = ([point("2026-10-19T08:00:00Z")], old());
        let rebuilt = rebuild(&RebuildAsk {
            length: Length::AllDay(2),
            end_zone: Tz::UTC,
            ..ask(&spec, SeriesStart::AllDay(day(2026, 10, 5)), &deleted, &old)
        })
        .unwrap();
        assert_eq!(
            rebuilt.nodes,
            (
                json!({ "date": "2026-10-05" }),
                json!({ "date": "2026-10-07" })
            )
        );
        assert_eq!(
            rebuilt.lines,
            [
                "RRULE:FREQ=DAILY;UNTIL=20261231",
                "EXDATE;VALUE=DATE:20261019"
            ]
        );
    }

    /// "Annually on its own date" starts on the first such date on or after the series' start.
    #[test]
    fn yearly_on_a_date_anchors_at_the_occurrences_date() {
        assert_eq!(
            yearly_anchor(day(2026, 10, 5), day(2026, 10, 13)),
            day(2026, 10, 13)
        );
        assert_eq!(
            yearly_anchor(day(2026, 10, 13), day(2026, 10, 5)),
            day(2027, 10, 5)
        );
        assert_eq!(
            yearly_anchor(day(2025, 1, 1), day(2024, 2, 29)),
            day(2028, 2, 29)
        );
        assert_eq!(
            yearly_anchor(day(2096, 3, 1), day(2096, 2, 29)),
            day(2104, 2, 29)
        );
        // Through a plan: picked on a later occurrence, it repeats on that date.
        let case = Case::new(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]),
            plain("2026-10-12T09:00:00+01:00"),
        );
        let yearly = rule(RepeatPattern::YearlyDate, 1, RepeatEnds::Never);
        let plan = case.edit(Scope::All, repeat(yearly.clone())).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            timed_node("2026-10-12T09:00:00+01:00")
        );
        assert_eq!(body(&plan, 0)["recurrence"], json!(["RRULE:FREQ=YEARLY"]));
        // The first occurrence moved two days earlier, to Saturday 03-10: it repeats from there,
        // never from a year on.
        let first = Case::new(weekly(), plain("2026-10-05T09:00:00+01:00"));
        let earlier = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule { rule: yearly }),
            ..at("09:00", "2026-10-03")
        };
        let plan = first.edit(Scope::All, earlier).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            timed_node("2026-10-03T09:00:00+01:00")
        );
        assert_eq!(body(&plan, 0)["recurrence"], json!(["RRULE:FREQ=YEARLY"]));
        assert!(plan.warnings.contains(&SeriesWarning::StartMoved {
            from: day(2026, 10, 5),
            to: day(2026, 10, 3)
        }));
    }

    const PROBE: &str = "PM_RULE_ZONE_PROBE";
    const PROBE_LINE: &str = "rule-probe: ";

    /// Rebuilds whose every value is built with an explicit zone.
    fn probe() -> String {
        let deleted = [point("2026-10-19T08:00:00Z"), point("2026-11-02T09:00:00Z")];
        let weekdays = spec_of_rule(&weekly_on(&[DayCode::Mon, DayCode::Wed]));
        let monthly = spec_of_rule(&rule(
            RepeatPattern::MonthlyDay { day: 31 },
            1,
            RepeatEnds::After { count: 5 },
        ));
        let (all_day, old) = (spec_of_rule(&daily()), old());
        let out: Vec<String> = [
            rebuild(&ask(&weekdays, london(2026, 10, 13, 23), &deleted, &old)),
            rebuild(&ask(&monthly, london(2026, 10, 5, 0), &deleted, &old)),
            rebuild(&RebuildAsk {
                length: Length::AllDay(1),
                end_zone: Tz::UTC,
                ..ask(
                    &all_day,
                    SeriesStart::AllDay(day(2026, 10, 5)),
                    &deleted,
                    &old,
                )
            }),
        ]
        .into_iter()
        .map(|r| match r {
            Ok(r) => format!("{:?} {:?} {:?}", r.start, r.nodes, r.lines),
            Err(_) => "refused".into(),
        })
        .collect();
        out.join(" | ")
    }

    /// The child half of the next test.
    #[test]
    fn rule_zone_probe() {
        if std::env::var_os(PROBE).is_some() {
            println!("{PROBE_LINE}{}", probe());
        }
    }

    /// Trap (b): a rebuild run in a process set to Auckland or Los Angeles is the one run here.
    #[test]
    fn rebuild_is_the_same_under_another_computer_zone() {
        let here = probe();
        let exe = std::env::current_exe().unwrap();
        for zone in ["Pacific/Auckland", "America/Los_Angeles"] {
            let out = std::process::Command::new(&exe)
                .args([
                    "calendar_write::series::rule::tests::rule_zone_probe",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(PROBE, "1")
                .env("TZ", zone)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let line = stdout
                .lines()
                .find_map(|l| l.split_once(PROBE_LINE).map(|(_, probe)| probe))
                .unwrap_or_else(|| panic!("no probe output with TZ={zone}"));
            assert_eq!(line, here, "TZ={zone}");
        }
    }

    // --- "All events" with a new rule ---

    /// §9.10: weekly Monday from 05-10 made "Weekly on Tuesday" starts on Tuesday 06-10, and says so;
    /// one PATCH of the start, the end and the rule, undone only when half of it lands.
    #[test]
    fn all_with_a_new_rule_patches_start_end_and_rule_and_says_the_start_moved() {
        let case = Case::new(weekly(), monday_19());
        let plan = case
            .edit(Scope::All, repeat(weekly_on(&[DayCode::Tue])))
            .unwrap();
        assert_eq!(plan.scope, Scope::All);
        assert_eq!(plan.steps.len(), 1);
        let sent = body(&plan, 0);
        assert_eq!(sent["start"], timed_node("2026-10-06T09:00:00+01:00"));
        assert_eq!(sent["end"], timed_node("2026-10-06T10:00:00+01:00"));
        assert_eq!(sent["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]));
        assert_eq!(sent.len(), 3);
        assert_eq!(
            plan.warnings,
            [SeriesWarning::StartMoved {
                from: day(2026, 10, 5),
                to: day(2026, 10, 6)
            }]
        );
        assert!(matches!(
            &plan.steps[0].undo,
            Some(Undo::RestoreMaster { body, .. })
                if body["recurrence"] == json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"])
        ));
        assert_eq!(plan.rule_start, london(2026, 10, 6, 9));
        // The same rule asked again is no change of rule at all.
        assert_eq!(
            case.edit(Scope::All, repeat(weekly_on(&[DayCode::Mon]))),
            Err(NotPlanned::NoChange)
        );
    }

    /// A rule PM can only show is replaced whole by one from the editor's set.
    #[test]
    fn all_with_a_new_rule_replaces_a_custom_rule() {
        let custom = master(&["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"]);
        let case = Case::new(custom, plain("2026-10-13T09:00:00+01:00"));
        let plan = case.edit(Scope::All, repeat(daily())).unwrap();
        let sent = body(&plan, 0);
        assert_eq!(sent["recurrence"], json!(["RRULE:FREQ=DAILY"]));
        // Already on the daily rule's first day: the start stays.
        assert!(sent.get("start").is_none());
        // A line PM can't read could be an EXDATE, whose occurrence would come back.
        let unreadable = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Mars/Olympus:20261019T090000",
        ]);
        assert_eq!(
            Case::new(unreadable, monday_19()).edit(Scope::All, repeat(daily())),
            Err(NotPlanned::Refused(SeriesRefusal::Recur(
                RecurError::TextOnly(TextOnly::Unreadable)
            )))
        );
    }

    /// C11c review: the second Tuesday from 08-09, November's deleted, moved a day on and made "the
    /// second Wednesday". Spelt BYSETPOS, a rule PM can only show as text, PM can't tell whether
    /// Wednesday 11-11 had an occurrence of its own, so it refuses in words rather than bring
    /// November's back; spelt 2TU, the deletion moves to 11-11.
    #[test]
    fn a_moved_day_under_a_custom_rule_refuses_rather_than_bring_a_deletion_back() {
        let second_tuesday = |rule: &str| {
            master_with(
                &[rule, "EXDATE;TZID=Europe/London:20261110T090000"],
                "2026-09-08T09:00:00+01:00",
                "2026-09-08T10:00:00+01:00",
            )
        };
        let second_wednesday = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: rule(
                    RepeatPattern::MonthlyWeekday {
                        nth: NthCode::Second,
                        weekday: DayCode::Wed,
                    },
                    1,
                    RepeatEnds::Never,
                ),
            }),
            ..at("09:00", "2026-10-14")
        };
        let october = plain("2026-10-13T09:00:00+01:00");
        let custom = Case::new(
            second_tuesday("RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"),
            october.clone(),
        );
        assert_eq!(
            custom.edit(Scope::All, second_wednesday.clone()),
            Err(NotPlanned::Refused(SeriesRefusal::DeletedDayUnread(day(
                2026, 11, 10
            ))))
        );
        let read = Case::new(second_tuesday("RRULE:FREQ=MONTHLY;BYDAY=2TU"), october);
        let plan = read.edit(Scope::All, second_wednesday).unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=MONTHLY;BYDAY=2WE", "EXDATE:20261111T090000Z"])
        );
        // The refusal names the day deleted as the series has it, in its own zone (C11c review round
        // 3). At 23:30 in London, the 13-10 occurrence is 14-10 07:30 in Tokyo, moved a day on there
        // and made "Weekly on Thursday": November's, deleted on Tuesday 10-11, is Wednesday 11-11 in
        // Tokyo.
        let late = master_with(
            &[
                "RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2",
                "EXDATE;TZID=Europe/London:20261110T233000",
            ],
            "2026-09-08T23:30:00+01:00",
            "2026-09-09T00:30:00+01:00",
        );
        let thursdays = EventPatchDraft {
            time: Some(in_tokyo("2026-10-15", "07:30", "2026-10-15", "08:30")),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Thu]),
            }),
            ..Default::default()
        };
        assert_eq!(
            Case::new(late, plain("2026-10-13T23:30:00+01:00")).edit(Scope::All, thursdays),
            Err(NotPlanned::Refused(SeriesRefusal::DeletedDayUnread(day(
                2026, 11, 10
            ))))
        );
    }

    /// C11 critique 2 M4: a new rule with this occurrence moved to another day moves every
    /// occurrence as far, as Google allows, and asks about the days it repeats on.
    #[test]
    fn all_with_a_new_rule_and_a_moved_day_moves_the_series_by_as_much() {
        let case = Case::new(weekly(), monday_19());
        let to_tuesday = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Tue]),
            }),
            ..at("10:00", "2026-10-20")
        };
        let plan = case.edit(Scope::All, to_tuesday.clone()).unwrap();
        let sent = body(&plan, 0);
        assert_eq!(sent["start"], timed_node("2026-10-06T10:00:00+01:00"));
        assert_eq!(sent["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]));
        assert_eq!(
            plan.warnings,
            [
                SeriesWarning::RepeatDayChanges {
                    from: day(2026, 10, 19),
                    to: day(2026, 10, 20)
                },
                SeriesWarning::StartMoved {
                    from: day(2026, 10, 5),
                    to: day(2026, 10, 6)
                }
            ]
        );
        // A daily rule names no day: the series' first moves by as much, but a deleted Monday it still
        // makes stays deleted, never the Tuesday after it (C11c review).
        let with_exdate = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261026T090000",
        ]);
        let every_day = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule { rule: daily() }),
            ..at("09:00", "2026-10-20")
        };
        let plan = Case::new(with_exdate, monday_19())
            .edit(Scope::All, every_day)
            .unwrap();
        let sent = body(&plan, 0);
        assert_eq!(sent["start"], timed_node("2026-10-06T09:00:00+01:00"));
        assert_eq!(
            sent["recurrence"],
            json!(["RRULE:FREQ=DAILY", "EXDATE:20261026T090000Z"])
        );
        // No question of which days it repeats on, as if Mondays became Tuesdays: where it starts
        // says it all.
        assert_eq!(
            plan.warnings,
            [SeriesWarning::StartMoved {
                from: day(2026, 10, 5),
                to: day(2026, 10, 6)
            }]
        );
        // On the first occurrence it's the same plan, the move its own.
        let first = Case::new(weekly(), plain("2026-10-05T09:00:00+01:00"));
        let from_first = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Tue]),
            }),
            ..at("10:00", "2026-10-06")
        };
        let plan = first.edit(Scope::All, from_first).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            timed_node("2026-10-06T10:00:00+01:00")
        );
    }

    /// A cancelled occurrence and an EXDATE stay deleted wherever the new rule still has the day.
    #[test]
    fn all_with_a_new_rule_keeps_deleted_occurrences_deleted() {
        let with_exdate = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261102T090000",
        ]);
        let case =
            Case::new(with_exdate, monday_19()).exceptions(vec![cancelled("2026-10-26T09:00:00Z")]);
        let plan = case.edit(Scope::All, repeat(daily())).unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!([
                "RRULE:FREQ=DAILY",
                "EXDATE:20261026T090000Z,20261102T090000Z"
            ])
        );
        assert!(plan.warnings.is_empty());
    }

    /// RM1: an extra date deleted, by an EXDATE or cancelled, stays deleted wherever it's kept, for
    /// every occurrence and from one on, though the new rule doesn't make its day.
    #[test]
    fn a_new_rule_keeps_a_deleted_extra_date_deleted() {
        let both = |extra: &str| {
            let exdate = extra.replacen("RDATE", "EXDATE", 1);
            [
                Case::new(
                    master(&["RRULE:FREQ=WEEKLY;BYDAY=MO", extra, exdate.as_str()]),
                    monday_19(),
                ),
                Case::new(master(&["RRULE:FREQ=WEEKLY;BYDAY=MO", extra]), monday_19()),
            ]
        };
        // Every occurrence: Friday 23-10, made Mondays and Wednesdays.
        let friday = "RDATE;TZID=Europe/London:20261023T090000";
        let [exdated, cancelled_one] = both(friday);
        let cancelled_one = cancelled_one.exceptions(vec![cancelled("2026-10-23T09:00:00+01:00")]);
        for case in [exdated, cancelled_one] {
            let plan = case
                .edit(Scope::All, repeat(weekly_on(&[DayCode::Mon, DayCode::Wed])))
                .unwrap();
            assert_eq!(
                body(&plan, 0)["recurrence"],
                json!([
                    "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
                    "EXDATE:20261023T080000Z",
                    friday
                ])
            );
        }
        // From Monday 19-10 on: Wednesday 28-10 (London on GMT again), made Mondays and Fridays.
        let wednesday = "RDATE;TZID=Europe/London:20261028T090000";
        let [exdated, cancelled_one] = both(wednesday);
        let cancelled_one = cancelled_one.exceptions(vec![cancelled("2026-10-28T09:00:00Z")]);
        for case in [exdated, cancelled_one] {
            let plan = case
                .edit(
                    Scope::Following,
                    repeat(weekly_on(&[DayCode::Mon, DayCode::Fri])),
                )
                .unwrap();
            assert_eq!(
                body(&plan, 0)["recurrence"],
                json!([
                    "RRULE:FREQ=WEEKLY;BYDAY=MO,FR",
                    "EXDATE:20261028T090000Z",
                    wednesday
                ])
            );
        }
    }

    /// C11c review round 3: an extra date opened stays where it is, as its own RDATE, so it takes no
    /// slot of the new rule's. Monday 19-10's 09:00, moved to 14:00 as some imports write a move (an
    /// EXDATE and an RDATE), opened at 14:00 and made "Mondays and Wednesdays": 09:00 stays deleted.
    #[test]
    fn an_extra_date_opened_takes_no_slot_of_the_new_rule() {
        let two_pm = "RDATE;TZID=Europe/London:20261019T140000";
        let moved_on_import = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261019T090000",
            two_pm,
        ]);
        let plan = Case::new(moved_on_import, plain("2026-10-19T14:00:00+01:00"))
            .edit(Scope::All, repeat(weekly_on(&[DayCode::Mon, DayCode::Wed])))
            .unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
                "EXDATE:20261019T080000Z",
                two_pm
            ])
        );
    }

    /// A deleted occurrence PM can't see could come back: an incomplete listing refuses.
    #[test]
    fn all_with_a_new_rule_needs_every_changed_occurrence() {
        let mut case = Case::new(weekly(), monday_19());
        case.exceptions = Exceptions {
            items: Vec::new(),
            complete: false,
        };
        for draft in [repeat(daily()), stop()] {
            assert_eq!(
                case.edit(Scope::All, draft),
                Err(NotPlanned::Refused(SeriesRefusal::Incomplete))
            );
        }
    }

    /// Q2: "Does not repeat" for every occurrence keeps the first as one event and sends no rule;
    /// what Google answers is read by meaning, `null`, `[]` and none alike.
    #[test]
    fn all_does_not_repeat_sends_no_rule_and_keeps_the_first() {
        let case = Case::new(weekly(), monday_19());
        let plan = case.edit(Scope::All, stop()).unwrap();
        assert_eq!(
            body(&plan, 0),
            json!({ "recurrence": null }).as_object().unwrap()
        );
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 5)
            }]
        );
        assert!(matches!(
            &plan.steps[0].undo,
            Some(Undo::RestoreMaster { .. })
        ));
        // A stop reply without a rule is applied, one still holding the old rule ignored.
        let mut one_off = weekly();
        one_off.as_object_mut().unwrap().remove("recurrence");
        assert_eq!(plan.verdict(0, Some(&one_off)), Applied::Yes);
        assert_eq!(
            plan.verdict(0, Some(&with(weekly(), "recurrence", json!([])))),
            Applied::Yes
        );
        assert_eq!(plan.verdict(0, Some(&weekly())), Applied::No);
    }

    /// C11 critique 1 H1: a deleted first occurrence never comes back; the first one still there is
    /// kept, whether deleted by an EXDATE or cancelled.
    #[test]
    fn all_does_not_repeat_skips_a_deleted_first() {
        let exdated = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261005T090000",
        ]);
        let cancelled_first = Case::new(weekly(), monday_19())
            .exceptions(vec![cancelled("2026-10-05T09:00:00+01:00")]);
        for case in [Case::new(exdated, monday_19()), cancelled_first] {
            let plan = case.edit(Scope::All, stop()).unwrap();
            let sent = body(&plan, 0);
            assert_eq!(sent["start"], timed_node("2026-10-12T09:00:00+01:00"));
            assert_eq!(sent["end"], timed_node("2026-10-12T10:00:00+01:00"));
            assert_eq!(sent["recurrence"], Value::Null);
            assert_eq!(
                plan.warnings,
                [SeriesWarning::BecomesOneEvent {
                    day: day(2026, 10, 12)
                }]
            );
        }
        // An extra date opened after every one of the rule's was deleted: none to keep.
        let only_extra = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=1",
            "EXDATE;TZID=Europe/London:20261005T090000",
            "RDATE;TZID=Europe/London:20261007T090000",
        ]);
        let extra = plain("2026-10-07T09:00:00+01:00");
        assert_eq!(
            Case::new(only_extra, extra).edit(Scope::All, stop()),
            Err(NotPlanned::Refused(SeriesRefusal::NothingToKeep))
        );
        // A line PM can't read may be the EXDATE that deleted the first: it could come back as the
        // one event, so a stop refuses it as a new rule does.
        let unreadable = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Mars/Olympus:20261005T090000",
        ]);
        assert_eq!(
            Case::new(unreadable, monday_19()).edit(Scope::All, stop()),
            Err(NotPlanned::Refused(SeriesRefusal::Recur(
                RecurError::TextOnly(TextOnly::Unreadable)
            )))
        );
        // Its words point where it can be done: "This event" can't change how it repeats.
        assert_eq!(
            SeriesRefusal::NothingToKeep.to_string(),
            "Every occurrence of its repeat up to this one was deleted, so PM can't tell which one \
             should stay. Change it in Google Calendar."
        );
    }

    /// Every other occurrence goes, but Google may keep those changed on their own, so the hedge
    /// counts them; never the one kept, whose own changes go with it.
    #[test]
    fn all_does_not_repeat_hedges_on_the_others_changed_on_their_own() {
        let kick_off = with(
            plain("2026-10-05T09:00:00+01:00"),
            "summary",
            json!("Kick-off"),
        );
        let planning = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Planning"));
        let replied = plain("2026-11-02T09:00:00Z");
        let case =
            Case::new(weekly(), monday_19()).exceptions(vec![kick_off.clone(), planning, replied]);
        let plan = case.edit(Scope::All, stop()).unwrap();
        assert_eq!(body(&plan, 0)["summary"], "Kick-off");
        assert_eq!(
            plan.warnings,
            [
                SeriesWarning::BecomesOneEvent {
                    day: day(2026, 10, 5)
                },
                SeriesWarning::ExceptionsMayReset {
                    count: 1,
                    complete: true
                }
            ]
        );
        // Only the one kept was changed: nothing to hedge on.
        let plan = Case::new(weekly(), monday_19())
            .exceptions(vec![kick_off])
            .edit(Scope::All, stop())
            .unwrap();
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 5)
            }]
        );
    }

    /// With this occurrence moved a day on, the one kept moves as far, and the dialog says it leaves
    /// its own day; a new time on the same day moves nothing.
    #[test]
    fn all_does_not_repeat_with_a_moved_day_says_the_first_moves() {
        let case = Case::new(weekly(), monday_19());
        let a_day_on = EventPatchDraft {
            recurrence: Some(RepeatDraft::Once),
            ..at("09:00", "2026-10-20")
        };
        let plan = case.edit(Scope::All, a_day_on).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            timed_node("2026-10-06T09:00:00+01:00")
        );
        assert_eq!(
            plan.warnings,
            [
                SeriesWarning::BecomesOneEvent {
                    day: day(2026, 10, 6)
                },
                SeriesWarning::StartMoved {
                    from: day(2026, 10, 5),
                    to: day(2026, 10, 6)
                }
            ]
        );
        let an_hour_on = EventPatchDraft {
            recurrence: Some(RepeatDraft::Once),
            ..at("10:00", "2026-10-19")
        };
        assert_eq!(
            case.edit(Scope::All, an_hour_on).unwrap().warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 5)
            }]
        );
    }

    /// C11 critique 1 H1: the kept occurrence's own time and title stay, under the draft's.
    #[test]
    fn all_does_not_repeat_keeps_the_firsts_own_changes() {
        let kick_off = with(
            moved(
                "2026-10-05T09:00:00+01:00",
                "2026-10-06T11:00:00+01:00",
                "2026-10-06T12:00:00+01:00",
            ),
            "summary",
            json!("Kick-off"),
        );
        let case = Case::new(weekly(), monday_19()).exceptions(vec![kick_off.clone()]);
        let plan = case.edit(Scope::All, stop()).unwrap();
        let sent = body(&plan, 0);
        assert_eq!(sent["start"], timed_node("2026-10-06T11:00:00+01:00"));
        assert_eq!(sent["end"], timed_node("2026-10-06T12:00:00+01:00"));
        assert_eq!(sent["summary"], "Kick-off");
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 6)
            }]
        );
        // The draft's own title wins.
        let retro = EventPatchDraft {
            recurrence: Some(RepeatDraft::Once),
            ..title("Retro")
        };
        assert_eq!(
            body(&case.edit(Scope::All, retro).unwrap(), 0)["summary"],
            "Retro"
        );
        // A colour, reminders or a video call of its own, which the series' PATCH can't keep, refuse
        // in words.
        for (key, value) in [
            ("colorId", json!("5")),
            (
                "reminders",
                json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": 5 }] }),
            ),
            ("conferenceData", json!({ "conferenceId": "abc-defg-hij" })),
        ] {
            let own = with(plain("2026-10-05T09:00:00+01:00"), key, value);
            assert_eq!(
                Case::new(weekly(), monday_19())
                    .exceptions(vec![own])
                    .edit(Scope::All, stop()),
                Err(NotPlanned::Refused(SeriesRefusal::KeptHasOwn(day(
                    2026, 10, 5
                )))),
                "{key}"
            );
        }
    }

    /// C11 critique 1 H2: a rebuilt rule never repeats on or past where a later part starts; one
    /// that ends before it is fine, and so is a stop.
    #[test]
    fn a_new_rule_never_repeats_past_a_later_part() {
        let ending = |date: &str| {
            repeat(rule(
                RepeatPattern::Daily,
                1,
                RepeatEnds::OnDate { date: date.into() },
            ))
        };
        let past = Err(NotPlanned::Refused(SeriesRefusal::RepeatsPastPart(day(
            2026, 11, 2,
        ))));
        for later in [
            LaterPart::At(point("2026-11-02T09:00:00Z")),
            // A later part known by its day: that day counts.
            LaterPart::At(Point::Day(day(2026, 11, 2))),
        ] {
            let mut case = Case::new(weekly(), monday_19());
            case.later = later;
            assert_eq!(case.edit(Scope::All, repeat(daily())), past);
            assert_eq!(case.edit(Scope::Following, repeat(daily())), past);
            assert_eq!(case.edit(Scope::All, ending("2026-11-02")), past);
            assert!(case.edit(Scope::All, ending("2026-11-01")).is_ok());
            assert!(case.edit(Scope::Following, ending("2026-11-01")).is_ok());
            assert!(case.edit(Scope::All, stop()).is_ok());
        }
        let mut case = Case::new(weekly(), monday_19());
        case.later = LaterPart::Unread;
        assert_eq!(
            case.edit(Scope::All, repeat(daily())),
            Err(NotPlanned::Refused(SeriesRefusal::LaterPartUnread))
        );
    }

    /// C11c review: a series that is itself a part, held to its last occurrence's day now (Monday
    /// 26-10), never repeats on a day after it, timed or all-day, whatever later part there may be.
    #[test]
    fn a_part_never_repeats_past_where_it_ends_now() {
        let ending = |date: &str| {
            rule(
                RepeatPattern::Daily,
                1,
                RepeatEnds::OnDate { date: date.into() },
            )
        };
        let all_day = |rule: RepeatRule| EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            recurrence: Some(RepeatDraft::Rule { rule }),
            ..Default::default()
        };
        let mut case = Case::new(weekly(), monday_19());
        case.later = LaterPart::Through(day(2026, 10, 26));
        let past = Err(NotPlanned::Refused(SeriesRefusal::RepeatsPastItsEnd(day(
            2026, 10, 26,
        ))));
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(case.edit(scope, repeat(daily())), past);
            assert_eq!(case.edit(scope, repeat(ending("2026-10-27"))), past);
            assert!(case.edit(scope, repeat(ending("2026-10-26"))).is_ok());
            assert_eq!(case.edit(scope, all_day(ending("2026-10-27"))), past);
            assert!(case.edit(scope, all_day(ending("2026-10-26"))).is_ok());
            assert!(case.edit(scope, stop()).is_ok());
        }
    }

    /// C11 critique 2 M8: what was sent is read at the start it repeats from once landed. Every
    /// occurrence made all-day, daily until 31-12: Google writing the UNTIL as midnight UTC is the
    /// same day there, which the old timed start would read as another moment.
    #[test]
    fn a_new_rule_reply_is_read_at_the_new_start() {
        let case = Case::new(weekly(), monday_19());
        let all_day = EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            recurrence: Some(RepeatDraft::Rule {
                rule: rule(
                    RepeatPattern::Daily,
                    1,
                    RepeatEnds::OnDate {
                        date: "2026-12-31".into(),
                    },
                ),
            }),
            ..Default::default()
        };
        let plan = case.edit(Scope::All, all_day).unwrap();
        let sent = body(&plan, 0);
        assert_eq!(
            sent["start"],
            json!({ "date": "2026-10-05", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            sent["recurrence"],
            json!(["RRULE:FREQ=DAILY;UNTIL=20261231"])
        );
        assert!(plan.warnings.contains(&SeriesWarning::LengthChanges));
        assert_eq!(plan.rule_start, SeriesStart::AllDay(day(2026, 10, 5)));
        let reply = with(
            with(
                with(weekly(), "start", json!({ "date": "2026-10-05" })),
                "end",
                json!({ "date": "2026-10-06" }),
            ),
            "recurrence",
            json!(["RRULE:UNTIL=20261231T000000Z;FREQ=DAILY"]),
        );
        assert_eq!(plan.verdict(0, Some(&reply)), Applied::Yes);
        // The rule landed without its start: half a change, read the same way.
        let half = with(
            weekly(),
            "recurrence",
            json!(["RRULE:UNTIL=20261231T000000Z;FREQ=DAILY"]),
        );
        assert!(plan.half_applied(0, &half));
        // Weekly on Tuesday, written back in another order.
        let tuesday = case
            .edit(Scope::All, repeat(weekly_on(&[DayCode::Tue])))
            .unwrap();
        let landed = with(
            with(
                with(weekly(), "start", timed_node("2026-10-06T08:00:00Z")),
                "end",
                timed_node("2026-10-06T09:00:00Z"),
            ),
            "recurrence",
            json!(["RRULE:BYDAY=TU;FREQ=WEEKLY"]),
        );
        assert_eq!(tuesday.verdict(0, Some(&landed)), Applied::Yes);
    }

    /// DS8: a new rule that lands without its start (or the start without the rule) is half a
    /// change, and the master is put back.
    #[test]
    fn half_applied_reads_a_new_rule_with_the_old_start() {
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::All, repeat(weekly_on(&[DayCode::Tue])))
            .unwrap();
        let tuesdays = json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]);
        assert!(plan.half_applied(0, &with(weekly(), "recurrence", tuesdays.clone())));
        let new_start = with(
            with(weekly(), "start", timed_node("2026-10-06T09:00:00+01:00")),
            "end",
            timed_node("2026-10-06T10:00:00+01:00"),
        );
        assert!(plan.half_applied(0, &new_start));
        assert!(!plan.half_applied(0, &with(new_start, "recurrence", tuesdays)));
        assert!(!plan.half_applied(0, &weekly()));
    }

    // --- "This and following" with a new rule ---

    /// §9.10: a Tuesday occurrence made "Weekly on Monday and Wednesday" starts the new part on the
    /// Wednesday, and says so.
    #[test]
    fn following_with_a_new_rule_starts_the_new_part_on_its_first_day() {
        let tuesdays = master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=TU"],
            "2026-10-06T09:00:00+01:00",
            "2026-10-06T10:00:00+01:00",
        );
        let case = Case::new(tuesdays, plain("2026-10-20T09:00:00+01:00"));
        let plan = case
            .edit(
                Scope::Following,
                repeat(weekly_on(&[DayCode::Wed, DayCode::Mon])),
            )
            .unwrap();
        assert_eq!(plan.scope, Scope::Following);
        let inserted = body(&plan, 0);
        assert_eq!(inserted["start"], timed_node("2026-10-21T09:00:00+01:00"));
        assert_eq!(
            inserted["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"])
        );
        assert_eq!(
            inserted["extendedProperties"]["private"]["pmSplitOf"],
            "abc"
        );
        assert_eq!(
            body(&plan, 1)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU;UNTIL=20261020T075959Z"])
        );
        assert_eq!(
            plan.warnings,
            [
                SeriesWarning::StartMoved {
                    from: day(2026, 10, 20),
                    to: day(2026, 10, 21)
                },
                SeriesWarning::NewIdentity
            ]
        );
        assert!(plan.expect.contains(&Expect::Present {
            series: TAIL.into()
        }));
        assert!(matches!(plan.steps[0].shows, Shows::Tail { .. }));
    }

    /// A new part made all-day is read against its own start: Google writing its rule's UNTIL as
    /// midnight UTC is still the part PM made, never a change made to it since.
    #[test]
    fn a_new_part_of_another_kind_is_read_at_its_own_start() {
        let all_day = EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            recurrence: Some(RepeatDraft::Rule {
                rule: rule(
                    RepeatPattern::Daily,
                    1,
                    RepeatEnds::OnDate {
                        date: "2026-12-31".into(),
                    },
                ),
            }),
            ..Default::default()
        };
        let plan = Case::new(weekly(), monday_19())
            .edit(Scope::Following, all_day)
            .unwrap();
        let mut copy = Value::Object(body(&plan, 0).clone());
        assert_eq!(copy["start"], json!({ "date": "2026-10-19" }));
        assert_eq!(
            copy["recurrence"],
            json!(["RRULE:FREQ=DAILY;UNTIL=20261231"])
        );
        assert!(plan.tail_as_inserted(0, &copy));
        copy["recurrence"] = json!(["RRULE:FREQ=DAILY;UNTIL=20261231T000000Z"]);
        assert!(plan.tail_as_inserted(0, &copy));
        copy["recurrence"] = json!(["RRULE:FREQ=DAILY;UNTIL=20261230"]);
        assert!(!plan.tail_as_inserted(0, &copy));
    }

    /// What a new part can't carry refuses, as a split does; a stop takes every later occurrence
    /// away, so only this one's own must travel.
    #[test]
    fn a_new_rule_or_a_stop_refuses_what_it_cant_carry() {
        let attachment = json!([{ "fileUrl": "https://drive.google.com/x" }]);
        let later = with(
            plain("2026-10-26T09:00:00Z"),
            "attachments",
            attachment.clone(),
        );
        let case = Case::new(weekly(), monday_19()).exceptions(vec![later]);
        assert_eq!(
            case.edit(Scope::Following, repeat(daily())),
            Err(NotPlanned::Refused(SeriesRefusal::CannotCarry(day(
                2026, 10, 26
            ))))
        );
        assert!(case.edit(Scope::Following, stop()).is_ok());
        let this_one = with(monday_19(), "attachments", attachment);
        let case = Case::new(weekly(), this_one.clone()).exceptions(vec![this_one]);
        assert_eq!(
            case.edit(Scope::Following, stop()),
            Err(NotPlanned::Refused(SeriesRefusal::CannotCarry(day(
                2026, 10, 19
            ))))
        );
    }

    /// One event at a time the clocks skip is refused as a one-off's save refuses it: every
    /// occurrence a week later puts the first, 22-03 01:30, on 29-03, when London skips 01:30.
    #[test]
    fn a_stop_at_a_time_the_clocks_skip_is_refused() {
        let sundays = master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=SU"],
            "2026-03-22T01:30:00+00:00",
            "2026-03-22T02:30:00+00:00",
        );
        let case = Case::new(sundays, plain("2026-04-05T01:30:00+01:00"));
        let a_week_on = EventPatchDraft {
            recurrence: Some(RepeatDraft::Once),
            ..at("01:30", "2026-04-12")
        };
        assert!(matches!(
            case.edit(Scope::All, a_week_on),
            Err(NotPlanned::Refused(SeriesRefusal::Time(time::TimeError::Gap { wall, .. })))
                if wall == "29-03-2026 01:30"
        ));
    }

    /// Q3: a change made on its own to an occurrence the new rule still has is carried onto it; one
    /// on a day it skips goes, and is counted. Both are cancelled on the original.
    #[test]
    fn following_with_a_new_rule_carries_only_changes_on_its_days() {
        let planning = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Planning"));
        let off_site = with(plain("2026-11-02T09:00:00Z"), "summary", json!("Off-site"));
        let case = Case::new(weekly(), monday_19()).exceptions(vec![planning, off_site]);
        let fortnightly = rule(
            RepeatPattern::Weekly {
                days: vec![DayCode::Mon],
            },
            2,
            RepeatEnds::Never,
        );
        let plan = case.edit(Scope::Following, repeat(fortnightly)).unwrap();
        // Insert, cut; 26-10 cancelled with nothing carried; 02-11 carried, then cancelled.
        assert_eq!(plan.steps.len(), 5);
        assert!(matches!(
            &plan.steps[2].op,
            Op::Cancel { original, .. } if *original == point("2026-10-26T09:00:00Z")
        ));
        assert_eq!(plan.steps[2].requires, None);
        assert!(matches!(
            &plan.steps[3].op,
            Op::Carry { original, series, body, .. }
                if *original == point("2026-11-02T09:00:00Z")
                    && series == TAIL
                    && body["summary"] == "Off-site"
        ));
        assert!(matches!(
            &plan.steps[4].op,
            Op::Cancel { original, .. } if *original == point("2026-11-02T09:00:00Z")
        ));
        assert_eq!(plan.steps[4].requires, Some(3));
        assert_eq!(
            plan.warnings,
            [
                SeriesWarning::ChangesLost { count: 1 },
                SeriesWarning::NewIdentity
            ]
        );
    }

    /// C11c review: moved to Wednesday and made "Mondays and Wednesdays", the new part keeps a
    /// Monday deleted and a Monday changed where they were, since it still has Mondays; never on the
    /// Wednesdays two days on, which nobody deleted or changed.
    #[test]
    fn following_with_a_moved_day_keeps_deletions_and_changes_on_days_it_keeps() {
        let planning = with(plain("2026-11-02T09:00:00Z"), "summary", json!("Planning"));
        let case = Case::new(weekly(), monday_19())
            .exceptions(vec![cancelled("2026-10-26T09:00:00Z"), planning]);
        let to_wednesday = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Mon, DayCode::Wed]),
            }),
            ..at("09:00", "2026-10-21")
        };
        let plan = case.edit(Scope::Following, to_wednesday).unwrap();
        let inserted = body(&plan, 0);
        assert_eq!(inserted["start"], timed_node("2026-10-21T09:00:00+01:00"));
        assert_eq!(
            inserted["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO,WE", "EXDATE:20261026T090000Z"])
        );
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, body, .. }
                if *original == point("2026-11-02T09:00:00Z") && body["summary"] == "Planning"
        ));
        assert_eq!(plan.warnings, [SeriesWarning::NewIdentity]);
    }

    /// A change on a day the new rule doesn't keep moves with the day, but never onto a day the old
    /// series had an occurrence of its own: which is which would be a guess, so it goes, counted.
    #[test]
    fn following_with_a_moved_day_carries_by_the_move_only_onto_new_days() {
        let planning = with(plain("2026-11-02T09:00:00Z"), "summary", json!("Planning"));
        let case = Case::new(weekly(), monday_19()).exceptions(vec![planning.clone()]);
        let wednesdays = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Wed]),
            }),
            ..at("09:00", "2026-10-21")
        };
        let plan = case.edit(Scope::Following, wednesdays).unwrap();
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, .. } if *original == point("2026-11-04T09:00:00Z")
        ));
        // Mondays and Wednesdays made Wednesdays and Fridays, two days on: Monday 26-10 would land
        // on Wednesday 28-10, which has its own change already.
        let both = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"]);
        let monday = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Planning"));
        let wednesday = with(plain("2026-10-28T09:00:00Z"), "summary", json!("Review"));
        let case = Case::new(both, monday_19()).exceptions(vec![monday, wednesday]);
        let later_on = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Wed, DayCode::Fri]),
            }),
            ..at("09:00", "2026-10-21")
        };
        let plan = case.edit(Scope::Following, later_on).unwrap();
        let carried: Vec<(Point, &Value)> = plan
            .steps
            .iter()
            .filter_map(|s| match &s.op {
                Op::Carry { original, body, .. } => Some((*original, &body["summary"])),
                _ => None,
            })
            .collect();
        assert_eq!(carried, [(point("2026-10-28T09:00:00Z"), &json!("Review"))]);
        assert!(plan
            .warnings
            .contains(&SeriesWarning::ChangesLost { count: 1 }));
    }

    /// The occurrence split at goes where the draft put it, a day earlier, though the new rule still
    /// has its own day.
    #[test]
    fn following_carries_this_ones_changes_where_the_draft_put_it() {
        let retro = with(monday_19(), "summary", json!("Retro"));
        let case = Case::new(weekly(), retro.clone()).exceptions(vec![retro]);
        let a_day_back = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule { rule: daily() }),
            ..at("09:00", "2026-10-18")
        };
        let plan = case.edit(Scope::Following, a_day_back).unwrap();
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, body, .. }
                if *original == point("2026-10-18T08:00:00Z") && body["summary"] == "Retro"
        ));
    }

    /// Every carry a plan makes: where it goes, and the title it carries.
    fn carries(plan: &SeriesPlan) -> Vec<(Point, Value)> {
        plan.steps
            .iter()
            .filter_map(|s| match &s.op {
                Op::Carry { original, body, .. } => Some((*original, body["summary"].clone())),
                _ => None,
            })
            .collect()
    }

    /// C11c review: Mondays and Tuesdays, Monday 19-10 opened and moved onto Tuesday 20-10, made
    /// "Weekly on Tuesday". That slot is the occurrence opened's: Tuesday 20-10 deleted, by an EXDATE
    /// or cancelled, writes no EXDATE there, for every occurrence or from this one on; changed on its
    /// own, its changes go, counted, rather than land on the one the user just moved there.
    #[test]
    fn the_occurrence_opened_keeps_the_slot_the_draft_gives_it() {
        let mon_tue = |more: &[&str]| {
            let mut lines = vec!["RRULE:FREQ=WEEKLY;BYDAY=MO,TU"];
            lines.extend_from_slice(more);
            master(&lines)
        };
        let to_tuesday = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Tue]),
            }),
            ..at("09:00", "2026-10-20")
        };
        let exdated = Case::new(
            mon_tue(&["EXDATE;TZID=Europe/London:20261020T090000"]),
            monday_19(),
        );
        let cancelled_one = Case::new(mon_tue(&[]), monday_19())
            .exceptions(vec![cancelled("2026-10-20T09:00:00+01:00")]);
        for case in [exdated, cancelled_one] {
            let plan = case.edit(Scope::Following, to_tuesday.clone()).unwrap();
            let inserted = body(&plan, 0);
            assert_eq!(inserted["start"], timed_node("2026-10-20T09:00:00+01:00"));
            assert_eq!(
                inserted["recurrence"],
                json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"])
            );
            let plan = case.edit(Scope::All, to_tuesday.clone()).unwrap();
            assert_eq!(
                body(&plan, 0)["recurrence"],
                json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"])
            );
        }
        // Tuesday 20-10 retitled and moved to 11:00 on its own; Monday 19-10, opened, retitled too.
        let review = with(
            moved(
                "2026-10-20T09:00:00+01:00",
                "2026-10-20T11:00:00+01:00",
                "2026-10-20T12:00:00+01:00",
            ),
            "summary",
            json!("Review"),
        );
        let retro = with(monday_19(), "summary", json!("Retro"));
        let case = Case::new(mon_tue(&[]), retro.clone()).exceptions(vec![retro, review]);
        let plan = case.edit(Scope::Following, to_tuesday).unwrap();
        assert_eq!(
            carries(&plan),
            [(point("2026-10-20T08:00:00Z"), json!("Retro"))]
        );
        assert!(plan
            .warnings
            .contains(&SeriesWarning::ChangesLost { count: 1 }));
        assert!(plan.steps.iter().any(|s| matches!(
            &s.op,
            Op::Cancel { original, .. } if *original == point("2026-10-20T08:00:00Z")
        )));
    }

    /// Mondays and Tuesdays at 23:30 in London, Monday 19-10 opened and moved into Tokyo at the same
    /// moment, Tuesday 20-10 07:30, made "Tuesdays and Wednesdays", as the days read there.
    fn into_tokyo() -> (Value, EventPatchDraft) {
        let late = master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO,TU"],
            "2026-10-05T23:30:00+01:00",
            "2026-10-06T00:30:00+01:00",
        );
        let draft = EventPatchDraft {
            time: Some(in_tokyo("2026-10-20", "07:30", "2026-10-20", "08:30")),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Tue, DayCode::Wed]),
            }),
            ..Default::default()
        };
        (late, draft)
    }

    /// A time in Tokyo, from `start` on `start_date` to `end` on `end_date`.
    fn in_tokyo(start_date: &str, start: &str, end_date: &str, end: &str) -> TimeDraft {
        TimeDraft::Timed {
            start_date: start_date.into(),
            start_time: start.into(),
            start_zone: "Asia/Tokyo".into(),
            end_date: end_date.into(),
            end_time: end.into(),
            end_zone: "Asia/Tokyo".into(),
        }
    }

    fn tokyo_node(at: &str) -> Value {
        json!({ "dateTime": at, "timeZone": "Asia/Tokyo" })
    }

    /// C11c review: a move into another zone that keeps every moment moves no day. Tuesday 20-10
    /// 23:30 in London, deleted, is Wednesday 21-10 07:30 in Tokyo: it stays deleted there, never on
    /// the Tuesday before it, which is the occurrence opened; and nothing asks about the days.
    #[test]
    fn all_into_another_zone_keeps_each_deletion_at_its_moment() {
        let (late, draft) = into_tokyo();
        let case = Case::new(late, plain("2026-10-19T23:30:00+01:00"))
            .exceptions(vec![cancelled("2026-10-20T23:30:00+01:00")]);
        let plan = case.edit(Scope::All, draft).unwrap();
        let sent = body(&plan, 0);
        assert_eq!(sent["start"], tokyo_node("2026-10-06T07:30:00+09:00"));
        assert_eq!(sent["end"], tokyo_node("2026-10-06T08:30:00+09:00"));
        assert_eq!(
            sent["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU,WE", "EXDATE:20261020T223000Z"])
        );
        assert!(!plan.warnings.iter().any(|w| matches!(
            w,
            SeriesWarning::RepeatDayChanges { .. } | SeriesWarning::StartMoved { .. }
        )));
        // A day move there is read in Tokyo's days too. Moved on to Wednesday 21-10 and made
        // "Wednesdays and Thursdays": Monday 12-10 deleted, Tuesday 13-10 in Tokyo, which the new rule
        // skips, would move to Wednesday 14-10 there; but that's Tuesday 13-10 23:30 in London, an
        // occurrence of its own, so nothing is deleted on it.
        let (late, _) = into_tokyo();
        let wednesday = EventPatchDraft {
            time: Some(in_tokyo("2026-10-21", "07:30", "2026-10-21", "08:30")),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Wed, DayCode::Thu]),
            }),
            ..Default::default()
        };
        let case = Case::new(late, plain("2026-10-19T23:30:00+01:00"))
            .exceptions(vec![cancelled("2026-10-12T23:30:00+01:00")]);
        let plan = case.edit(Scope::All, wednesday).unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=WE,TH"])
        );
    }

    /// The same from this occurrence on: the deletion stays at its moment, and a change made on its
    /// own (Tuesday 27-10, London on GMT again) goes onto the new part's occurrence on its own day
    /// there, Wednesday 28-10, never onto the one before it.
    #[test]
    fn following_into_another_zone_keeps_each_occurrence_on_its_own_day() {
        let (late, draft) = into_tokyo();
        let review = with(plain("2026-10-27T23:30:00Z"), "summary", json!("Review"));
        let case = Case::new(late, plain("2026-10-19T23:30:00+01:00"))
            .exceptions(vec![cancelled("2026-10-20T23:30:00+01:00"), review]);
        let plan = case.edit(Scope::Following, draft).unwrap();
        let inserted = body(&plan, 0);
        assert_eq!(inserted["start"], tokyo_node("2026-10-20T07:30:00+09:00"));
        assert_eq!(
            inserted["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU,WE", "EXDATE:20261020T223000Z"])
        );
        assert_eq!(
            carries(&plan),
            [(point("2026-10-27T22:30:00Z"), json!("Review"))]
        );
        assert!(!plan
            .warnings
            .iter()
            .any(|w| matches!(w, SeriesWarning::ChangesLost { .. })));
    }

    /// C11c review round 3: from this occurrence on, the day move is counted in the new part's days
    /// too, so a move into Tokyo at the same moment moves no day. Made "Tuesdays and Thursdays",
    /// Tuesday 20-10 23:30 in London is Wednesday 21-10 there, which the new rule skips and the series
    /// had its own on: changed on its own, its change goes, counted; deleted, nothing is deleted.
    /// Never onto Thursday 22-10, which nobody changed or deleted.
    #[test]
    fn following_into_another_zone_moves_no_day() {
        let (late, draft) = into_tokyo();
        let draft = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Tue, DayCode::Thu]),
            }),
            ..draft
        };
        let opened = plain("2026-10-19T23:30:00+01:00");
        let review = with(
            plain("2026-10-20T23:30:00+01:00"),
            "summary",
            json!("Review"),
        );
        let plan = Case::new(late.clone(), opened.clone())
            .exceptions(vec![review])
            .edit(Scope::Following, draft.clone())
            .unwrap();
        assert!(carries(&plan).is_empty());
        assert!(plan
            .warnings
            .contains(&SeriesWarning::ChangesLost { count: 1 }));
        let plan = Case::new(late, opened)
            .exceptions(vec![cancelled("2026-10-20T23:30:00+01:00")])
            .edit(Scope::Following, draft)
            .unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU,TH"])
        );
    }

    /// Mondays and Tuesdays at 15:30 in London from 05-10, with `more` lines; London's clocks go back
    /// on 25-10.
    fn afternoons(more: &[&str]) -> Value {
        let mut lines = vec!["RRULE:FREQ=WEEKLY;BYDAY=MO,TU"];
        lines.extend_from_slice(more);
        master_with(
            &lines,
            "2026-10-05T15:30:00+01:00",
            "2026-10-05T16:30:00+01:00",
        )
    }

    /// C11c review round 3: both rules repeat by wall day in their own zone, so a move into another
    /// zone keeps each occurrence's place in the week whatever either zone's clocks do. Monday 19-10
    /// opened and moved into Tokyo at the same moment, 23:30 there: once London's clocks go back,
    /// Monday 02-11 15:30 is Tuesday 03-11 00:30 in Tokyo, but it's still the Monday, now Monday 23:30
    /// there. Deleted, it stays deleted on that Monday, never on the live Tuesday; changed on its own,
    /// its change goes onto that Monday, and the Tuesday's onto the Tuesday.
    #[test]
    fn a_zone_move_across_a_clock_change_keeps_each_occurrence_in_its_place() {
        let draft = EventPatchDraft {
            time: Some(in_tokyo("2026-10-19", "23:30", "2026-10-20", "00:30")),
            recurrence: Some(RepeatDraft::Rule {
                rule: rule(
                    RepeatPattern::Weekly {
                        days: vec![DayCode::Mon, DayCode::Tue],
                    },
                    1,
                    RepeatEnds::OnDate {
                        date: "2026-12-31".into(),
                    },
                ),
            }),
            ..Default::default()
        };
        let opened = plain("2026-10-19T15:30:00+01:00");
        let exdated = Case::new(
            afternoons(&["EXDATE;TZID=Europe/London:20261102T153000"]),
            opened.clone(),
        );
        let cancelled_one = Case::new(afternoons(&[]), opened.clone())
            .exceptions(vec![cancelled("2026-11-02T15:30:00Z")]);
        for case in [exdated, cancelled_one] {
            let plan = case.edit(Scope::All, draft.clone()).unwrap();
            let sent = body(&plan, 0);
            assert_eq!(sent["start"], tokyo_node("2026-10-05T23:30:00+09:00"));
            assert_eq!(
                sent["recurrence"],
                json!([
                    "RRULE:FREQ=WEEKLY;BYDAY=MO,TU;UNTIL=20261231T145959Z",
                    "EXDATE:20261102T143000Z"
                ])
            );
            assert!(plan.warnings.is_empty());
        }
        let retro = with(plain("2026-11-02T15:30:00Z"), "summary", json!("Retro"));
        let review = with(plain("2026-11-03T15:30:00Z"), "summary", json!("Review"));
        let plan = Case::new(afternoons(&[]), opened)
            .exceptions(vec![retro, review])
            .edit(Scope::Following, draft)
            .unwrap();
        assert_eq!(
            carries(&plan),
            [
                (point("2026-11-02T14:30:00Z"), json!("Retro")),
                (point("2026-11-03T14:30:00Z"), json!("Review"))
            ]
        );
        assert!(!plan
            .warnings
            .iter()
            .any(|w| matches!(w, SeriesWarning::ChangesLost { .. })));
        // And the other way: Monday 02-11 opened, in winter time, is Tuesday 03-11 00:30 in Tokyo, made
        // "Tuesdays and Wednesdays" there. Tuesday 13-10 15:30, in summer time, is 23:30 that Tuesday
        // in Tokyo, but it's the Tuesday: deleted, it stays deleted on the Wednesday 00:30 after it.
        let winter = EventPatchDraft {
            time: Some(in_tokyo("2026-11-03", "00:30", "2026-11-03", "01:30")),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Tue, DayCode::Wed]),
            }),
            ..Default::default()
        };
        let plan = Case::new(
            afternoons(&["EXDATE;TZID=Europe/London:20261013T153000"]),
            plain("2026-11-02T15:30:00Z"),
        )
        .edit(Scope::All, winter)
        .unwrap();
        let sent = body(&plan, 0);
        assert_eq!(sent["start"], tokyo_node("2026-10-06T00:30:00+09:00"));
        assert_eq!(
            sent["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU,WE", "EXDATE:20261013T153000Z"])
        );
    }

    /// C11c review round 3: whether the old series had an occurrence of its own on the day a deletion
    /// moves to is read on that day in its own zone. Mondays and Wednesdays at 15:30 in London, Monday
    /// 19-10 moved to Wednesday 21-10 23:30 in Tokyo and made "Weekly on Wednesday": Monday 02-11,
    /// deleted, would move to Wednesday 04-11, which had its own (Thursday 00:30 in Tokyo, once
    /// London's clocks go back), so nothing is deleted on it.
    #[test]
    fn a_zone_move_looks_for_the_old_days_in_their_own_zone() {
        let mon_wed = master_with(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
                "EXDATE;TZID=Europe/London:20261102T153000",
            ],
            "2026-10-05T15:30:00+01:00",
            "2026-10-05T16:30:00+01:00",
        );
        let wednesdays = EventPatchDraft {
            time: Some(in_tokyo("2026-10-21", "23:30", "2026-10-22", "00:30")),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Wed]),
            }),
            ..Default::default()
        };
        let plan = Case::new(mon_wed, plain("2026-10-19T15:30:00+01:00"))
            .edit(Scope::All, wednesdays)
            .unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=WE"])
        );
    }

    /// C11c review round 3: "Does not repeat" for every occurrence moved into Tokyo keeps the first
    /// at its own moment, in the draft's days. Monday 05-10 23:30 in London is Tuesday 06-10 07:30
    /// there, and nothing says it moves; moved a day on there, it moves, worded in the series' own
    /// days as the dialog reads them: from Monday 05-10 (C11d review 5).
    #[test]
    fn all_does_not_repeat_into_another_zone_keeps_the_first_at_its_moment() {
        let once = |time: TimeDraft| EventPatchDraft {
            time: Some(time),
            recurrence: Some(RepeatDraft::Once),
            ..Default::default()
        };
        let (late, _) = into_tokyo();
        let case = Case::new(late, plain("2026-10-19T23:30:00+01:00"));
        let plan = case
            .edit(
                Scope::All,
                once(in_tokyo("2026-10-20", "07:30", "2026-10-20", "08:30")),
            )
            .unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            tokyo_node("2026-10-06T07:30:00+09:00")
        );
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 6)
            }]
        );
        let plan = case
            .edit(
                Scope::All,
                once(in_tokyo("2026-10-21", "07:30", "2026-10-21", "08:30")),
            )
            .unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            tokyo_node("2026-10-07T07:30:00+09:00")
        );
        assert_eq!(
            plan.warnings,
            [
                SeriesWarning::BecomesOneEvent {
                    day: day(2026, 10, 7)
                },
                SeriesWarning::StartMoved {
                    from: day(2026, 10, 5),
                    to: day(2026, 10, 7)
                }
            ]
        );
        // Across the clock change: weekly on Monday at 15:30 in London, Monday 02-11 opened, in winter
        // time, and moved into Tokyo at the same moment, Tuesday 03-11 00:30. The first, Monday 05-10
        // 15:30 in summer time, is 23:30 that Monday in Tokyo, but it's the Monday the series now puts
        // on Tuesday 00:30: one event on Tuesday 06-10, never Monday 05-10 00:30, a day early. A new
        // rule starts there too.
        let afternoon = master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            "2026-10-05T15:30:00+01:00",
            "2026-10-05T16:30:00+01:00",
        );
        let case = Case::new(afternoon, plain("2026-11-02T15:30:00Z"));
        let winter = in_tokyo("2026-11-03", "00:30", "2026-11-03", "01:30");
        let plan = case.edit(Scope::All, once(winter.clone())).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            tokyo_node("2026-10-06T00:30:00+09:00")
        );
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 6)
            }]
        );
        let every_day = EventPatchDraft {
            time: Some(winter),
            recurrence: Some(RepeatDraft::Rule { rule: daily() }),
            ..Default::default()
        };
        let plan = case.edit(Scope::All, every_day).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            tokyo_node("2026-10-06T00:30:00+09:00")
        );
        assert!(plan.warnings.is_empty());
    }

    /// RM1: an extra date kept is still an occurrence where it was, so its own changes go onto it
    /// there, though the new rule doesn't make its day.
    #[test]
    fn following_with_a_new_rule_carries_an_extra_dates_own_changes() {
        let wednesday = "RDATE;TZID=Europe/London:20261028T090000";
        let review = with(plain("2026-10-28T09:00:00Z"), "summary", json!("Review"));
        let case = Case::new(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO", wednesday]),
            monday_19(),
        )
        .exceptions(vec![review]);
        let plan = case
            .edit(
                Scope::Following,
                repeat(weekly_on(&[DayCode::Mon, DayCode::Fri])),
            )
            .unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO,FR", wednesday])
        );
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, body, .. }
                if *original == point("2026-10-28T09:00:00Z") && body["summary"] == "Review"
        ));
        assert_eq!(plan.warnings, [SeriesWarning::NewIdentity]);
    }

    /// Monday 19-10 moved on its own to 11:00.
    fn monday_19_at_eleven() -> Value {
        moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T11:00:00+01:00",
            "2026-10-19T12:00:00+01:00",
        )
    }

    /// The occurrence split at keeps its own time on the new part's same occurrence, but never over
    /// a time the draft gives it.
    #[test]
    fn following_with_a_new_rule_carries_this_ones_time_unless_the_draft_moves_it() {
        let eleven = monday_19_at_eleven();
        let case = Case::new(weekly(), eleven.clone()).exceptions(vec![eleven]);
        let plan = case
            .edit(
                Scope::Following,
                repeat(weekly_on(&[DayCode::Mon, DayCode::Wed])),
            )
            .unwrap();
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, .. } if *original == point("2026-10-19T08:00:00Z")
        ));
        assert_eq!(
            body(&plan, 2)["start"],
            timed_node("2026-10-19T11:00:00+01:00")
        );
        // Moved to 14:00 in the draft: the new part starts then, and nothing puts 11:00 back.
        let two_pm = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Mon, DayCode::Wed]),
            }),
            ..at("14:00", "2026-10-19")
        };
        let plan = case.edit(Scope::Following, two_pm).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            timed_node("2026-10-19T14:00:00+01:00")
        );
        assert!(!plan.steps.iter().any(|s| matches!(s.op, Op::Carry { .. })));
    }

    /// A later occurrence moved on its own keeps its time on the new part; onto a new part made
    /// all-day, with the other kind's key `null`, as Google merges nodes (C11 critique 1 L9).
    #[test]
    fn following_with_a_new_rule_carries_a_later_ones_own_time() {
        let eleven = moved(
            "2026-10-26T09:00:00Z",
            "2026-10-26T11:00:00Z",
            "2026-10-26T12:00:00Z",
        );
        let case = Case::new(weekly(), monday_19()).exceptions(vec![eleven]);
        let plan = case.edit(Scope::Following, repeat(daily())).unwrap();
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, .. } if *original == point("2026-10-26T09:00:00Z")
        ));
        let node = |at: &str| json!({ "dateTime": at, "timeZone": "Europe/London" });
        assert_eq!(body(&plan, 2)["start"], node("2026-10-26T11:00:00Z"));
        assert_eq!(body(&plan, 2)["end"], node("2026-10-26T12:00:00Z"));
        let all_day = EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            recurrence: Some(RepeatDraft::Rule { rule: daily() }),
            ..Default::default()
        };
        let plan = case.edit(Scope::Following, all_day).unwrap();
        assert!(matches!(
            &plan.steps[2].op,
            Op::Carry { original, .. } if *original == Point::Day(day(2026, 10, 26))
        ));
        let crossed =
            |at: &str| json!({ "dateTime": at, "timeZone": "Europe/London", "date": null });
        assert_eq!(body(&plan, 2)["start"], crossed("2026-10-26T11:00:00Z"));
        assert_eq!(body(&plan, 2)["end"], crossed("2026-10-26T12:00:00Z"));
    }

    /// C11 critique 1 L3: an all-day rule is measured by days, so it can't repeat on the day a timed
    /// later part starts, nor past it.
    #[test]
    fn an_all_day_rule_never_repeats_into_a_timed_later_part() {
        let all_day = |ends: RepeatEnds| EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            recurrence: Some(RepeatDraft::Rule {
                rule: rule(RepeatPattern::Daily, 1, ends),
            }),
            ..Default::default()
        };
        let until = |date: &str| RepeatEnds::OnDate { date: date.into() };
        let mut case = Case::new(weekly(), monday_19());
        case.later = LaterPart::At(point("2026-11-02T09:00:00Z"));
        let past = Err(NotPlanned::Refused(SeriesRefusal::RepeatsPastPart(day(
            2026, 11, 2,
        ))));
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(case.edit(scope, all_day(RepeatEnds::Never)), past);
            assert_eq!(case.edit(scope, all_day(until("2026-11-02"))), past);
            assert!(case.edit(scope, all_day(until("2026-11-01"))).is_ok());
        }
    }

    /// C11c review round 4: a timed series made all-day is read by its own wall days, never by the
    /// all-day start's zone. Weekly on Monday at 00:30 in London, Monday 12-10 deleted (Sunday 11-10
    /// 23:30 UTC), Monday 19-10 opened and made all-day on 19-10, "Weekly on Sunday and Monday": the
    /// user moved no day, so nothing warns that the repeat day changes, and the deletion stays on
    /// Monday 12-10, never on Sunday 11-10, which the new rule makes. From this one on, Monday 26-10,
    /// changed on its own, goes onto the new part's Monday 26-10.
    #[test]
    fn a_timed_series_made_all_day_keeps_its_own_days() {
        let half_past_midnight = || {
            master_with(
                &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
                "2026-10-05T00:30:00+01:00",
                "2026-10-05T01:30:00+01:00",
            )
        };
        let all_day = EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Sun, DayCode::Mon]),
            }),
            ..Default::default()
        };
        let no_day_move = |plan: &SeriesPlan| {
            !plan
                .warnings
                .iter()
                .any(|w| matches!(w, SeriesWarning::RepeatDayChanges { .. }))
        };
        let case = Case::new(half_past_midnight(), plain("2026-10-19T00:30:00+01:00"))
            .exceptions(vec![cancelled("2026-10-12T00:30:00+01:00")]);
        let plan = case.edit(Scope::All, all_day.clone()).unwrap();
        let sent = body(&plan, 0);
        assert_eq!(
            sent["start"],
            json!({ "date": "2026-10-05", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            sent["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO,SU",
                "EXDATE;VALUE=DATE:20261012"
            ])
        );
        assert!(no_day_move(&plan));
        let review = with(plain("2026-10-26T00:30:00Z"), "summary", json!("Review"));
        let case = Case::new(half_past_midnight(), plain("2026-10-19T00:30:00+01:00"))
            .exceptions(vec![review]);
        let plan = case.edit(Scope::Following, all_day).unwrap();
        assert_eq!(body(&plan, 0)["start"], json!({ "date": "2026-10-19" }));
        assert_eq!(
            carries(&plan),
            [(Point::Day(day(2026, 10, 26)), json!("Review"))]
        );
        assert!(no_day_move(&plan));
    }

    /// Q3: "after n times" the user set is n occurrences of the new part, whatever the old count.
    #[test]
    fn following_with_a_new_rule_writes_after_n_as_set() {
        let plan = counted_case()
            .edit(
                Scope::Following,
                repeat(rule(
                    RepeatPattern::Weekly {
                        days: vec![DayCode::Tue],
                    },
                    1,
                    RepeatEnds::After { count: 5 },
                )),
            )
            .unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=5"])
        );
    }

    /// C11 critique 1 M4: the series' own count, left as it was, is what's left of it: 10 in all,
    /// two before Monday 19-10.
    #[test]
    fn following_with_an_untouched_count_rebases_it() {
        let plan = counted_case()
            .edit(
                Scope::Following,
                repeat(rule(
                    RepeatPattern::Weekly {
                        days: vec![DayCode::Tue],
                    },
                    1,
                    RepeatEnds::After { count: 10 },
                )),
            )
            .unwrap();
        assert_eq!(
            body(&plan, 0)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=8"])
        );
    }

    /// Weekly on Monday, 10 times, from Monday 19-10, with Google's count of what came before.
    fn counted_case() -> Case {
        let mut case = Case::new(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"]),
            monday_19(),
        );
        case.instances = Some(Listing {
            items: vec![
                plain("2026-10-05T09:00:00+01:00"),
                plain("2026-10-12T09:00:00+01:00"),
                monday_19(),
            ],
            complete: true,
        });
        case
    }

    /// A29: the new part can't start where the part before it still reaches.
    #[test]
    fn following_with_a_new_rule_that_starts_before_the_last_kept_is_refused() {
        let back_a_week = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Mon, DayCode::Wed]),
            }),
            ..at("09:00", "2026-10-12")
        };
        assert_eq!(
            Case::new(weekly(), monday_19()).edit(Scope::Following, back_a_week),
            Err(NotPlanned::Refused(SeriesRefusal::Retime(
                RetimeError::Overlaps(day(2026, 10, 12))
            )))
        );
    }

    /// A29 in the series' own zone, whatever zone the new part moves into (C11c review): from a daily
    /// 23:00 in London, Wednesday 14-10 moved to 06:00 in Tokyo is Tuesday 13-10 22:00 in London, the
    /// day the part before still reaches; 20:00 in New York on the 13th is the 14th in London, past it.
    #[test]
    fn following_into_another_zone_measures_the_part_before_in_its_own() {
        let nights = master_with(
            &["RRULE:FREQ=DAILY"],
            "2026-10-05T23:00:00+01:00",
            "2026-10-06T00:00:00+01:00",
        );
        let case = Case::new(nights, plain("2026-10-14T23:00:00+01:00"));
        let moved_to = |date: &str, start: &str, end: &str, zone: &str| EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: date.into(),
                start_time: start.into(),
                start_zone: zone.into(),
                end_date: date.into(),
                end_time: end.into(),
                end_zone: zone.into(),
            }),
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[
                    DayCode::Mon,
                    DayCode::Tue,
                    DayCode::Wed,
                    DayCode::Thu,
                    DayCode::Fri,
                ]),
            }),
            ..Default::default()
        };
        assert_eq!(
            case.edit(
                Scope::Following,
                moved_to("2026-10-14", "06:00", "07:00", "Asia/Tokyo")
            ),
            Err(NotPlanned::Refused(SeriesRefusal::Retime(
                RetimeError::Overlaps(day(2026, 10, 14))
            )))
        );
        let plan = case
            .edit(
                Scope::Following,
                moved_to("2026-10-13", "20:00", "21:00", "America/New_York"),
            )
            .unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            json!({ "dateTime": "2026-10-13T20:00:00-04:00", "timeZone": "America/New_York" })
        );
        // An all-day series has no zone of its own: made timed, the rest starts on its own date,
        // Wednesday 14-10 at 00:30 in London (Tuesday in UTC), the day after the part before ends.
        let days = with(
            with(
                master(&["RRULE:FREQ=DAILY"]),
                "start",
                json!({ "date": "2026-10-05" }),
            ),
            "end",
            json!({ "date": "2026-10-06" }),
        );
        let wednesday = json!({
            "kind": "calendar#event",
            "id": "abc_20261014",
            "etag": "\"d1\"",
            "status": "confirmed",
            "summary": "Standup",
            "description": "Daily notes",
            "recurringEventId": "abc",
            "originalStartTime": { "date": "2026-10-14" },
            "start": { "date": "2026-10-14" },
            "end": { "date": "2026-10-15" },
            "organizer": { "email": "me@example.com", "self": true },
            "reminders": { "useDefault": true },
        });
        assert!(Case::new(days, wednesday)
            .edit(
                Scope::Following,
                moved_to("2026-10-14", "00:30", "01:30", "Europe/London")
            )
            .is_ok());
    }

    /// §9.5.7: "Does not repeat" from this occurrence on makes one event of it, its own title with
    /// it, and cuts the original; every later occurrence goes, changed or not, as the one warning
    /// says, and nothing is carried.
    #[test]
    fn following_does_not_repeat_inserts_one_event_and_cuts() {
        let retro = with(monday_19(), "summary", json!("Retro"));
        let planning = with(plain("2026-10-26T09:00:00Z"), "summary", json!("Planning"));
        let replied = plain("2026-11-02T09:00:00Z");
        let case = Case::new(weekly(), retro.clone()).exceptions(vec![retro, planning, replied]);
        let plan = case.edit(Scope::Following, stop()).unwrap();
        let inserted = body(&plan, 0);
        assert!(inserted.get("recurrence").is_none());
        assert_eq!(inserted["summary"], "Retro");
        assert_eq!(inserted["start"], timed_node("2026-10-19T09:00:00+01:00"));
        assert_eq!(
            inserted["extendedProperties"]["private"]["pmSplitOf"],
            "abc"
        );
        assert!(matches!(plan.steps[1].op, Op::Patch { .. }));
        assert_eq!(plan.steps.len(), 5);
        for step in &plan.steps[2..] {
            assert!(matches!(step.op, Op::Cancel { .. }) && step.requires.is_none());
        }
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 19)
            }]
        );
        assert!(plan.expect.contains(&Expect::Event { id: TAIL.into() }));
        assert!(!plan
            .expect
            .iter()
            .any(|e| matches!(e, Expect::Present { .. })));
        // The draft's title wins over the occurrence's own.
        let renamed = EventPatchDraft {
            recurrence: Some(RepeatDraft::Once),
            ..title("Wrap-up")
        };
        assert_eq!(
            body(&case.edit(Scope::Following, renamed).unwrap(), 0)["summary"],
            "Wrap-up"
        );
    }

    /// "Does not repeat" from an occurrence moved on its own to 11:00 keeps it at 11:00.
    #[test]
    fn following_does_not_repeat_keeps_this_ones_own_time() {
        let eleven = monday_19_at_eleven();
        let case = Case::new(weekly(), eleven.clone()).exceptions(vec![eleven]);
        let plan = case.edit(Scope::Following, stop()).unwrap();
        let inserted = body(&plan, 0);
        assert_eq!(inserted["start"], timed_node("2026-10-19T11:00:00+01:00"));
        assert_eq!(inserted["end"], timed_node("2026-10-19T12:00:00+01:00"));
        assert!(inserted.get("time").is_none());
        assert_eq!(
            plan.warnings,
            [SeriesWarning::BecomesOneEvent {
                day: day(2026, 10, 19)
            }]
        );
    }

    // --- the guards every series plan keeps, on the rule paths (C11c review) ---

    /// R2: this occurrence moved in Google since the editor opened is a conflict. For every
    /// occurrence even with no time in the draft, since a new rule, or none, sets the times anyway;
    /// from this one on, when the draft gives it a time.
    #[test]
    fn a_rule_change_refuses_this_occurrence_moved_in_google_since() {
        let mut case = Case::new(weekly(), monday_19());
        case.opened_instance = fresh(moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T10:00:00+01:00",
            "2026-10-19T11:00:00+01:00",
        ));
        let conflict = Err(NotPlanned::Conflict(vec!["start".into(), "end".into()]));
        assert_eq!(case.edit(Scope::All, repeat(daily())), conflict);
        assert_eq!(case.edit(Scope::All, stop()), conflict);
        for recurrence in [RepeatDraft::Rule { rule: daily() }, RepeatDraft::Once] {
            let timed = EventPatchDraft {
                recurrence: Some(recurrence),
                ..at("09:00", "2026-10-19")
            };
            assert_eq!(case.edit(Scope::Following, timed), conflict);
        }
    }

    /// R2 from this occurrence on: the series changed in Google since the editor opened, its title or
    /// how it repeats, is a conflict.
    #[test]
    fn a_rule_change_from_here_refuses_a_series_changed_in_google_since() {
        for (key, was) in [
            ("summary", json!("Daily standup")),
            ("recurrence", json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=20"])),
        ] {
            let mut case = Case::new(weekly(), monday_19());
            case.opened_master = fresh(with(weekly(), key, was));
            for draft in [repeat(daily()), stop()] {
                assert_eq!(
                    case.edit(Scope::Following, draft),
                    Err(NotPlanned::Conflict(vec![key.to_string()])),
                    "{key}"
                );
            }
        }
    }

    /// Until C12, a change that would reach an occurrence with guests of its own is refused, for every
    /// occurrence and from one on, a new rule or none.
    #[test]
    fn a_rule_change_refuses_to_reach_an_occurrence_with_guests() {
        let guests = with(
            plain("2026-10-26T09:00:00Z"),
            "attendees",
            json!([{ "email": "ana@example.com" }]),
        );
        let case = Case::new(weekly(), monday_19()).exceptions(vec![guests]);
        let refused = Err(NotPlanned::Refused(SeriesRefusal::GuestsElsewhere(day(
            2026, 10, 26,
        ))));
        for scope in [Scope::All, Scope::Following] {
            for draft in [repeat(daily()), stop()] {
                assert_eq!(case.edit(scope, draft), refused);
            }
        }
    }

    /// A formatted description is never saved over as plain text, whatever the rule.
    #[test]
    fn a_rule_change_keeps_a_formatted_description() {
        let case = Case::new(
            with(weekly(), "description", json!("<b>Agenda</b>")),
            monday_19(),
        );
        for scope in [Scope::All, Scope::Following] {
            for recurrence in [RepeatDraft::Rule { rule: daily() }, RepeatDraft::Once] {
                let notes = EventPatchDraft {
                    description: Some("Notes".into()),
                    recurrence: Some(recurrence),
                    ..Default::default()
                };
                assert_eq!(
                    case.edit(scope, notes),
                    Err(NotPlanned::Refused(SeriesRefusal::FormattedDescription))
                );
            }
        }
    }

    /// The new part from here goes under an id PM minted for it, never the series' own (C8b review
    /// ADV-5).
    #[test]
    fn a_rule_change_from_here_needs_an_id_of_its_own() {
        let case = Case::new(weekly(), monday_19());
        for id in ["abc", ".."] {
            for draft in [repeat(daily()), stop()] {
                assert_eq!(
                    super::super::plan::plan_series_edit(
                        Scope::Following,
                        &case.input(),
                        &draft,
                        id,
                        None
                    ),
                    Err(NotPlanned::Refused(SeriesRefusal::Unreadable)),
                    "{id}"
                );
            }
        }
    }

    /// §9.6: a rule, or none, is never one occurrence's; the series' own rule is no rule change.
    #[test]
    fn this_event_with_a_rule_is_refused_not_no_change() {
        let case = Case::new(weekly(), monday_19());
        for draft in [repeat(weekly_on(&[DayCode::Tue])), stop()] {
            assert_eq!(
                case.edit(Scope::This, draft),
                Err(NotPlanned::Refused(SeriesRefusal::RuleIsSeriesWide))
            );
        }
        let same_rule = EventPatchDraft {
            recurrence: Some(RepeatDraft::Rule {
                rule: weekly_on(&[DayCode::Mon]),
            }),
            ..title("Retro")
        };
        assert!(case.edit(Scope::This, same_rule).is_ok());
        // A rule outside the limits is no one occurrence's either, and every occurrence's refuses it
        // in its own words.
        let bad = repeat(rule(RepeatPattern::Daily, 0, RepeatEnds::Never));
        assert_eq!(
            case.edit(Scope::This, bad.clone()),
            Err(NotPlanned::Refused(SeriesRefusal::RuleIsSeriesWide))
        );
        assert_eq!(
            case.edit(Scope::All, bad),
            Err(NotPlanned::Refused(SeriesRefusal::Spec(
                SpecError::Interval
            )))
        );
    }

    /// The choice reads a rule as the planners do.
    #[test]
    fn a_rule_change_is_read_as_the_planners_read_it() {
        let m = Master::read(&fresh(weekly())).unwrap();
        assert_eq!(rule_change(&title("Retro"), &m), None);
        assert_eq!(rule_change(&repeat(weekly_on(&[DayCode::Mon])), &m), None);
        assert_eq!(
            rule_change(&repeat(weekly_on(&[DayCode::Tue])), &m),
            Some(RuleChange::New)
        );
        assert_eq!(rule_change(&stop(), &m), Some(RuleChange::Stop));
        let bad = repeat(rule(RepeatPattern::Daily, 0, RepeatEnds::Never));
        assert_eq!(rule_change(&bad, &m), Some(RuleChange::New));
    }
}
