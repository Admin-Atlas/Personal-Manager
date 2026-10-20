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
    rule_from_spec, spec_of, Ends, Pattern, RecurrenceSpec, SpecError, YearlyOn,
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
    /// How many days the change moves every occurrence (a day move with the new rule, C11 critique
    /// 2 M4): a deleted day is looked for that much later.
    pub shift: i64,
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
    /// The start isn't on the day asked.
    pub moved: bool,
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
    // A deleted day stays deleted wherever the new rule still makes it (from its start on). An
    // EXDATE it doesn't make is never written: Google refuses one that "doesn't point to a valid
    // instance".
    let old_zone = ask.old_start.zone();
    let mut deleted = Vec::new();
    for p in ask.deleted {
        let Some(day) = moved_day(p.day_in(old_zone), ask.shift) else {
            continue;
        };
        let Some(candidate) = on_its_day(&synced, day) else {
            continue;
        };
        if is_occurrence(&facts, &synced, candidate)? {
            deleted.push(candidate);
        }
    }
    // An extra date names a moment of the old series: it stays only where that is still one of
    // this series' times, as a move refuses it today.
    let same_times = ask.shift == 0 && same_wall_time(ask.old_start, &synced);
    if !ask.extra.is_empty() && !same_times {
        return Err(RetimeError::HasExtraDates.into());
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
    })
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
/// day for an all-day one, so an equal day across kinds counts (critique 1 L3).
fn clear_of_later(later: LaterPart, rebuilt: &Rebuilt, old_zone: Tz) -> Result<(), NotPlanned> {
    let at = match later {
        LaterPart::None => return Ok(()),
        LaterPart::Unread => return Err(SeriesRefusal::LaterPartUnread.into()),
        LaterPart::At(at) => at,
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
        Err(SeriesRefusal::RepeatsPastPart(day).into())
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
    // A move of this occurrence to another day moves every occurrence as far, as a move of the
    // series does (C11 critique 2 M4), measured from where it first was.
    let from = original.day_in(zone);
    let shift = asked
        .as_ref()
        .map_or(0, |a| (a.start.date() - from).num_days());
    let (wall, length, end_zone) = match &asked {
        Some(a) => (a.start, a.length, a.end_zone),
        None => (master.start, master.length, master.end_zone),
    };
    let first_day = master.first.day_in(zone);
    let deleted = deleted_points(master, input)?;
    let mut warnings = Vec::new();
    let undo = Some(Undo::RestoreMaster {
        calendar: input.calendar.to_string(),
        event: master.id.clone(),
        body: snapshot(master_json, &["start", "end", "recurrence"]),
    });
    let out_of_range = || NotPlanned::from(SeriesRefusal::Unreadable);
    match ask {
        RuleAsk::New(spec) => {
            let asked_day = moved_day(from, shift).ok_or_else(out_of_range)?;
            let day = match spec.pattern {
                Pattern::Yearly(YearlyOn::Date) => yearly_anchor(first_day, asked_day),
                _ => moved_day(first_day, shift).ok_or_else(out_of_range)?,
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
                shift,
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
            if shift != 0 {
                warnings.push(SeriesWarning::RepeatDayChanges {
                    from,
                    to: asked_day,
                });
            }
            if rebuilt.start.date() != first_day {
                warnings.push(SeriesWarning::StartMoved {
                    from: first_day,
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
                    let draft_keys: Vec<&str> =
                        draft_fields(draft).iter().map(|(k, _)| *k).collect();
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
        mut warnings,
        undo,
        notify,
    } = ask;
    let master_json = input.fresh.master.json();
    let zone = master.start.zone();
    let (first, start_zone, end_zone, length) = match (asked, own_time) {
        (Some(a), _) => {
            let day = moved_day(kept.day_in(zone), shift).ok_or(SeriesRefusal::Unreadable)?;
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
    let mut plan = all_plan(input, master, body, warnings, undo, notify, rule_start)?;
    // Every other occurrence goes, as its own words say; what Google does with the ones changed on
    // their own is P-R2's to show, and their landing keeps any it still shows.
    plan.warnings
        .retain(|w| !matches!(w, SeriesWarning::ExceptionsMayReset { .. }));
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
    let shift = (start.date() - at_day).num_days();
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
    let touched = draft_fields(draft);
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
                shift,
                extra: &extra,
            })?;
            // The rest can't start where the part before it still reaches (A29).
            let head = last_kept(
                master,
                &facts,
                at,
                &cancelled_points(master, input.exceptions)?,
            )?;
            if head.is_some_and(|h| rebuilt.start.date() <= h.day_in(zone)) {
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
                let target = moved_day(p.day_in(zone), shift)
                    .and_then(|day| on_its_day(&rebuilt.start, day));
                let target = match target {
                    Some(q) if is_occurrence(&rebuilt.facts, &rebuilt.start, q)? => Some(q),
                    _ => None,
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
        guests_allowed: input.guests_allowed,
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
    use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern, RepeatRule};
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

    /// `ask` for `spec` from `start` an hour long, the old series starting at `old`, with `deleted`,
    /// no extra dates and no shift.
    fn ask<'a>(
        spec: &'a RecurrenceSpec,
        start: SeriesStart,
        deleted: &'a [Point],
        old: &'a SeriesStart,
    ) -> RebuildAsk<'a> {
        RebuildAsk {
            spec,
            start,
            length: Length::Timed(TimeDelta::hours(1)),
            end_zone: London,
            deleted,
            old_start: old,
            shift: 0,
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
        // A day the change moves every occurrence by moves the deleted day with it.
        let shifted = RebuildAsk {
            shift: 1,
            ..ask(&spec, london(2026, 10, 5, 9), &deleted, &old)
        };
        assert_eq!(
            rebuild(&shifted).unwrap().lines,
            [
                "RRULE:FREQ=DAILY",
                "EXDATE:20261020T080000Z,20261103T090000Z"
            ]
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
        let plan = case.edit(Scope::All, repeat(yearly)).unwrap();
        assert_eq!(
            body(&plan, 0)["start"],
            timed_node("2026-10-12T09:00:00+01:00")
        );
        assert_eq!(body(&plan, 0)["recurrence"], json!(["RRULE:FREQ=YEARLY"]));
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
        // A daily rule names no day: the series' first moves by as much, and a deleted day with it.
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
            json!(["RRULE:FREQ=DAILY", "EXDATE:20261027T090000Z"])
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
        // A colour of its own, which the series' PATCH can't keep, refuses in words.
        let coloured = with(plain("2026-10-05T09:00:00+01:00"), "colorId", json!("5"));
        assert_eq!(
            Case::new(weekly(), monday_19())
                .exceptions(vec![coloured])
                .edit(Scope::All, stop()),
            Err(NotPlanned::Refused(SeriesRefusal::KeptHasOwn(day(
                2026, 10, 5
            ))))
        );
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
