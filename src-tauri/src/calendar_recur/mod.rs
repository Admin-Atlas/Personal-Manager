// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Repeat rules (#884): the `recurrence` lines of a Google event, or an iCal VEVENT's repeat lines,
//! read, described and changed, with no I/O. The read path (an iCal row's "Weekly on Monday") and the
//! write path (C8's series plans) share it, so what PM says a rule means and what it sends can't drift.
//!
//! - [`lines`]: the lines parsed in order, each keeping the exact text it came as, and which rules PM
//!   can change rather than only show.
//! - [`spec`]: the closed set of rules PM's editor offers, read from a rule, written as one in PM's
//!   own canonical form, and put into words.
//! - [`expand`]: occurrence maths through the `rrule` crate.
//! - [`split`]: ending a series just before one occurrence, and what the rest of it needs.
//!
//! Three traps in rrule 0.14 shape this module (each read in the crate's source):
//! - Its `Display` adds BYHOUR, BYMINUTE and BYSECOND to every rule, so a rule it prints pins the
//!   hour and breaks a later time change. PM writes rules itself and never prints one with the crate.
//! - It reads a date, or a time with no zone, in the computer's own zone. PM gives the crate only
//!   values it built with an explicit zone, and never text to parse.
//! - Without its `exrule` feature (on since C7, for the iCal expander) it drops EXRULE with only a
//!   log line. Here a series with one is shown as text and never expanded or changed.

pub mod expand;
pub mod lines;
pub mod spec;
pub mod split;

use std::cmp::Ordering;
use std::fmt;

use chrono::{DateTime, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

/// Where a series starts, as Google's master `start` (or an iCal DTSTART) says: its first occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesStart {
    /// A timed series: the wall-clock start, in the zone it repeats in (Google's `start.timeZone`).
    Timed { local: NaiveDateTime, zone: Tz },
    /// An all-day series: its first day. It names days rather than instants, so it repeats in UTC.
    AllDay(NaiveDate),
}

impl SeriesStart {
    pub fn date(&self) -> NaiveDate {
        match self {
            SeriesStart::Timed { local, .. } => local.date(),
            SeriesStart::AllDay(day) => *day,
        }
    }

    /// The zone the rule repeats in.
    pub fn zone(&self) -> Tz {
        match self {
            SeriesStart::Timed { zone, .. } => *zone,
            SeriesStart::AllDay(_) => Tz::UTC,
        }
    }

    /// The first occurrence as a point in the series, or `None` for a start the clocks skip.
    pub fn point(&self) -> Option<Point> {
        match self {
            SeriesStart::Timed { local, zone } => {
                first_instant(*zone, *local).map(|t| Point::At(t.with_timezone(&Utc)))
            }
            SeriesStart::AllDay(day) => Some(Point::Day(*day)),
        }
    }
}

/// A moment in a series: an instant in a timed one, a day in an all-day one. An occurrence's
/// `originalStartTime`, an EXDATE and an UNTIL are each one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    At(DateTime<Utc>),
    Day(NaiveDate),
}

impl Point {
    /// The day this falls on in `zone`.
    pub fn day_in(&self, zone: Tz) -> NaiveDate {
        match self {
            Point::At(t) => t.with_timezone(&zone).date_naive(),
            Point::Day(day) => *day,
        }
    }

    /// Order within a series repeating in `zone`. Two instants, or two days, compare directly; an
    /// instant against a day compares the day it falls on there.
    pub fn cmp_in(&self, other: &Point, zone: Tz) -> Ordering {
        match (self, other) {
            (Point::At(a), Point::At(b)) => a.cmp(b),
            (Point::Day(a), Point::Day(b)) => a.cmp(b),
            _ => self.day_in(zone).cmp(&other.day_in(zone)),
        }
    }
}

/// The instant a wall-clock time names in `zone`: the first of a repeated hour, `None` for a skipped
/// one. For a series' start, where a skipped time is refused.
pub(crate) fn first_instant(zone: Tz, wall: NaiveDateTime) -> Option<DateTime<Tz>> {
    zone.from_local_datetime(&wall).earliest()
}

/// The instant a date value's wall-clock time names in `zone`, as RFC 5545 §3.3.5 reads one: the first
/// of a repeated hour, and a skipped one with the UTC offset in force just before the clocks jumped.
/// So 01:30 on the night London skips to 02:00 is 01:30 GMT, which is where Google and the crate put
/// that day's 01:30 occurrence, and an EXDATE written for it still finds it.
pub(crate) fn rfc_instant(zone: Tz, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
    if let Some(t) = zone.from_local_datetime(&wall).earliest() {
        return Some(t.with_timezone(&Utc));
    }
    // The nearest earlier wall time that exists carries the offset from before the jump (a gap is an
    // hour or two; Samoa's skipped day, 30-12-2011, is the longest on record).
    let before = (1..=48).find_map(|h| {
        zone.from_local_datetime(&(wall - chrono::Duration::hours(h)))
            .earliest()
    })?;
    before
        .offset()
        .fix()
        .from_local_datetime(&wall)
        .single()
        .map(|t| t.with_timezone(&Utc))
}

/// The last instant of `day` in `zone`: 23:59:59, the later one when that hour repeats, or an hour
/// earlier in the one zone-rule shape where the clocks skip it.
pub(crate) fn end_of_day(zone: Tz, day: NaiveDate) -> Option<DateTime<Utc>> {
    let wall = day.and_hms_opt(23, 59, 59)?;
    let at = match zone.from_local_datetime(&wall) {
        LocalResult::Single(t) => t,
        LocalResult::Ambiguous(a, b) => a.max(b),
        LocalResult::None => zone
            .from_local_datetime(&(wall - chrono::Duration::hours(1)))
            .latest()?,
    };
    Some(at.with_timezone(&Utc))
}

/// Why a series can't be cut or counted as asked. Shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecurError {
    /// PM can only show this series' rule as text.
    TextOnly(lines::TextOnly),
    /// The series had already ended by that occurrence.
    Ended,
    /// The occurrence isn't one the rule makes as PM reads it, so PM won't guess what's left.
    CannotCount,
    /// The occurrence comes before the series starts.
    BeforeStart,
    /// A timed occurrence of an all-day series, or the other way round.
    Mismatch,
}

impl fmt::Display for RecurError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecurError::TextOnly(why) => {
                write!(f, "PM can't change how this series repeats: {why}.")
            }
            RecurError::Ended => write!(f, "This series had already ended by then."),
            RecurError::CannotCount => write!(
                f,
                "PM can't work out how many times this series has left, so it won't split it."
            ),
            RecurError::BeforeStart => write!(f, "That occurrence is before the series starts."),
            RecurError::Mismatch => write!(
                f,
                "That occurrence and its series don't agree on whether they're all-day."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Point {
        Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    #[test]
    fn an_instant_and_a_day_compare_by_the_day_in_the_series_zone() {
        let day = Point::Day(NaiveDate::from_ymd_opt(2026, 10, 13).unwrap());
        // 23:30 UTC on the 12th is already the 13th in Auckland, still the 12th in Los Angeles.
        let late = at("2026-10-12T23:30:00Z");
        assert_eq!(
            late.cmp_in(&day, chrono_tz::Pacific::Auckland),
            Ordering::Equal
        );
        assert_eq!(
            late.cmp_in(&day, chrono_tz::America::Los_Angeles),
            Ordering::Less
        );
        assert_eq!(
            at("2026-10-12T08:00:00Z").cmp_in(&at("2026-10-12T07:59:59Z"), Tz::UTC),
            Ordering::Greater
        );
    }

    #[test]
    fn the_end_of_a_day_is_its_last_second_in_that_zone() {
        let d = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert_eq!(
            end_of_day(chrono_tz::Europe::London, d)
                .unwrap()
                .to_rfc3339(),
            "2026-12-31T23:59:59+00:00"
        );
        assert_eq!(
            end_of_day(chrono_tz::America::New_York, d)
                .unwrap()
                .to_rfc3339(),
            "2027-01-01T04:59:59+00:00"
        );
    }

    /// A skipped wall time in a date value reads with the offset from before the jump (RFC 5545
    /// §3.3.5), including where the jump is at midnight.
    #[test]
    fn a_skipped_wall_time_reads_with_the_offset_before_the_jump() {
        let wall = |y, m, d, h, min| {
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, min, 0)
                .unwrap()
        };
        let utc = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let london = chrono_tz::Europe::London;
        assert_eq!(
            rfc_instant(london, wall(2026, 3, 29, 1, 30)),
            Some(utc("2026-03-29T01:30:00Z"))
        );
        // Times that happen are read as they are; a repeated one takes its first.
        assert_eq!(
            rfc_instant(london, wall(2026, 3, 29, 2, 30)),
            Some(utc("2026-03-29T01:30:00Z"))
        );
        assert_eq!(
            rfc_instant(london, wall(2026, 10, 25, 1, 30)),
            Some(utc("2026-10-25T00:30:00Z"))
        );
        // Egypt's clocks go from 00:00 to 01:00 on Friday 24-04-2026: 00:30 is 00:30 at +02:00.
        assert_eq!(
            rfc_instant(chrono_tz::Africa::Cairo, wall(2026, 4, 24, 0, 30)),
            Some(utc("2026-04-23T22:30:00Z"))
        );
        // A series' start in the gap stays refused.
        assert_eq!(first_instant(london, wall(2026, 3, 29, 1, 30)), None);
    }

    #[test]
    fn a_start_the_clocks_skip_has_no_point() {
        let gap = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let start = SeriesStart::Timed {
            local: gap,
            zone: chrono_tz::Europe::London,
        };
        assert_eq!(start.point(), None);
        assert_eq!(
            SeriesStart::AllDay(gap.date()).point(),
            Some(Point::Day(gap.date()))
        );
    }
}
