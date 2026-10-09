// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Occurrence maths through the `rrule` crate. Everything the crate is given is built here from chrono
//! values with an explicit zone: it never parses text from PM, so it can't read a date in the
//! computer's own zone (trap b), and PM never asks it to print a rule (trap a).
//!
//! Only the rule goes in. A COUNT counts the starts the rule makes, excluded ones included (RFC 5545
//! §3.8.5.3), and nothing here needs EXDATEs or RDATEs.

use std::cmp::Ordering;

use chrono::{DateTime, Month, NaiveTime, TimeZone, Utc};
use rrule::{Frequency, NWeekday, RRule, RRuleSet};

use super::lines::{self, Freq, RuleFacts, Stamp};
use super::spec::{rule_from_spec, RecurrenceSpec, SpecError};
use super::{end_of_day, first_instant, rfc_instant, Point, RecurError, SeriesStart};

/// How many starts a count walks past before giving up: far more than the 730 Google allows.
const WALK_LIMIT: u32 = 5_000;

/// The rule as the crate's set, from `start`, with the crate's loop guard on (an impossible rule,
/// such as the 30th of every February, would otherwise spin forever).
fn crate_set(facts: &RuleFacts, start: &SeriesStart) -> Option<RRuleSet> {
    let zone = rrule::Tz::Tz(start.zone());
    let dt_start = match start {
        SeriesStart::Timed { local, zone: tz } => first_instant(*tz, *local)?.with_timezone(&zone),
        SeriesStart::AllDay(day) => zone.from_utc_datetime(&day.and_time(NaiveTime::MIN)),
    };
    let mut rule = RRule::new(match facts.freq {
        Freq::Daily => Frequency::Daily,
        Freq::Weekly => Frequency::Weekly,
        Freq::Monthly => Frequency::Monthly,
        Freq::Yearly => Frequency::Yearly,
    })
    .interval(facts.interval);
    if let Some(count) = facts.count {
        rule = rule.count(count);
    }
    if let Some(until) = facts.until {
        // RFC 5545 (and the crate) want UTC here whenever the start names a zone, and it always does.
        rule = rule.until(until_instant(until, start)?.with_timezone(&rrule::Tz::UTC));
    }
    if !facts.by_day.is_empty() {
        rule = rule.by_weekday(
            facts
                .by_day
                .iter()
                .map(|d| match d.nth {
                    Some(n) => NWeekday::Nth(i16::from(n), d.weekday),
                    None => NWeekday::Every(d.weekday),
                })
                .collect(),
        );
    }
    if let Some(day) = facts.by_month_day {
        rule = rule.by_month_day(vec![day]);
    }
    if !facts.by_month.is_empty() {
        let months = facts
            .by_month
            .iter()
            .map(|m| Month::try_from(*m).ok())
            .collect::<Option<Vec<_>>>()?;
        rule = rule.by_month(&months);
    }
    if let Some(week_start) = facts.week_start {
        rule = rule.week_start(week_start);
    }
    rule.build(dt_start).ok().map(RRuleSet::limit)
}

/// An UNTIL as an instant. A day is inclusive: an all-day series' midnight that day (it repeats in
/// UTC), a timed series' last second of it.
fn until_instant(until: Stamp, start: &SeriesStart) -> Option<DateTime<Utc>> {
    match (until, start) {
        (Stamp::Utc(t), _) => Some(Utc.from_utc_datetime(&t)),
        (Stamp::Wall(t), SeriesStart::AllDay(_)) => Some(Utc.from_utc_datetime(&t)),
        (Stamp::Wall(t), SeriesStart::Timed { zone, .. }) => {
            match zone.from_local_datetime(&t).latest() {
                Some(t) => Some(t.with_timezone(&Utc)),
                None => rfc_instant(*zone, t),
            }
        }
        (Stamp::Day(day), SeriesStart::AllDay(_)) => {
            Some(Utc.from_utc_datetime(&day.and_time(NaiveTime::MIN)))
        }
        (Stamp::Day(day), SeriesStart::Timed { zone, .. }) => end_of_day(*zone, day),
    }
}

/// A crate occurrence as a point in the series.
fn point_of(occurrence: DateTime<rrule::Tz>, start: &SeriesStart) -> Point {
    match start {
        SeriesStart::AllDay(_) => Point::Day(occurrence.date_naive()),
        SeriesStart::Timed { .. } => Point::At(occurrence.with_timezone(&Utc)),
    }
}

/// Every start the one rule in `lines` makes from `start`, for tests that compare expansions.
#[cfg(test)]
pub(super) fn starts<S: AsRef<str>>(lines: &[S], start: &SeriesStart) -> Vec<Point> {
    let parsed = lines::parse(lines);
    let (_, facts) = lines::series_rule(&parsed).unwrap();
    let set = crate_set(&facts, start).unwrap();
    (&set)
        .into_iter()
        .take(1_000)
        .map(|o| point_of(o, start))
        .collect()
}

/// How many of the rule's starts come before `at`, when `at` is itself one of them. Anything else,
/// an occurrence the rule doesn't make or one past its end, is [`RecurError::CannotCount`].
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C8, the series planners")
)]
pub fn occurrences_before(
    facts: &RuleFacts,
    start: &SeriesStart,
    at: Point,
) -> Result<u32, RecurError> {
    match (start, at) {
        (SeriesStart::Timed { .. }, Point::At(_)) | (SeriesStart::AllDay(_), Point::Day(_)) => {}
        _ => return Err(RecurError::Mismatch),
    }
    let set = crate_set(facts, start).ok_or(RecurError::CannotCount)?;
    let zone = start.zone();
    let mut before = 0;
    for occurrence in &set {
        match point_of(occurrence, start).cmp_in(&at, zone) {
            Ordering::Less if before < WALK_LIMIT => before += 1,
            Ordering::Equal => return Ok(before),
            _ => break,
        }
    }
    Err(RecurError::CannotCount)
}

/// Whether the crate puts `at` where Google does. `google_before` is how many of the series'
/// occurrences Google lists before it (`events.instances` with cancelled ones, RDATE-only ones left
/// out). PM rebases a COUNT on Google's number and uses this only as a cross-check: when the two
/// disagree (a master whose start isn't on its own rule, which RFC 5545 counts and the crate
/// doesn't, among others) it refuses to split rather than guess.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C8, the series planners")
)]
pub fn crate_agrees(facts: &RuleFacts, start: &SeriesStart, at: Point, google_before: u32) -> bool {
    occurrences_before(facts, start, at) == Ok(google_before)
}

/// The series' first occurrence on or after `start` under `spec`: "Weekly on Monday and Wednesday"
/// chosen on a Tuesday starts on the Wednesday, at the time chosen. RFC 5545 counts a series' start as
/// an occurrence whether or not its rule makes it, and the crate doesn't, so PM never writes a start
/// its own rule doesn't make.
///
/// The day comes from the rule run by days, in UTC, where none is ever skipped: in a zone, the crate
/// moves an occurrence whose time the clocks skip, and drops one they skip at midnight. The time is
/// the one chosen; if the clocks skip it on that day, it's refused, as any new start in a gap is.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C11, repeat rules")
)]
pub fn sync_start(spec: &RecurrenceSpec, start: &SeriesStart) -> Result<SeriesStart, SpecError> {
    let by_day = SeriesStart::AllDay(start.date());
    let rule = rule_from_spec(spec, &by_day)?;
    let lines = lines::parse(&[rule]);
    let (_, facts) = lines::series_rule(&lines).map_err(|_| SpecError::NoOccurrence)?;
    let day = crate_set(&facts, &by_day)
        .and_then(|set| (&set).into_iter().next())
        .ok_or(SpecError::NoOccurrence)?
        .date_naive();
    match *start {
        SeriesStart::AllDay(_) => Ok(SeriesStart::AllDay(day)),
        SeriesStart::Timed { local, zone } => {
            let local = day.and_time(local.time());
            first_instant(zone, local).ok_or(SpecError::Gap(day))?;
            Ok(SeriesStart::Timed { local, zone })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_recur::spec::{Ends, Pattern};
    use chrono::{NaiveDate, Weekday};
    use chrono_tz::Europe::London;

    fn facts(rule: &str) -> RuleFacts {
        let lines = lines::parse(&[rule]);
        lines::series_rule(&lines).unwrap().1
    }

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

    /// T6: the starts before a split, as a COUNT counts them.
    #[test]
    fn starts_before_an_occurrence_are_counted() {
        let daily = facts("RRULE:FREQ=DAILY;COUNT=10");
        let start = london(2026, 10, 1, 9);
        assert_eq!(
            occurrences_before(&daily, &start, at("2026-10-05T09:00:00+01:00")),
            Ok(4)
        );
        assert_eq!(
            occurrences_before(&daily, &start, at("2026-10-01T09:00:00+01:00")),
            Ok(0)
        );
        let mon_wed = facts("RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6");
        assert_eq!(
            occurrences_before(
                &mon_wed,
                &london(2026, 10, 5, 9),
                at("2026-10-14T09:00:00+01:00")
            ),
            Ok(3)
        );
        // A COUNT counts the starts the rule makes, so an excluded one still counts.
        let lines = lines::parse(&[
            "RRULE:FREQ=DAILY;COUNT=10",
            "EXDATE;TZID=Europe/London:20261002T090000",
        ]);
        let (_, with_exdate) = lines::series_rule(&lines).unwrap();
        // By construction: the rule PM counts never holds its EXDATEs.
        assert_eq!(with_exdate, daily);
        assert_eq!(
            occurrences_before(&with_exdate, &start, at("2026-10-05T09:00:00+01:00")),
            Ok(4)
        );
        let three = facts("RRULE:FREQ=DAILY;COUNT=3");
        assert_eq!(
            occurrences_before(&three, &start, at("2026-10-03T09:00:00+01:00")),
            Ok(2)
        );
        // Past the last of a COUNT, and off the rule's own times, there is nothing to count.
        assert_eq!(
            occurrences_before(&three, &start, at("2026-10-04T09:00:00+01:00")),
            Err(RecurError::CannotCount)
        );
        assert_eq!(
            occurrences_before(&daily, &start, at("2026-10-05T10:00:00+01:00")),
            Err(RecurError::CannotCount)
        );
        // An all-day occurrence of a timed series, or the other way round, isn't one.
        assert_eq!(
            occurrences_before(&daily, &start, Point::Day(day(2026, 10, 5))),
            Err(RecurError::Mismatch)
        );
    }

    /// Across the clocks going back on 25-10-2026, a 09:00 series stays at 09:00 on the wall.
    #[test]
    fn a_count_walks_across_a_clock_change_by_wall_time() {
        let five = facts("RRULE:FREQ=DAILY;COUNT=5");
        let start = london(2026, 10, 23, 9);
        assert_eq!(
            occurrences_before(&five, &start, at("2026-10-26T09:00:00Z")),
            Ok(3)
        );
        assert_eq!(
            occurrences_before(&five, &start, at("2026-10-26T09:00:00+01:00")),
            Err(RecurError::CannotCount)
        );
    }

    #[test]
    fn an_all_day_series_counts_days() {
        let five = facts("RRULE:FREQ=DAILY;COUNT=5");
        let start = SeriesStart::AllDay(day(2026, 10, 23));
        assert_eq!(
            occurrences_before(&five, &start, Point::Day(day(2026, 10, 26))),
            Ok(3)
        );
        let until = facts("RRULE:FREQ=WEEKLY;UNTIL=20261106");
        assert_eq!(
            occurrences_before(&until, &start, Point::Day(day(2026, 11, 6))),
            Ok(2)
        );
        assert_eq!(
            occurrences_before(&until, &start, Point::Day(day(2026, 11, 13))),
            Err(RecurError::CannotCount)
        );
    }

    /// The crate cross-checks Google's own count, and refuses on a start that isn't on its rule:
    /// Google counts that start as the first occurrence, the crate never makes it.
    #[test]
    fn the_crate_must_agree_with_google_before_a_count_is_trusted() {
        let daily = facts("RRULE:FREQ=DAILY;COUNT=10");
        let start = london(2026, 10, 1, 9);
        let fifth = at("2026-10-05T09:00:00+01:00");
        assert!(crate_agrees(&daily, &start, fifth, 4));
        assert!(!crate_agrees(&daily, &start, fifth, 5));
        // Weekly on Wednesday from Tuesday 13-10: Google lists Tue 13, Wed 14, then Wed 21.
        let wednesdays = facts("RRULE:FREQ=WEEKLY;BYDAY=WE;COUNT=5");
        let off_rule = london(2026, 10, 13, 9);
        assert!(!crate_agrees(
            &wednesdays,
            &off_rule,
            at("2026-10-21T09:00:00+01:00"),
            2
        ));
    }

    /// An impossible rule ends the walk instead of spinning.
    #[test]
    fn a_rule_with_no_occurrence_is_not_a_hang() {
        let never = facts("RRULE:FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30");
        assert_eq!(
            occurrences_before(&never, &london(2026, 10, 1, 9), at("2027-02-28T09:00:00Z")),
            Err(RecurError::CannotCount)
        );
    }

    fn weekly(days: &[Weekday], ends: Ends) -> RecurrenceSpec {
        RecurrenceSpec {
            pattern: Pattern::Weekly(days.to_vec()),
            interval: 1,
            ends,
        }
    }

    #[test]
    fn a_start_moves_to_the_first_day_its_rule_makes() {
        let mon_wed = weekly(&[Weekday::Mon, Weekday::Wed], Ends::Never);
        assert_eq!(
            sync_start(&mon_wed, &london(2026, 10, 13, 9)),
            Ok(london(2026, 10, 14, 9))
        );
        // Already on the rule: unchanged.
        assert_eq!(
            sync_start(&mon_wed, &london(2026, 10, 12, 9)),
            Ok(london(2026, 10, 12, 9))
        );
        // Across the clock change the wall time holds.
        let sundays = weekly(&[Weekday::Sun], Ends::Never);
        assert_eq!(
            sync_start(&sundays, &london(2026, 10, 20, 9)),
            Ok(london(2026, 10, 25, 9))
        );
        // All-day: Saturday's "every weekday" starts on Monday.
        let weekdays = weekly(
            &[
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            Ends::Never,
        );
        assert_eq!(
            sync_start(&weekdays, &SeriesStart::AllDay(day(2026, 10, 31))),
            Ok(SeriesStart::AllDay(day(2026, 11, 2)))
        );
    }

    const PROBE: &str = "PM_RECUR_ZONE_PROBE";
    const PROBE_LINE: &str = "zone-probe: ";

    /// A spread of the crate maths: timed and all-day, COUNT and both kinds of UNTIL, and a start
    /// moved onto its rule.
    fn probe() -> String {
        let all_day = SeriesStart::AllDay(day(2026, 10, 23));
        let results = [
            occurrences_before(
                &facts("RRULE:FREQ=DAILY;COUNT=5"),
                &all_day,
                Point::Day(day(2026, 10, 26)),
            ),
            occurrences_before(
                &facts("RRULE:FREQ=WEEKLY;UNTIL=20261106"),
                &all_day,
                Point::Day(day(2026, 11, 6)),
            ),
            occurrences_before(
                &facts("RRULE:FREQ=DAILY;UNTIL=20261030T080000Z"),
                &london(2026, 10, 23, 9),
                at("2026-10-26T09:00:00Z"),
            ),
        ];
        let moved = sync_start(
            &weekly(&[Weekday::Mon], Ends::OnDate(day(2026, 12, 31))),
            &all_day,
        );
        format!("{results:?} {moved:?}")
    }

    /// The child half of the next test: in a process started with [`PROBE`] set, prints [`probe`]
    /// worked out in that process's zone. Without it, it checks nothing.
    #[test]
    fn zone_probe() {
        if std::env::var_os(PROBE).is_some() {
            println!("{PROBE_LINE}{}", probe());
        }
    }

    /// Trap (b): the crate reads a value with no zone in the computer's own zone. PM never gives it
    /// one, so the same maths run in a process set to Auckland and in one set to Los Angeles agree
    /// with each other and with this one. (Windows reads its zone from the system, not `TZ`, so there
    /// all three runs are the same zone and agree by default.)
    #[test]
    fn the_computers_own_zone_changes_nothing() {
        let here = probe();
        let exe = std::env::current_exe().unwrap();
        for zone in ["Pacific/Auckland", "America/Los_Angeles"] {
            let out = std::process::Command::new(&exe)
                .args([
                    "calendar_recur::expand::tests::zone_probe",
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
                // The harness prints the test's name first, on the same line.
                .find_map(|l| l.split_once(PROBE_LINE).map(|(_, probe)| probe))
                .unwrap_or_else(|| panic!("no probe output with TZ={zone}: {stdout}"));
            assert_eq!(line, here, "TZ={zone}");
        }
    }

    /// A start moved onto a day where the clocks skip its time is refused, never moved to another
    /// time (the crate would make 01:30 into 02:30) or another week (it drops a day whose time is
    /// skipped at midnight).
    #[test]
    fn a_start_moved_into_a_skipped_hour_is_refused() {
        let at = |y, m, d, h, min, zone| SeriesStart::Timed {
            local: day(y, m, d).and_hms_opt(h, min, 0).unwrap(),
            zone,
        };
        assert_eq!(
            sync_start(
                &weekly(&[Weekday::Sun], Ends::Never),
                &at(2026, 3, 24, 1, 30, London)
            ),
            Err(SpecError::Gap(day(2026, 3, 29)))
        );
        assert_eq!(
            sync_start(
                &weekly(&[Weekday::Fri], Ends::Never),
                &at(2026, 4, 21, 0, 30, chrono_tz::Africa::Cairo)
            ),
            Err(SpecError::Gap(day(2026, 4, 24)))
        );
        // The same day at a time that happens is fine.
        assert_eq!(
            sync_start(
                &weekly(&[Weekday::Sun], Ends::Never),
                &at(2026, 3, 24, 9, 0, London)
            ),
            Ok(london(2026, 3, 29, 9))
        );
    }

    #[test]
    fn a_rule_that_ends_before_its_first_occurrence_is_refused() {
        let wednesday_then_stop = weekly(&[Weekday::Wed], Ends::OnDate(day(2026, 10, 13)));
        assert_eq!(
            sync_start(&wednesday_then_stop, &london(2026, 10, 13, 9)),
            Err(SpecError::NoOccurrence)
        );
        assert_eq!(
            sync_start(&weekly(&[], Ends::Never), &london(2026, 10, 13, 9)),
            Err(SpecError::NoWeekdays)
        );
    }
}
