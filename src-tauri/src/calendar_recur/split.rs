// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ending a series just before one of its occurrences ("this and following"), and what the rest of it
//! needs. Google documents no split, so this follows the prior art (gcal_sync, Nextcloud calendar-js):
//! the original gets an UNTIL one second before the occurrence (the day before it, for an all-day
//! series) and loses its COUNT, and its EXDATEs and RDATEs from the occurrence on move to the rest,
//! which is a new series. C8's planners put these together; nothing here talks to Google.

use std::cmp::Ordering;

use chrono::Duration;

use super::lines::{self, Kind, Line, Stamp};
use super::{Point, RecurError, SeriesStart};

/// A series cut at one of its occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Truncation {
    /// The occurrence is the series' first, so "this and following" is the whole series.
    First,
    Cut(Cut),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    /// The original's new `recurrence` lines: its rule cut, every untouched line exactly as it was.
    pub lines: Vec<String>,
    /// Its EXDATE and RDATE values from the occurrence on, as lines of their own, for the rest.
    pub moved: Vec<Line>,
}

/// The series' `recurrence` cut to end just before the occurrence at `at` (its `originalStartTime`).
///
/// An UNTIL has the type of the series' start (RFC 5545 §3.3.10): UTC for a timed series, a date for
/// an all-day one, which ends the day before. COUNT goes, as it can't stand beside UNTIL.
///
/// A rule that had already ended before `at` (which is then an extra RDATE) is [`RecurError::Ended`]:
/// cutting it would lengthen the series rather than shorten it. An UNTIL says so itself; a COUNT is
/// judged by `google_before`, how many of the rule's occurrences Google lists before `at` (A7:
/// `events.instances`, cancelled ones in, RDATE-only ones out), so a rule with a COUNT needs it.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C8, the series planners")
)]
pub fn truncate_recurrence<S: AsRef<str>>(
    lines: &[S],
    start: &SeriesStart,
    at: Point,
    google_before: Option<u32>,
) -> Result<Truncation, RecurError> {
    let parsed = lines::parse(lines);
    let (rule, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    let zone = start.zone();
    let until = match (start, at) {
        (SeriesStart::Timed { .. }, Point::At(t)) => Stamp::from_instant(t - Duration::seconds(1)),
        (SeriesStart::AllDay(_), Point::Day(day)) => {
            Stamp::Day(day.pred_opt().ok_or(RecurError::BeforeStart)?)
        }
        _ => return Err(RecurError::Mismatch),
    };
    match at.cmp_in(&start.point().ok_or(RecurError::CannotCount)?, zone) {
        Ordering::Less => return Err(RecurError::BeforeStart),
        Ordering::Equal => return Ok(Truncation::First),
        Ordering::Greater => {}
    }
    if let Some(end) = facts.until {
        let end = end.point(None, start).ok_or(RecurError::CannotCount)?;
        if end.cmp_in(&at, zone) == Ordering::Less {
            return Err(RecurError::Ended);
        }
    }

    if let Some(count) = facts.count {
        if count <= google_before.ok_or(RecurError::CannotCount)? {
            return Err(RecurError::Ended);
        }
    }

    let cut = rule.without("COUNT").with("UNTIL", &until.text()).line();
    let mut kept = Vec::with_capacity(parsed.len());
    let mut moved = Vec::new();
    for line in &parsed {
        let dates = match &line.kind {
            Kind::Rule(_) => {
                kept.push(cut.clone());
                continue;
            }
            Kind::ExDate(dates) | Kind::RDate(dates) => dates,
            // `series_rule` refused both.
            Kind::ExRule | Kind::Unreadable => {
                kept.push(line.raw.clone());
                continue;
            }
        };
        // A value PM can't place can't be left on either side safely: an EXDATE kept on the wrong one
        // brings back an occurrence the user deleted.
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for value in &dates.values {
            match dates.point(value, start) {
                Some(p) if p.cmp_in(&at, zone) == Ordering::Less => before.push(*value),
                Some(_) => after.push(*value),
                None => return Err(RecurError::CannotCount),
            }
        }
        if after.is_empty() {
            kept.push(line.raw.clone());
            continue;
        }
        if !before.is_empty() {
            kept.push(dates.holding(before).line());
        }
        let after = dates.holding(after);
        moved.push(Line {
            raw: after.line(),
            kind: match line.kind {
                Kind::ExDate(_) => Kind::ExDate(after),
                _ => Kind::RDate(after),
            },
        });
    }
    Ok(Truncation::Cut(Cut { lines: kept, moved }))
}

/// The rest of a split series' `recurrence`, unchanged in rule: the original rule with its COUNT set
/// to `remaining` (from [`remaining_count`]) or its UNTIL kept, then the values
/// [`truncate_recurrence`] moved. A new rule, and EXDATEs shifted by a change of time, are C8's.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C8, the series planners")
)]
pub fn tail_recurrence<S: AsRef<str>>(
    lines: &[S],
    remaining: Option<u32>,
    moved: &[Line],
) -> Result<Vec<String>, RecurError> {
    let parsed = lines::parse(lines);
    let (rule, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    let rule = match (facts.count, remaining) {
        (Some(_), Some(n)) => rule.with("COUNT", &n.to_string()),
        (None, None) => rule.clone(),
        // A COUNT with nothing to rebase it on, or a count for a rule that has none.
        _ => return Err(RecurError::CannotCount),
    };
    Ok(std::iter::once(rule.line())
        .chain(moved.iter().map(|l| l.raw.clone()))
        .collect())
}

/// What's left of a COUNT once `before` of its occurrences have gone (Google's number, from
/// `events.instances`): at least one, or the series had already ended.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C8, the series planners")
)]
pub fn remaining_count(count: u32, before: u32) -> Result<u32, RecurError> {
    count
        .checked_sub(before)
        .filter(|n| *n >= 1)
        .ok_or(RecurError::Ended)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_recur::expand::starts;
    use crate::calendar_recur::lines::TextOnly;
    use chrono::{DateTime, NaiveDate, Utc};
    use chrono_tz::Europe::London;

    fn london(y: i32, m: u32, d: u32, h: u32) -> SeriesStart {
        SeriesStart::Timed {
            local: NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap(),
            zone: London,
        }
    }

    fn at(s: &str) -> Point {
        Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn cut(lines: &[&str], start: &SeriesStart, split: Point, before: Option<u32>) -> Cut {
        match truncate_recurrence(lines, start, split, before) {
            Ok(Truncation::Cut(cut)) => cut,
            other => panic!("{other:?}"),
        }
    }

    fn raw(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.raw.as_str()).collect()
    }

    /// T5: the original ends one second before the occurrence, in UTC.
    #[test]
    fn a_timed_series_ends_one_second_before_the_split() {
        let monday = london(2026, 10, 5, 9);
        let split = at("2026-10-12T08:00:00Z");
        assert_eq!(
            cut(&["RRULE:FREQ=WEEKLY;BYDAY=MO"], &monday, split, None).lines,
            ["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261012T075959Z"]
        );
        // COUNT goes; an UNTIL already there is replaced where it stands.
        assert_eq!(
            cut(
                &["RRULE:FREQ=DAILY;COUNT=10"],
                &london(2026, 10, 1, 9),
                at("2026-10-04T08:00:00Z"),
                Some(3)
            )
            .lines,
            ["RRULE:FREQ=DAILY;UNTIL=20261004T075959Z"]
        );
        assert_eq!(
            cut(
                &["RRULE:FREQ=WEEKLY;UNTIL=20261231T235959Z;WKST=SU"],
                &monday,
                split,
                None
            )
            .lines,
            ["RRULE:FREQ=WEEKLY;UNTIL=20261012T075959Z;WKST=SU"]
        );
    }

    /// A rule that ran out before the split (which is then an extra RDATE) has ended, whether by COUNT
    /// or by UNTIL: swapping its COUNT for an UNTIL would lengthen the series, and a "delete this and
    /// following" would add occurrences.
    #[test]
    fn a_rule_that_ended_before_the_split_is_never_cut() {
        let start = london(2026, 10, 1, 9);
        let split = at("2026-10-10T08:00:00Z");
        let by_count = ["RRULE:FREQ=DAILY;COUNT=3", "RDATE:20261010T080000Z"];
        assert_eq!(
            truncate_recurrence(&by_count, &start, split, Some(3)),
            Err(RecurError::Ended)
        );
        let by_until = [
            "RRULE:FREQ=DAILY;UNTIL=20261003T080000Z",
            "RDATE:20261010T080000Z",
        ];
        assert_eq!(
            truncate_recurrence(&by_until, &start, split, None),
            Err(RecurError::Ended)
        );
        // Still running at the split: cut.
        assert!(matches!(
            truncate_recurrence(&by_count, &start, at("2026-10-02T08:00:00Z"), Some(1)),
            Ok(Truncation::Cut(_))
        ));
        // A COUNT can't be judged without Google's count of what came before.
        assert_eq!(
            truncate_recurrence(&by_count, &start, split, None),
            Err(RecurError::CannotCount)
        );
    }

    /// An EXDATE for an occurrence on the night the clocks skip its hour is read as RFC 5545 reads it,
    /// so it moves with that occurrence instead of staying behind and bringing it back.
    #[test]
    fn an_exdate_in_a_skipped_hour_moves_with_its_occurrence() {
        let lines = [
            "RRULE:FREQ=DAILY",
            "EXDATE;TZID=Europe/London:20260329T013000",
        ];
        let start = SeriesStart::Timed {
            local: day(2026, 3, 20).and_hms_opt(1, 30, 0).unwrap(),
            zone: London,
        };
        let result = cut(&lines, &start, at("2026-03-25T01:30:00Z"), None);
        assert_eq!(result.lines, ["RRULE:FREQ=DAILY;UNTIL=20260325T012959Z"]);
        assert_eq!(
            raw(&result.moved),
            ["EXDATE;TZID=Europe/London:20260329T013000"]
        );
        // And it is the instant the rule makes that day.
        assert!(starts(&lines[..1], &start).contains(&at("2026-03-29T01:30:00Z")));
    }

    #[test]
    fn an_all_day_series_ends_the_day_before() {
        assert_eq!(
            cut(
                &["RRULE:FREQ=YEARLY"],
                &SeriesStart::AllDay(day(2026, 3, 1)),
                Point::Day(day(2027, 3, 1)),
                None
            )
            .lines,
            ["RRULE:FREQ=YEARLY;UNTIL=20270228"]
        );
    }

    #[test]
    fn exdates_and_rdates_from_the_split_on_move_to_the_rest() {
        let monday = london(2026, 10, 5, 9);
        let split = at("2026-10-12T08:00:00Z");
        let lines = [
            "EXDATE;TZID=Europe/London:20261005T090000,20261019T090000",
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE:20260928T080000Z",
            "RDATE;TZID=Europe/London:20261014T090000",
        ];
        let result = cut(&lines, &monday, split, None);
        assert_eq!(
            result.lines,
            [
                "EXDATE;TZID=Europe/London:20261005T090000",
                "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261012T075959Z",
                // Untouched: written back exactly.
                "EXDATE:20260928T080000Z",
            ]
        );
        assert_eq!(
            raw(&result.moved),
            [
                "EXDATE;TZID=Europe/London:20261019T090000",
                "RDATE;TZID=Europe/London:20261014T090000",
            ]
        );
        assert!(matches!(result.moved[0].kind, Kind::ExDate(_)));
        assert!(matches!(result.moved[1].kind, Kind::RDate(_)));
        // An EXDATE on the split itself belongs to the rest.
        let on_split = cut(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO", "EXDATE:20261012T080000Z"],
            &monday,
            split,
            None,
        );
        assert_eq!(on_split.lines.len(), 1);
        assert_eq!(raw(&on_split.moved), ["EXDATE:20261012T080000Z"]);
    }

    #[test]
    fn a_split_that_is_no_split_says_why() {
        let monday = london(2026, 10, 5, 9);
        let weekly = ["RRULE:FREQ=WEEKLY"];
        assert_eq!(
            truncate_recurrence(&weekly, &monday, at("2026-10-05T08:00:00Z"), None),
            Ok(Truncation::First)
        );
        assert_eq!(
            truncate_recurrence(&weekly, &monday, at("2026-09-28T08:00:00Z"), None),
            Err(RecurError::BeforeStart)
        );
        assert_eq!(
            truncate_recurrence(
                &["RRULE:FREQ=DAILY;UNTIL=20261001T000000Z"],
                &london(2026, 9, 20, 9),
                at("2026-10-12T08:00:00Z"),
                None
            ),
            Err(RecurError::Ended)
        );
        assert_eq!(
            truncate_recurrence(
                &["RRULE:FREQ=WEEKLY", "EXRULE:FREQ=MONTHLY"],
                &monday,
                at("2026-10-12T08:00:00Z"),
                None
            ),
            Err(RecurError::TextOnly(TextOnly::ExRule))
        );
        assert_eq!(
            truncate_recurrence(&weekly, &monday, Point::Day(day(2026, 10, 12)), None),
            Err(RecurError::Mismatch)
        );
        assert_eq!(
            truncate_recurrence(
                &["RRULE:FREQ=YEARLY"],
                &SeriesStart::AllDay(day(2026, 3, 1)),
                Point::Day(day(2026, 3, 1)),
                None
            ),
            Ok(Truncation::First)
        );
    }

    #[test]
    fn what_is_left_of_a_count() {
        assert_eq!(remaining_count(10, 4), Ok(6));
        assert_eq!(remaining_count(3, 2), Ok(1));
        assert_eq!(remaining_count(3, 3), Err(RecurError::Ended));
        assert_eq!(remaining_count(3, 7), Err(RecurError::Ended));
    }

    #[test]
    fn the_rest_keeps_the_rule_with_its_count_rebased() {
        let moved = cut(
            &["RRULE:FREQ=DAILY;COUNT=10", "EXDATE:20261008T080000Z"],
            &london(2026, 10, 1, 9),
            at("2026-10-05T08:00:00Z"),
            Some(4),
        )
        .moved;
        assert_eq!(
            tail_recurrence(&["RRULE:FREQ=DAILY;COUNT=10"], Some(6), &moved),
            Ok(vec![
                "RRULE:FREQ=DAILY;COUNT=6".to_string(),
                "EXDATE:20261008T080000Z".to_string()
            ])
        );
        assert_eq!(
            tail_recurrence(&["RRULE:FREQ=WEEKLY;UNTIL=20261231T235959Z"], None, &[]),
            Ok(vec!["RRULE:FREQ=WEEKLY;UNTIL=20261231T235959Z".to_string()])
        );
        assert_eq!(
            tail_recurrence(&["RRULE:FREQ=DAILY;COUNT=10"], None, &[]),
            Err(RecurError::CannotCount)
        );
        assert_eq!(
            tail_recurrence(&["RRULE:FREQ=DAILY"], Some(3), &[]),
            Err(RecurError::CannotCount)
        );
    }

    /// The cut original and the rest, expanded, are the original's occurrences exactly: none lost at
    /// the seam, none doubled, a clock change in between.
    #[test]
    fn the_two_halves_of_a_split_are_the_whole_series() {
        let start = london(2026, 10, 20, 9);
        let original = ["RRULE:FREQ=DAILY;COUNT=10"];
        let whole = starts(&original, &start);
        assert_eq!(whole.len(), 10);
        let split = whole[6];
        let Ok(Truncation::Cut(head)) = truncate_recurrence(&original, &start, split, Some(6))
        else {
            panic!()
        };
        let Point::At(t) = split else { panic!() };
        let rest_start = SeriesStart::Timed {
            local: t.with_timezone(&London).naive_local(),
            zone: London,
        };
        let rest = tail_recurrence(
            &original,
            Some(remaining_count(10, 6).unwrap()),
            &head.moved,
        )
        .unwrap();
        let mut joined = starts(&head.lines, &start);
        joined.extend(starts(&rest, &rest_start));
        assert_eq!(joined, whole);
    }
}
