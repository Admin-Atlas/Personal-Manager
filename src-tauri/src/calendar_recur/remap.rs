// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Moving a whole series, or the rest of one, when the user moves one occurrence and picks "all
//! events" or "this and following". Everything that names an occurrence moves with it: the rule's
//! repeat day ([`super::reanchor`]), the series' start, its UNTIL, and its EXDATEs. An EXDATE left
//! where it was removes nothing (Google: it "must point to a valid instance"), so a deleted
//! occurrence would come back.
//!
//! Each old occurrence is mapped on its own ([`Remap`]): its day by the rule's change, then the new
//! wall-clock time of day in the series' zone, read as RFC 5545 reads a skipped hour. The result is
//! then proved occurrence by occurrence against the new series ([`retime`]'s lockstep check, all of a
//! series that ends, its end included), so a move that would lose, add or double an occurrence is
//! refused with the day it concerns rather than sent. Every value written is read back the way it
//! will be read, so an EXDATE or UNTIL can't name a moment an hour off the one meant.

use std::cmp::Ordering;
use std::fmt;

use chrono::{NaiveDate, NaiveTime, TimeDelta};
use chrono_tz::Tz;

use super::expand::{is_occurrence, occurrences_from, until_point};
use super::lines::{self, Kind, Line, Rule, RuleFacts, Stamp};
use super::reanchor::{reanchor, DayMap, ReanchorError};
use super::{first_instant, rfc_instant, Point, RecurError, SeriesStart};

/// How many occurrences of an endless series, old and new, are compared one by one: more than the 730
/// a Google series may have by COUNT. A series that ends is compared to its end, however long.
const LOCKSTEP: usize = 800;

/// Where each occurrence of a series lands after it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Remap {
    pub day: DayMap,
    /// The new wall-clock time of day (a timed series), the same for every occurrence.
    pub time: Option<NaiveTime>,
    /// The zone the series repeats in.
    pub zone: Tz,
}

impl Remap {
    /// Where the old occurrence at `old` lands: its day in the series' zone mapped, then the new time
    /// of day placed with the RFC 5545 reading. Never the old instant's wall time plus a difference,
    /// which is an hour out on the day the clocks skip the series' time. `None` when that month or
    /// year has no such day.
    pub fn point(&self, old: Point) -> Option<Point> {
        match old {
            Point::Day(day) => self.day.day_of(day).map(Point::Day),
            Point::At(t) => {
                let day = self.day.day_of(t.with_timezone(&self.zone).date_naive())?;
                Some(Point::At(rfc_instant(self.zone, day.and_time(self.time?))?))
            }
        }
    }
}

/// Why a series can't be moved as asked. Shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetimeError {
    /// PM shows the series' rule as text only.
    Recur(RecurError),
    Reanchor(ReanchorError),
    /// Between timed and all-day: a different kind of series, not a move.
    KindChange,
    /// The series has extra one-off dates (RDATE), which PM doesn't move.
    HasExtraDates,
    /// The occurrence moved isn't one the series' rule makes (an extra date).
    NotOnRule,
    /// The series starts on this day, which its own rule doesn't make.
    StartNotOnRule(NaiveDate),
    /// The new time happens twice that day, as the clocks go back, and it's the second.
    RepeatedTime(NaiveDate),
    /// The series' new start falls in the hour the clocks skip that day.
    Gap(NaiveDate),
    /// The rest of a split would start on this day, on or before the last of the part before it.
    Overlaps(NaiveDate),
    /// An occurrence (or a deleted one) on this day would have nowhere to go.
    Loses(NaiveDate),
    /// The new series would have an occurrence on this day that the old one didn't.
    Adds(NaiveDate),
    /// PM can't place every occurrence: a value it can't read, or a series too long to walk.
    CannotPlace,
}

impl fmt::Display for RetimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let date = |d: &NaiveDate| d.format("%d-%m-%Y").to_string();
        match self {
            RetimeError::Recur(e) => e.fmt(f),
            RetimeError::Reanchor(e) => e.fmt(f),
            // Shown on the dialog's rows: no "series" or "split" (C9b-1 critique 2 M4). A change of
            // kind or zone rebuilds every occurrence from the rule (C11 §12.4), which needs one PM
            // can read; a move alone never meets this.
            RetimeError::KindChange => write!(
                f,
                "PM can only change every occurrence between all-day and timed, or into another \
                 time zone, when it can read how the event repeats. Change this occurrence on its \
                 own."
            ),
            RetimeError::HasExtraDates => write!(
                f,
                "This repeating event also has dates of its own outside its rule, which PM can't \
                 move with it."
            ),
            RetimeError::NotOnRule => write!(
                f,
                "That occurrence is an extra date, not one the repeat rule makes, so PM can't move \
                 the others by it."
            ),
            RetimeError::StartNotOnRule(d) => write!(
                f,
                "This repeating event starts on {}, a day its own rule doesn't repeat on, so PM \
                 can't move every occurrence with it. Change this occurrence on its own.",
                date(d)
            ),
            RetimeError::RepeatedTime(d) => write!(
                f,
                "On {} that time happens twice, as the clocks go back, and PM can't move these \
                 occurrences to the second one. Pick another time.",
                date(d)
            ),
            RetimeError::Gap(d) => write!(
                f,
                "On {}, the new start falls in the hour the clocks skip.",
                date(d)
            ),
            RetimeError::Overlaps(d) => write!(
                f,
                "That would start the new part on {}, which the earlier part already reaches.",
                date(d)
            ),
            RetimeError::Loses(d) => write!(
                f,
                "The occurrence on {} would have nowhere to go after that move.",
                date(d)
            ),
            RetimeError::Adds(d) => write!(
                f,
                "That move would add an occurrence on {} that the repeating event doesn't have now.",
                date(d)
            ),
            RetimeError::CannotPlace => write!(
                f,
                "PM can't tell where every occurrence would land after that move, so it won't move \
                 them. Change this occurrence on its own."
            ),
        }
    }
}

impl From<RecurError> for RetimeError {
    fn from(e: RecurError) -> Self {
        match e {
            RecurError::TextOnly(why) => RetimeError::Recur(RecurError::TextOnly(why)),
            // The rest are worded for counting and splitting, which a move isn't.
            _ => RetimeError::CannotPlace,
        }
    }
}

/// A series moved: its new start and `recurrence`, and how to place anything else that named one of
/// its occurrences (a cancelled occurrence, a changed one being carried).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retimed {
    /// The series' new first occurrence, as Google's `start` holds it.
    pub start: SeriesStart,
    /// The same, as a point.
    pub first: Point,
    pub lines: Vec<String>,
    pub remap: Remap,
    /// The new rule, read.
    pub facts: RuleFacts,
    old: SeriesStart,
    old_facts: RuleFacts,
    anchor: Point,
    /// The old series from the anchor and the new one from its first, as the lockstep walked them: all
    /// of a series that ends (`whole`), else the first [`LOCKSTEP`] of each.
    olds: Vec<Point>,
    news: Vec<Point>,
    whole: bool,
}

impl Retimed {
    /// Where the old occurrence at `old` is now. `Ok(None)` when `old` was never one of the old
    /// series' occurrences (an EXDATE for it excluded nothing, and still doesn't), or comes before the
    /// anchor (a split leaves those where they are). An error when it was one and has no place in the
    /// new series, or isn't the series' kind of point.
    pub fn moved(&self, old: Point) -> Result<Option<Point>, RetimeError> {
        if matches!(old, Point::At(_)) != matches!(self.first, Point::At(_)) {
            return Err(RetimeError::CannotPlace);
        }
        let zone = self.old.zone();
        if old.cmp_in(&self.anchor, zone) == Ordering::Less
            || !self.made(&self.olds, old, &self.old_facts, &self.old)?
        {
            return Ok(None);
        }
        let lost = RetimeError::Loses(old.day_in(zone));
        let Some(new) = self.remap.point(old) else {
            return Err(lost);
        };
        if !self.made(&self.news, new, &self.facts, &self.start)? {
            return Err(lost);
        }
        Ok(Some(new))
    }

    /// Whether `p` is one of a series' occurrences: from the walked list where it reaches (`walked`
    /// holds every occurrence from its first to its last), else by walking the rule.
    fn made(
        &self,
        walked: &[Point],
        p: Point,
        facts: &RuleFacts,
        start: &SeriesStart,
    ) -> Result<bool, RetimeError> {
        let zone = self.old.zone();
        if self.whole
            || walked
                .last()
                .is_some_and(|l| p.cmp_in(l, zone) != Ordering::Greater)
        {
            return Ok(walked.binary_search_by(|q| q.cmp_in(&p, zone)).is_ok());
        }
        is_occurrence(facts, start, p).map_err(RetimeError::from)
    }
}

/// Move the series whose `recurrence` is `lines` and which starts at `start`, from `anchor` on (its
/// own start for "all events", the split occurrence for the rest of a split), so that its occurrence
/// at `original` (an `originalStartTime`) lands at `new`. For a split, `head` is the last occurrence
/// of the part before it: the rest must start on a later day, or that day would hold both.
///
/// The day moves by what the rule's change implies, measured from the occurrence's original day, so
/// a dragged exception moves the series by where it came from (T7). The time of day is `new`'s.
pub fn retime<S: AsRef<str>>(
    lines: &[S],
    start: &SeriesStart,
    anchor: Point,
    original: Point,
    new: Point,
    head: Option<Point>,
) -> Result<Retimed, RetimeError> {
    let parsed = lines::parse(lines);
    if parsed.iter().any(|l| matches!(l.kind, Kind::RDate(_))) {
        return Err(RetimeError::HasExtraDates);
    }
    let (rule, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    let timed = matches!(start, SeriesStart::Timed { .. });
    if [anchor, original, new]
        .iter()
        .any(|p| matches!(p, Point::At(_)) != timed)
    {
        return Err(RetimeError::KindChange);
    }
    let zone = start.zone();
    // Both the moved occurrence and the first one kept must be the rule's own: an extra date's time
    // isn't the series', and RFC 5545 counts an off-rule start where the walk doesn't (RM11, RM14).
    if !is_occurrence(&facts, start, original)? {
        return Err(RetimeError::NotOnRule);
    }
    if !is_occurrence(&facts, start, anchor)? {
        return Err(RetimeError::StartNotOnRule(anchor.day_in(zone)));
    }
    let time = match new {
        Point::At(t) => Some(t.with_timezone(&zone).time()),
        Point::Day(_) => None,
    };
    let (moved_rule, day) = reanchor(rule, &facts, original.day_in(zone), new.day_in(zone))
        .map_err(RetimeError::Reanchor)?;
    let remap = Remap { day, time, zone };
    if remap.point(original) != Some(new) {
        // `new`'s wall time reads back as its first pass, so `new` is the second.
        return Err(RetimeError::RepeatedTime(new.day_in(zone)));
    }

    // The old series from the anchor on, each occurrence mapped, keeping the day it came from.
    let whole = facts.count.is_some() || facts.until.is_some();
    let olds = occurrences_from(
        &facts,
        start,
        anchor,
        if whole { usize::MAX } else { LOCKSTEP },
    )?;
    let mut images = olds
        .iter()
        .map(|p| {
            remap
                .point(*p)
                .map(|q| (q, p.day_in(zone)))
                .ok_or(RetimeError::Loses(p.day_in(zone)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    images.sort_by(|a, b| a.0.cmp_in(&b.0, zone));

    // The new series starts at its earliest occurrence, which a move of one of several days of the
    // month can put before the anchor's own.
    let first = images
        .first()
        .map(|(p, _)| *p)
        .ok_or(RetimeError::CannotPlace)?;
    let first_day = first.day_in(zone);
    if head.is_some_and(|h| first_day <= h.day_in(zone)) {
        return Err(RetimeError::Overlaps(first_day));
    }
    let new_start = match time {
        Some(time) => {
            let local = first_day.and_time(time);
            first_instant(zone, local).ok_or(RetimeError::Gap(first_day))?;
            SeriesStart::Timed { local, zone }
        }
        None => SeriesStart::AllDay(first_day),
    };

    // A series that ends by UNTIL must end at the image of its last occurrence: the first UNTIL that
    // proves to give the old series mapped, end included, is the one written.
    let untils = match facts.until {
        None => vec![None],
        Some(until) => until_candidates(until, start, &olds, &images)?,
    };
    let mut refused = None;
    for until in untils {
        let new_rule = match until {
            Some(stamp) => moved_rule.with("UNTIL", &stamp.text()),
            None => moved_rule.clone(),
        };
        let new_facts = new_rule.facts().map_err(RecurError::TextOnly)?;
        let news = match lockstep(&images, &new_facts, &new_start, first, whole, day) {
            Ok(news) => news,
            Err(e) => {
                refused.get_or_insert(e);
                continue;
            }
        };
        let retimed = Retimed {
            start: new_start,
            first,
            lines: Vec::new(),
            remap,
            facts: new_facts,
            old: *start,
            old_facts: facts.clone(),
            anchor,
            olds,
            news,
            whole,
        };
        let lines = moved_lines(&parsed, &new_rule, start, anchor, &retimed)?;
        return Ok(Retimed { lines, ..retimed });
    }
    Err(refused.unwrap_or(RetimeError::CannotPlace))
}

/// The UNTILs worth trying for the moved series, in the order preferred: the old one as it was; the
/// old one with its date moved as the last occurrence's day moved (an end-of-day UNTIL stays one);
/// as far past the latest image as the old one was past the old last occurrence (an UNTIL on the last
/// occurrence's own instant stays on it); the latest image itself, which works whenever any does. A
/// new value is written as Google writes one: UTC for a timed series, a date for an all-day one.
fn until_candidates(
    until: Stamp,
    start: &SeriesStart,
    olds: &[Point],
    images: &[(Point, NaiveDate)],
) -> Result<Vec<Option<Stamp>>, RetimeError> {
    let (Some(old_last), Some((last, _))) = (olds.last(), images.last()) else {
        return Err(RetimeError::CannotPlace);
    };
    let zone = start.zone();
    let shift = last.day_in(zone) - old_last.day_in(zone);
    let mut out = vec![until];
    match (until_point(until, start), *old_last, *last) {
        (Some(Point::At(u)), Point::At(l), Point::At(m)) => {
            let wall = u.with_timezone(&zone).naive_local() + shift;
            if let Some(Point::At(t)) = until_point(Stamp::Wall(wall), start) {
                out.push(Stamp::from_instant(t));
            }
            out.push(Stamp::from_instant(m + (u - l)));
            out.push(Stamp::from_instant(m));
        }
        (Some(Point::Day(u)), Point::Day(_), Point::Day(m)) => {
            if let Some(d) = u.checked_add_signed(shift) {
                out.push(Stamp::Day(d));
            }
            out.push(Stamp::Day(m));
        }
        _ => return Err(RetimeError::CannotPlace),
    }
    let mut unique: Vec<Option<Stamp>> = Vec::new();
    for stamp in out {
        if !unique.contains(&Some(stamp)) {
            unique.push(Some(stamp));
        }
    }
    Ok(unique)
}

/// How many days a move can carry one occurrence before an earlier one: under a week when only some
/// of a weekly rule's days move (`Crosses` stops the moving day short of the rule's next other day),
/// within its month for "the nth weekday", never when every occurrence moves alike (a one-day weekly
/// rule moves all its occurrences the same days, however many, so 7 is only conservative there).
fn reorder_days(day: DayMap) -> i64 {
    match day {
        DayMap::Weekday { .. } => 7,
        DayMap::Nth { .. } => 31,
        DayMap::Same | DayMap::Days(_) | DayMap::MonthDay(_) | DayMap::YearDate { .. } => 0,
    }
}

/// The old series' occurrences mapped (`images`, in order) must be exactly the new series' from
/// `first` on: all of both when the series ends (`whole`), else as far as both walks surely reach.
/// The new series' walked occurrences, or the day of the first one lost or added.
fn lockstep(
    images: &[(Point, NaiveDate)],
    new_facts: &RuleFacts,
    new: &SeriesStart,
    first: Point,
    whole: bool,
    day: DayMap,
) -> Result<Vec<Point>, RetimeError> {
    let zone = new.zone();
    let news = occurrences_from(
        new_facts,
        new,
        first,
        if whole { usize::MAX } else { LOCKSTEP },
    )?;
    let mut compared: &[(Point, NaiveDate)] = images;
    let mut against: &[Point] = &news;
    if !whole {
        // A walk stopped at the limit says nothing past its last occurrence, and an old occurrence
        // past it can map up to a week or a month before an image already walked.
        let margin = TimeDelta::days(2 * reorder_days(day));
        let stops = [
            compared
                .last()
                .map(|(p, _)| *p)
                .filter(|_| compared.len() == LOCKSTEP),
            against
                .last()
                .copied()
                .filter(|_| against.len() == LOCKSTEP),
        ];
        if let Some(stop) = stops.into_iter().flatten().min_by(|a, b| a.cmp_in(b, zone)) {
            let cut = match stop {
                Point::At(t) => Point::At(t - margin),
                Point::Day(d) => Point::Day(d.checked_sub_signed(margin).unwrap_or(d)),
            };
            let within = |p: &Point| p.cmp_in(&cut, zone) != Ordering::Greater;
            compared = &compared[..compared.partition_point(|(p, _)| within(p))];
            against = &against[..against.partition_point(within)];
        }
    }
    for i in 0..compared.len().max(against.len()) {
        match (compared.get(i), against.get(i)) {
            (Some((a, _)), Some(b)) if a == b => {}
            (Some((a, from)), Some(b)) if a.cmp_in(b, zone) == Ordering::Less => {
                return Err(RetimeError::Loses(*from))
            }
            (_, Some(b)) => return Err(RetimeError::Adds(b.day_in(zone))),
            (Some((_, from)), None) => return Err(RetimeError::Loses(*from)),
            (None, None) => break,
        }
    }
    Ok(news)
}

/// The series' `recurrence` lines, moved: the rule rewritten, every EXDATE value at or after the
/// anchor moved with its occurrence (one that named no occurrence dropped), the rest as they were. A
/// value whose own form can't name the moved moment (a wall time in the second pass of another zone's
/// repeated hour) is written in UTC on a line of its own, since a UTC value can't share a TZID line.
fn moved_lines(
    parsed: &[Line],
    new_rule: &Rule,
    start: &SeriesStart,
    anchor: Point,
    retimed: &Retimed,
) -> Result<Vec<String>, RetimeError> {
    let zone = start.zone();
    let mut out = Vec::with_capacity(parsed.len());
    let mut in_utc = Vec::new();
    for line in parsed {
        match &line.kind {
            Kind::Rule(_) => out.push(new_rule.line()),
            Kind::ExDate(dates) => {
                let mut kept = Vec::new();
                for value in &dates.values {
                    let old = dates.point(value, start).ok_or(RetimeError::CannotPlace)?;
                    if old.cmp_in(&anchor, zone) == Ordering::Less {
                        kept.push(*value);
                        continue;
                    }
                    let Some(moved) = retimed.moved(old)? else {
                        continue;
                    };
                    let stamp = stamp_like(*value, dates.zone, moved, zone);
                    if dates.point(&stamp, &retimed.start) == Some(moved) {
                        kept.push(stamp);
                    } else {
                        in_utc.push(moved);
                    }
                }
                if !kept.is_empty() {
                    out.push(dates.holding(kept).line());
                }
            }
            // RDATEs were refused; EXRULEs and unreadable lines by `series_rule`.
            _ => out.push(line.raw.clone()),
        }
    }
    out.extend(exdate_lines(&in_utc).into_iter().map(|l| l.raw));
    Ok(out)
}

/// `value` rewritten to name `moved`, in its own form: a date stays a date, a UTC time a UTC time, a
/// wall time a wall time in its line's zone (`line_zone`, else the series'). The caller checks that it
/// reads back.
fn stamp_like(value: Stamp, line_zone: Option<Tz>, moved: Point, zone: Tz) -> Stamp {
    match (value, moved) {
        (Stamp::Day(_), p) => Stamp::Day(p.day_in(zone)),
        (Stamp::Utc(_), Point::At(t)) => Stamp::Utc(t.naive_utc()),
        (Stamp::Wall(_), Point::At(t)) => {
            Stamp::Wall(t.with_timezone(&line_zone.unwrap_or(zone)).naive_local())
        }
        (Stamp::Utc(t), Point::Day(d)) => Stamp::Utc(d.and_time(t.time())),
        (Stamp::Wall(t), Point::Day(d)) => Stamp::Wall(d.and_time(t.time())),
    }
}

/// EXDATE lines for `points`: one for instants in UTC, one of dates for days, each sorted with
/// repeats dropped. Empty for no points.
pub fn exdate_lines(points: &[Point]) -> Vec<Line> {
    let mut instants: Vec<String> = Vec::new();
    let mut days: Vec<String> = Vec::new();
    for p in points {
        match p {
            Point::At(t) => instants.push(t.format("%Y%m%dT%H%M%SZ").to_string()),
            Point::Day(d) => days.push(d.format("%Y%m%d").to_string()),
        }
    }
    let mut raw = Vec::new();
    for (values, head) in [(&mut instants, "EXDATE"), (&mut days, "EXDATE;VALUE=DATE")] {
        values.sort_unstable();
        values.dedup();
        if !values.is_empty() {
            raw.push(format!("{head}:{}", values.join(",")));
        }
    }
    lines::parse(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use chrono_tz::Africa::Cairo;
    use chrono_tz::America::{New_York, Santiago};
    use chrono_tz::Europe::London;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn at(s: &str) -> Point {
        Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    fn timed(y: i32, m: u32, d: u32, h: u32, min: u32, zone: Tz) -> SeriesStart {
        SeriesStart::Timed {
            local: day(y, m, d).and_hms_opt(h, min, 0).unwrap(),
            zone,
        }
    }

    /// `retime` with the series' own start as the anchor ("all events").
    fn all(
        lines: &[&str],
        start: SeriesStart,
        original: Point,
        new: Point,
    ) -> Result<Retimed, RetimeError> {
        retime(lines, &start, start.point().unwrap(), original, new, None)
    }

    // --- T7: the whole series follows one occurrence ---

    #[test]
    fn a_new_time_moves_every_occurrence_and_the_rule_stays() {
        let monday = timed(2026, 10, 5, 9, 0, London);
        let moved = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            monday,
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-12T10:30:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 5, 10, 30, London));
        assert_eq!(moved.lines, ["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        // After the clocks go back it's still 10:30 on the wall.
        assert_eq!(
            moved.remap.point(at("2026-10-26T09:00:00Z")),
            Some(at("2026-10-26T10:30:00Z"))
        );
    }

    #[test]
    fn a_new_day_moves_the_repeat_day_and_the_start() {
        let moved = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-13T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 6, 9, 0, London));
        assert_eq!(moved.lines, ["RRULE:FREQ=WEEKLY;BYDAY=TU"]);
    }

    /// T7 and RM1: on Monday and Wednesday, moving a Wednesday to Thursday leaves the series' start
    /// on its Monday, and an EXDATE on a Monday stays on that Monday; one on a Wednesday follows it.
    #[test]
    fn only_the_moved_repeat_day_takes_its_exdates_with_it() {
        let moved = all(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
                "EXDATE;TZID=Europe/London:20261019T090000,20261021T090000",
            ],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-07T09:00:00+01:00"),
            at("2026-10-08T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 5, 9, 0, London));
        assert_eq!(
            moved.lines,
            [
                "RRULE:FREQ=WEEKLY;BYDAY=MO,TH",
                "EXDATE;TZID=Europe/London:20261019T090000,20261022T090000",
            ]
        );
        // A cancelled Monday occurrence maps to itself, a cancelled Wednesday to its Thursday.
        assert_eq!(
            moved.moved(at("2026-10-26T09:00:00Z")),
            Ok(Some(at("2026-10-26T09:00:00Z")))
        );
        assert_eq!(
            moved.moved(at("2026-10-28T09:00:00Z")),
            Ok(Some(at("2026-10-29T09:00:00Z")))
        );
        // Something that was never an occurrence stays nothing; a date can't name a timed one.
        assert_eq!(moved.moved(at("2026-10-27T09:00:00Z")), Ok(None));
        assert_eq!(
            moved.moved(Point::Day(day(2026, 10, 26))),
            Err(RetimeError::CannotPlace)
        );
        // Past the walked 800, membership is walked for.
        assert_eq!(
            moved.moved(at("2049-06-30T08:00:00Z")),
            Ok(Some(at("2049-07-01T08:00:00Z")))
        );
    }

    #[test]
    fn a_move_onto_or_past_another_repeat_day_or_week_is_refused() {
        let mo_we = ["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"];
        let start = timed(2026, 10, 5, 9, 0, London);
        assert_eq!(
            all(
                &mo_we,
                start,
                at("2026-10-07T09:00:00+01:00"),
                at("2026-10-05T09:00:00+01:00")
            ),
            Err(RetimeError::Reanchor(ReanchorError::Collision(
                chrono::Weekday::Mon
            )))
        );
        assert_eq!(
            all(
                &mo_we,
                start,
                at("2026-10-12T09:00:00+01:00"),
                at("2026-10-15T09:00:00+01:00")
            ),
            Err(RetimeError::Reanchor(ReanchorError::Crosses(
                chrono::Weekday::Wed
            )))
        );
        // Several days every two weeks, one of them moved across the week's start.
        assert_eq!(
            all(
                &["RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,SU"],
                timed(2026, 10, 4, 10, 0, London),
                at("2026-10-18T10:00:00+01:00"),
                at("2026-10-19T10:00:00+01:00")
            ),
            Err(RetimeError::Reanchor(ReanchorError::CrossWeek))
        );
    }

    /// Bobby, 16-10-2026: "every Sunday" dragged to Monday becomes "every Monday", its start and
    /// its cancelled Sundays with it, proved occurrence by occurrence like any other move.
    #[test]
    fn a_sunday_series_moved_to_monday_becomes_a_monday_series() {
        let sunday = timed(2026, 10, 4, 10, 0, London);
        let (from, to) = (
            at("2026-10-18T10:00:00+01:00"),
            at("2026-10-19T10:00:00+01:00"),
        );
        let moved = all(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=SU;COUNT=10",
                "EXDATE;TZID=Europe/London:20261025T100000",
            ],
            sunday,
            from,
            to,
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 5, 10, 0, London));
        assert_eq!(
            moved.lines,
            [
                "RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10",
                "EXDATE;TZID=Europe/London:20261026T100000",
            ]
        );
        assert_eq!(
            moved.remap.day,
            DayMap::Weekday {
                from: chrono::Weekday::Sun,
                days: 1
            }
        );
        // Every two weeks on Sunday: the fortnights move with the start.
        let fortnight = all(&["RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=SU"], sunday, from, to).unwrap();
        assert_eq!(fortnight.start, timed(2026, 10, 5, 10, 0, London));
        assert_eq!(fortnight.lines, ["RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO"]);
        assert_eq!(
            fortnight.moved(at("2026-11-01T10:00:00Z")),
            Ok(Some(at("2026-11-02T10:00:00Z")))
        );
        // Tuesdays and Sundays every week: the Sundays become Mondays, the Tuesdays stay.
        let both = all(&["RRULE:FREQ=WEEKLY;BYDAY=TU,SU"], sunday, from, to).unwrap();
        assert_eq!(both.start, timed(2026, 10, 5, 10, 0, London));
        assert_eq!(both.lines, ["RRULE:FREQ=WEEKLY;BYDAY=TU,MO"]);
        assert_eq!(
            both.moved(at("2026-10-27T10:00:00Z")),
            Ok(Some(at("2026-10-27T10:00:00Z")))
        );
        // A week and a day on: every Monday becomes every Tuesday, a week and a day later.
        let further = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-20T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(further.start, timed(2026, 10, 13, 9, 0, London));
        assert_eq!(further.lines, ["RRULE:FREQ=WEEKLY;BYDAY=TU"]);
    }

    #[test]
    fn a_monthly_day_moves_and_the_start_with_it() {
        let moved = all(
            &["RRULE:FREQ=MONTHLY;BYMONTHDAY=13"],
            timed(2026, 9, 13, 9, 0, London),
            at("2026-10-13T09:00:00+01:00"),
            at("2026-10-14T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 9, 14, 9, 0, London));
        assert_eq!(moved.lines, ["RRULE:FREQ=MONTHLY;BYMONTHDAY=14"]);
    }

    /// T7 and RM5: "the second Tuesday" → "the second Wednesday"; the start re-synced to its month's
    /// second Wednesday, and a September 2027 EXDATE (that month starts on a Wednesday) moved six
    /// days back, not one forward.
    #[test]
    fn an_nth_weekday_series_moves_month_by_month() {
        let moved = all(
            &[
                "RRULE:FREQ=MONTHLY;BYDAY=2TU",
                "EXDATE;TZID=Europe/London:20270914T100000",
            ],
            timed(2026, 10, 13, 10, 0, London),
            at("2026-11-10T10:00:00Z"),
            at("2026-11-11T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 14, 10, 0, London));
        assert_eq!(
            moved.lines,
            [
                "RRULE:FREQ=MONTHLY;BYDAY=2WE",
                "EXDATE;TZID=Europe/London:20270908T100000"
            ]
        );
        let last = all(
            &["RRULE:FREQ=MONTHLY;BYDAY=-1FR", "EXDATE:20270430T090000Z"],
            timed(2026, 10, 30, 10, 0, London),
            at("2026-11-27T10:00:00Z"),
            at("2026-11-28T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(last.start, timed(2026, 10, 31, 10, 0, London));
        assert_eq!(
            last.lines,
            ["RRULE:FREQ=MONTHLY;BYDAY=-1SA", "EXDATE:20270424T090000Z"]
        );
    }

    /// C8a review F5: moving the first Wednesday to the first Sunday on "the first Monday and
    /// Wednesday" puts October's Sunday 04-10 before its Monday 05-10, so the series starts there.
    #[test]
    fn the_series_starts_at_its_earliest_occurrence_after_the_move() {
        let moved = all(
            &["RRULE:FREQ=MONTHLY;BYDAY=1MO,1WE"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-07T09:00:00+01:00"),
            at("2026-10-04T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 4, 9, 0, London));
        assert_eq!(moved.lines, ["RRULE:FREQ=MONTHLY;BYDAY=1MO,1SU"]);
    }

    #[test]
    fn a_yearly_or_daily_series_moves_by_its_date() {
        let yearly = all(
            &["RRULE:FREQ=YEARLY"],
            SeriesStart::AllDay(day(2025, 10, 13)),
            Point::Day(day(2026, 10, 13)),
            Point::Day(day(2026, 10, 20)),
        )
        .unwrap();
        assert_eq!(yearly.start, SeriesStart::AllDay(day(2025, 10, 20)));
        let daily = all(
            &["RRULE:FREQ=DAILY"],
            timed(2026, 10, 1, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-13T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(daily.start, timed(2026, 10, 2, 9, 0, London));
    }

    /// T7: a New York series dropped at 15:00 London on 12-10 is at 10:00 New York.
    #[test]
    fn a_new_time_is_read_in_the_series_zone() {
        let moved = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            timed(2026, 10, 5, 9, 0, New_York),
            at("2026-10-12T09:00:00-04:00"),
            at("2026-10-12T15:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 5, 10, 0, New_York));
    }

    /// T7: a new start the clocks skip is refused.
    #[test]
    fn a_new_start_in_a_skipped_hour_is_refused() {
        assert_eq!(
            all(
                &["RRULE:FREQ=WEEKLY;BYDAY=SU"],
                timed(2026, 3, 29, 9, 0, London),
                at("2026-04-05T09:00:00+01:00"),
                at("2026-04-05T01:30:00+01:00"),
            ),
            Err(RetimeError::Gap(day(2026, 3, 29)))
        );
    }

    /// C8a review crate-6 and V2: the second 01:30 of the night the clocks go back can't be a
    /// series' time, since it reads back as the first; the first one can.
    #[test]
    fn a_time_the_clocks_repeat_is_refused_by_name() {
        let series = ["RRULE:FREQ=WEEKLY;BYDAY=SU"];
        let start = timed(2026, 10, 4, 9, 0, London);
        let original = at("2026-10-25T09:00:00Z");
        let refused = all(&series, start, original, at("2026-10-25T01:30:00Z"));
        assert_eq!(refused, Err(RetimeError::RepeatedTime(day(2026, 10, 25))));
        assert!(refused
            .unwrap_err()
            .to_string()
            .starts_with("On 25-10-2026 that time happens twice"));
        assert!(all(&series, start, original, at("2026-10-25T00:30:00Z")).is_ok());
    }

    /// C8a review V3: a start off the series' own rule is named as the start, not blamed on the
    /// occurrence moved.
    #[test]
    fn a_start_off_its_own_rule_is_named() {
        assert_eq!(
            all(
                &["RRULE:FREQ=WEEKLY;BYDAY=WE"],
                timed(2026, 10, 13, 9, 0, London),
                at("2026-10-21T08:00:00Z"),
                at("2026-10-21T09:00:00Z"),
            ),
            Err(RetimeError::StartNotOnRule(day(2026, 10, 13)))
        );
    }

    /// T7's last row: a moved exception dragged back to its own day moves the series by time only,
    /// because the day is measured from where it came from.
    #[test]
    fn a_dragged_exception_moves_the_series_from_its_original_day() {
        let moved = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-12T11:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.remap.day, DayMap::Same);
        assert_eq!(moved.start, timed(2026, 10, 5, 11, 0, London));
    }

    /// C8a review crate-3: Cairo skips 00:00-01:00 on Friday 30-04-2027. A Friday series moved to
    /// 00:30 keeps that day's occurrence, at 01:30 by the clock (22:30Z the day before), as Google does.
    #[test]
    fn a_move_to_a_time_a_midnight_change_skips_keeps_that_day() {
        let moved = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=FR"],
            timed(2026, 10, 2, 9, 0, Cairo),
            at("2026-10-09T09:00:00+03:00"),
            at("2026-10-09T00:30:00+03:00"),
        )
        .unwrap();
        assert_eq!(
            moved.moved(at("2027-04-30T06:00:00Z")),
            Ok(Some(at("2027-04-29T22:30:00Z")))
        );
    }

    // --- what the lockstep proof refuses ---

    /// RM4/AS7: a move onto a day not every month has would lose those months' occurrences.
    #[test]
    fn a_move_onto_a_day_some_months_lack_is_refused_by_name() {
        assert_eq!(
            all(
                &["RRULE:FREQ=MONTHLY;BYMONTHDAY=28"],
                timed(2026, 10, 28, 10, 0, London),
                at("2026-12-28T10:00:00Z"),
                at("2026-12-31T10:00:00Z"),
            ),
            Err(RetimeError::Loses(day(2026, 11, 28)))
        );
        // RC2: an implied day from a February start can't become the 30th.
        assert_eq!(
            all(
                &["RRULE:FREQ=MONTHLY"],
                timed(2026, 2, 15, 9, 0, London),
                at("2026-10-15T09:00:00+01:00"),
                at("2026-10-30T09:00:00Z"),
            ),
            Err(RetimeError::Loses(day(2026, 2, 15)))
        );
        // AS7: a 29 February series moved to 1 March would repeat every year.
        assert_eq!(
            all(
                &["RRULE:FREQ=YEARLY;COUNT=4"],
                SeriesStart::AllDay(day(2024, 2, 29)),
                Point::Day(day(2028, 2, 29)),
                Point::Day(day(2028, 3, 1)),
            ),
            Err(RetimeError::Adds(day(2025, 3, 1)))
        );
    }

    /// C8a review F2: the rest of a split can't start on or before the last day of the part before
    /// it, even at another time of day: that day would hold both.
    #[test]
    fn the_rest_of_a_split_cant_start_where_the_part_before_it_reaches() {
        let split = at("2026-10-14T09:00:00+01:00");
        let head = Some(at("2026-10-13T09:00:00+01:00"));
        let tail = |new: Point| {
            retime(
                &["RRULE:FREQ=DAILY"],
                &timed(2026, 10, 14, 9, 0, London),
                split,
                split,
                new,
                head,
            )
        };
        assert_eq!(
            tail(at("2026-10-13T09:00:00+01:00")).err(),
            Some(RetimeError::Overlaps(day(2026, 10, 13)))
        );
        assert_eq!(
            tail(at("2026-10-13T20:00:00+01:00")).err(),
            Some(RetimeError::Overlaps(day(2026, 10, 13)))
        );
        assert!(tail(at("2026-10-14T20:00:00+01:00")).is_ok());
        let year = Point::Day(day(2027, 10, 13));
        assert_eq!(
            retime(
                &["RRULE:FREQ=YEARLY"],
                &SeriesStart::AllDay(day(2027, 10, 13)),
                year,
                year,
                Point::Day(day(2026, 10, 13)),
                Some(Point::Day(day(2026, 10, 13))),
            )
            .err(),
            Some(RetimeError::Overlaps(day(2026, 10, 13)))
        );
    }

    // --- UNTIL and EXDATEs follow ---

    /// RM8: an UNTIL on the last occurrence's own instant (as Outlook writes it) keeps that
    /// occurrence when the series moves an hour later.
    #[test]
    fn an_until_on_the_last_occurrence_moves_with_it() {
        let moved = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T090000Z"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-12T10:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(
            moved.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T100000Z"]
        );
        // An end-of-day UNTIL stays the end of a day.
        let end_of_day = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-13T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(
            end_of_day.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=TU;UNTIL=20261201T235959Z"]
        );
    }

    /// C8a review F1 and crate-1: a series that ends is proved to its end. On Monday, Wednesday and
    /// Friday until Thursday 15-01-2032, moving the Wednesdays to Thursdays keeps that UNTIL, since
    /// moving it a day would add Friday 16-01-2032; all 827 occurrences are compared, past the 800 an
    /// endless series stops at.
    #[test]
    fn a_series_that_ends_is_proved_to_its_end() {
        let start = timed(2026, 10, 5, 9, 0, London);
        let (wednesday, thursday) = (
            at("2026-10-14T09:00:00+01:00"),
            at("2026-10-15T09:00:00+01:00"),
        );
        let long = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20320115T235959Z"],
            start,
            wednesday,
            thursday,
        )
        .unwrap();
        assert_eq!(
            long.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=MO,TH,FR;UNTIL=20320115T235959Z"]
        );
        assert_eq!(long.olds.len(), 827);
        assert_eq!(
            long.moved(at("2032-01-14T09:00:00Z")),
            Ok(Some(at("2032-01-15T09:00:00Z")))
        );
        let short = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20261217T235959Z"],
            start,
            wednesday,
            thursday,
        )
        .unwrap();
        assert_eq!(
            short.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=MO,TH,FR;UNTIL=20261217T235959Z"]
        );
        // "Ends Tuesday 20-10" on Monday and Wednesday, the Mondays moved to Tuesdays.
        let tuesday = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261020T225959Z"],
            start,
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-13T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(
            tuesday.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=TU,WE;UNTIL=20261020T225959Z"]
        );
    }

    /// F1 for "the last Friday" → "the last Saturday": April 2027 ends on a Friday, so an UNTIL in
    /// late April, kept or moved, would take in Saturday 24-04. The series ends on its last image.
    #[test]
    fn an_until_that_would_take_in_another_occurrence_ends_on_the_last_image() {
        let moved = all(
            &["RRULE:FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20270428T235959Z"],
            timed(2026, 10, 30, 10, 0, London),
            at("2026-11-27T10:00:00Z"),
            at("2026-11-28T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 31, 10, 0, London));
        assert_eq!(
            moved.lines,
            ["RRULE:FREQ=MONTHLY;BYDAY=-1SA;UNTIL=20270327T100000Z"]
        );
    }

    /// C8a review F4 and V6: an UNTIL in any form moves to the new last occurrence and is written as
    /// Google writes one, UTC on a timed series and a date on an all-day one.
    #[test]
    fn an_until_in_any_form_moves_and_reads_back() {
        // A date on a timed series ends that day in the series' zone.
        let dated = all(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130"],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-13T09:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(
            dated.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=TU;UNTIL=20261201T235959Z"]
        );
        // A wall-clock UNTIL on the night the clocks go back is read as the walk reads it (the later
        // 01:30), which keeps 25-10's 01:45 on its first pass; it moves with that occurrence.
        let wall = all(
            &["RRULE:FREQ=DAILY;UNTIL=20261025T013000"],
            timed(2026, 10, 20, 1, 45, London),
            at("2026-10-21T00:45:00Z"),
            at("2026-10-21T08:00:00Z"),
        )
        .unwrap();
        assert_eq!(wall.lines, ["RRULE:FREQ=DAILY;UNTIL=20261025T094500Z"]);
        // An all-day series keeps a date, and so does its EXDATE.
        let all_day = all(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130",
                "EXDATE;VALUE=DATE:20261019",
            ],
            SeriesStart::AllDay(day(2026, 10, 5)),
            Point::Day(day(2026, 10, 12)),
            Point::Day(day(2026, 10, 13)),
        )
        .unwrap();
        assert_eq!(all_day.start, SeriesStart::AllDay(day(2026, 10, 6)));
        assert_eq!(
            all_day.lines,
            [
                "RRULE:FREQ=WEEKLY;BYDAY=TU;UNTIL=20261201",
                "EXDATE;VALUE=DATE:20261020"
            ]
        );
    }

    /// RM13: on 28-03-2027 a 01:30 London occurrence is at 01:30Z (the clocks skip 01:00-02:00). Its
    /// EXDATE moves through the day it names, not the 02:30 its instant reads as.
    #[test]
    fn an_exdate_on_the_day_the_clocks_skip_moves_with_its_occurrence() {
        let moved = all(
            &["RRULE:FREQ=DAILY", "EXDATE:20270328T013000Z"],
            timed(2027, 3, 20, 1, 30, London),
            at("2027-03-22T01:30:00Z"),
            at("2027-03-22T03:00:00Z"),
        )
        .unwrap();
        assert_eq!(moved.lines, ["RRULE:FREQ=DAILY", "EXDATE:20270328T020000Z"]);
    }

    /// C8a review V1 and F3: a moved EXDATE that its own line's zone could only write as the second
    /// pass of a repeated hour goes on a UTC line of its own, so it still names the moved occurrence.
    #[test]
    fn an_exdate_its_own_zone_cant_name_goes_on_a_utc_line() {
        let new_york = all(
            &[
                "RRULE:FREQ=DAILY",
                "EXDATE;TZID=Europe/London:20261025T010000",
            ],
            timed(2026, 10, 20, 20, 0, New_York),
            at("2026-10-22T00:00:00Z"),
            at("2026-10-22T01:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            new_york.lines,
            ["RRULE:FREQ=DAILY", "EXDATE:20261025T010000Z"]
        );
        let london = all(
            &[
                "RRULE:FREQ=DAILY",
                "EXDATE;TZID=America/New_York:20261101T020000",
            ],
            timed(2026, 10, 26, 7, 0, London),
            at("2026-10-27T07:00:00Z"),
            at("2026-10-27T06:30:00Z"),
        )
        .unwrap();
        assert_eq!(
            london.lines,
            ["RRULE:FREQ=DAILY", "EXDATE:20261101T063000Z"]
        );
    }

    /// An EXDATE that never named an occurrence excluded nothing; it's dropped rather than moved onto
    /// a day where it would.
    #[test]
    fn an_exdate_that_named_nothing_is_dropped() {
        let moved = all(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;TZID=Europe/London:20261020T090000",
            ],
            timed(2026, 10, 5, 9, 0, London),
            at("2026-10-12T09:00:00+01:00"),
            at("2026-10-12T10:00:00+01:00"),
        )
        .unwrap();
        assert_eq!(moved.lines, ["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        // C8a review crate-5: that holds on a Sunday whose midnight Santiago skips.
        let santiago = all(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;TZID=America/Santiago:20270905T090000",
            ],
            timed(2027, 8, 2, 9, 0, Santiago),
            at("2027-08-09T13:00:00Z"),
            at("2027-08-09T14:00:00Z"),
        )
        .unwrap();
        assert_eq!(santiago.lines, ["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
    }

    /// The rest of a split moved: anchored at the split, EXDATEs before it left behind.
    #[test]
    fn the_rest_of_a_split_moves_from_its_first_occurrence() {
        let split = at("2026-10-19T09:00:00+01:00");
        let start = timed(2026, 10, 19, 9, 0, London);
        let head = Some(at("2026-10-12T09:00:00+01:00"));
        let moved = retime(
            &[
                "RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=6",
                "EXDATE:20261012T080000Z,20261026T090000Z",
            ],
            &start,
            split,
            split,
            at("2026-10-20T10:00:00+01:00"),
            head,
        )
        .unwrap();
        assert_eq!(moved.start, timed(2026, 10, 20, 10, 0, London));
        assert_eq!(
            moved.lines,
            [
                "RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=6",
                "EXDATE:20261012T080000Z,20261027T100000Z"
            ]
        );
        assert_eq!(moved.moved(at("2026-10-12T08:00:00Z")), Ok(None));
        // One that ends by UNTIL keeps an end-of-day UNTIL on its new last day.
        let until = retime(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261109T235959Z"],
            &start,
            split,
            split,
            at("2026-10-20T10:00:00+01:00"),
            head,
        )
        .unwrap();
        assert_eq!(
            until.lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=TU;UNTIL=20261110T235959Z"]
        );
    }

    #[test]
    fn what_retime_wont_move() {
        let start = timed(2026, 10, 5, 9, 0, London);
        assert_eq!(
            all(
                &["RRULE:FREQ=WEEKLY;BYDAY=MO", "RDATE:20261028T080000Z"],
                start,
                at("2026-10-12T09:00:00+01:00"),
                at("2026-10-12T10:00:00+01:00"),
            ),
            Err(RetimeError::HasExtraDates)
        );
        assert_eq!(
            all(
                &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
                start,
                at("2026-10-12T09:00:00+01:00"),
                Point::Day(day(2026, 10, 12)),
            ),
            Err(RetimeError::KindChange)
        );
        // Moving by an occurrence the rule doesn't make (an extra date's, or none at all).
        assert_eq!(
            all(
                &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
                start,
                at("2026-10-14T15:00:00+01:00"),
                at("2026-10-14T16:00:00+01:00"),
            ),
            Err(RetimeError::NotOnRule)
        );
        assert_eq!(
            all(
                &["RRULE:FREQ=HOURLY"],
                start,
                at("2026-10-12T09:00:00+01:00"),
                at("2026-10-12T10:00:00+01:00"),
            ),
            Err(RetimeError::Recur(RecurError::TextOnly(
                lines::TextOnly::SubDaily
            )))
        );
        // A series that ends too far off to walk to its end can't be proved, so it isn't moved.
        assert_eq!(
            all(
                &["RRULE:FREQ=DAILY;UNTIL=23001231T235959Z"],
                timed(2026, 1, 1, 9, 0, London),
                at("2026-10-12T09:00:00+01:00"),
                at("2026-10-12T10:00:00+01:00"),
            ),
            Err(RetimeError::CannotPlace)
        );
    }

    /// C8a review V2: a refusal reads as a move, never as the split or count copy.
    #[test]
    fn every_refusal_reads_as_a_move() {
        let d = day(2026, 10, 25);
        for e in [
            RetimeError::KindChange,
            RetimeError::HasExtraDates,
            RetimeError::NotOnRule,
            RetimeError::StartNotOnRule(d),
            RetimeError::RepeatedTime(d),
            RetimeError::Gap(d),
            RetimeError::Overlaps(d),
            RetimeError::Loses(d),
            RetimeError::Adds(d),
            RetimeError::CannotPlace,
            RetimeError::from(RecurError::CannotCount),
            RetimeError::from(RecurError::Mismatch),
        ] {
            let text = e.to_string();
            assert!(!text.contains("split") && !text.contains("left"), "{text}");
        }
        assert_eq!(
            RetimeError::from(RecurError::Ended),
            RetimeError::CannotPlace
        );
    }

    #[test]
    fn exdate_lines_for_points() {
        let lines = exdate_lines(&[
            at("2026-10-26T09:00:00Z"),
            at("2026-10-19T08:00:00Z"),
            at("2026-10-26T09:00:00Z"),
        ]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].raw, "EXDATE:20261019T080000Z,20261026T090000Z");
        assert!(matches!(lines[0].kind, Kind::ExDate(_)));
        let days = exdate_lines(&[Point::Day(day(2026, 10, 26))]);
        assert_eq!(days[0].raw, "EXDATE;VALUE=DATE:20261026");
        let both = exdate_lines(&[Point::Day(day(2026, 10, 26)), at("2026-10-19T08:00:00Z")]);
        let raw: Vec<&str> = both.iter().map(|l| l.raw.as_str()).collect();
        assert_eq!(
            raw,
            ["EXDATE:20261019T080000Z", "EXDATE;VALUE=DATE:20261026"]
        );
        assert!(exdate_lines(&[]).is_empty());
    }
}
