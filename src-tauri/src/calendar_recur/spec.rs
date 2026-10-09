// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The rules PM's editor offers: a closed set, read from a rule, written as one, and put into words.
//!
//! The set is Google Calendar's own (its presets and its Custom dialog: every N days, weeks, months or
//! years; days of the week; a day of the month or its first to fourth or last weekday; ending never, on
//! a date or after up to 730 times, Help 37115), plus "the second Tuesday of October" every year. A
//! rule outside it is described as a custom rule and can only be replaced, never edited in part.
//!
//! PM writes a rule in one canonical form, `FREQ;INTERVAL;BYMONTH;BYMONTHDAY;BYDAY;COUNT|UNTIL`, and
//! never through the crate (whose `Display` adds BYHOUR, BYMINUTE and BYSECOND). It writes no WKST:
//! weeks start on Monday, RFC 5545's default.

use std::fmt;

use chrono::{Datelike, NaiveDate, TimeZone, Utc, Weekday};

use super::lines::{self, weekday_code, ByDay, Freq, Kind, RuleFacts, Stamp};
use super::{end_of_day, rfc_instant, SeriesStart};

/// Google's cap on a series' length (Help 37115).
pub const MAX_OCCURRENCES: u16 = 730;
/// The largest "every N" the editor offers.
pub const MAX_INTERVAL: u16 = 99;

/// What a series that isn't in the editor's set reads as.
pub const CUSTOM_RULE: &str = "Repeats (custom rule)";

const WEEKDAYS: [Weekday; 5] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
];

/// A repeat rule from the editor's set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSpec {
    pub pattern: Pattern,
    /// Every this many days, weeks, months or years: 1 to [`MAX_INTERVAL`].
    pub interval: u16,
    pub ends: Ends,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Daily,
    /// On these days of the week, Monday first.
    Weekly(Vec<Weekday>),
    Monthly(MonthlyOn),
    Yearly(YearlyOn),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonthlyOn {
    /// On this day of the month: 1 to 31. A month without it has no occurrence (RFC 5545 §3.3.10).
    Day(u8),
    Weekday(Nth, Weekday),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YearlyOn {
    /// On the series' own start date.
    Date,
    Weekday {
        month: u8,
        nth: Nth,
        weekday: Weekday,
    },
}

/// Which of a month's weekdays: Google's editor offers the first four and the last, never a fifth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nth {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

impl Nth {
    fn from_number(n: i8) -> Option<Nth> {
        Some(match n {
            1 => Nth::First,
            2 => Nth::Second,
            3 => Nth::Third,
            4 => Nth::Fourth,
            -1 => Nth::Last,
            _ => return None,
        })
    }

    fn number(self) -> i8 {
        match self {
            Nth::First => 1,
            Nth::Second => 2,
            Nth::Third => 3,
            Nth::Fourth => 4,
            Nth::Last => -1,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Nth::First => "first",
            Nth::Second => "second",
            Nth::Third => "third",
            Nth::Fourth => "fourth",
            Nth::Last => "last",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ends {
    Never,
    /// The last day an occurrence can start on, in the series' zone.
    OnDate(NaiveDate),
    /// After this many occurrences: 1 to [`MAX_OCCURRENCES`].
    After(u16),
}

/// Why a rule from the editor can't be saved. Shown to the user as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    Interval,
    NoWeekdays,
    DayOfMonth,
    Month,
    Count,
    TooMany,
    EndsBeforeStart,
    /// The rule has no occurrence between its start and its end.
    NoOccurrence,
    /// The clocks skip the chosen time on the series' first day.
    Gap(NaiveDate),
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecError::Interval => write!(f, "Repeat every 1 to {MAX_INTERVAL}."),
            SpecError::NoWeekdays => write!(f, "Pick at least one day of the week."),
            SpecError::DayOfMonth => write!(f, "Pick a day of the month from 1 to 31."),
            SpecError::Month => write!(f, "Pick a month."),
            SpecError::Count => write!(f, "It has to happen at least once."),
            SpecError::TooMany => write!(
                f,
                "Google allows at most {MAX_OCCURRENCES} occurrences in a series."
            ),
            SpecError::EndsBeforeStart => write!(f, "The end date is before the series starts."),
            SpecError::NoOccurrence => write!(
                f,
                "That rule has no occurrence between the start and the end date."
            ),
            SpecError::Gap(day) => write!(
                f,
                "The series would start on {}, when the clocks skip that time. Pick a time outside \
                 that hour.",
                day.format("%d-%m-%Y")
            ),
        }
    }
}

/// The rule as one from the editor's set, or `None` when it isn't one.
pub fn spec_from_rule(f: &RuleFacts, start: &SeriesStart) -> Option<RecurrenceSpec> {
    if f.interval > MAX_INTERVAL {
        return None;
    }
    let ends = match (f.count, f.until) {
        (None, None) => Ends::Never,
        (Some(n), None) => Ends::After(u16::try_from(n).ok().filter(|n| *n <= MAX_OCCURRENCES)?),
        (None, Some(until)) => Ends::OnDate(last_day(until, start).filter(|d| *d >= start.date())?),
        (Some(_), Some(_)) => return None,
    };
    let first = start.date();
    let pattern = match f.freq {
        _ if f.freq != Freq::Yearly && !f.by_month.is_empty() => return None,
        Freq::Daily if f.by_month_day.is_some() => return None,
        Freq::Daily if f.by_day.is_empty() => Pattern::Daily,
        // A daily rule kept to some weekdays, repeating every day, is a weekly one.
        Freq::Daily if f.interval == 1 => Pattern::Weekly(weekdays(&f.by_day)),
        Freq::Daily => return None,
        Freq::Weekly if f.by_month_day.is_some() => return None,
        Freq::Weekly => {
            let days = match f.by_day.as_slice() {
                [] => vec![first.weekday()],
                by_day => weekdays(by_day),
            };
            // The start's own week decides which weeks are on, so its weekday counts too.
            let mut grouped = days.clone();
            grouped.push(first.weekday());
            if f.interval > 1 && straddles(&grouped, f.week_start) {
                return None;
            }
            Pattern::Weekly(days)
        }
        Freq::Monthly => Pattern::Monthly(match (f.by_month_day, f.by_day.as_slice()) {
            (None, []) => MonthlyOn::Day(first.day() as u8),
            (Some(day), []) if day > 0 => MonthlyOn::Day(day as u8),
            (
                None,
                [ByDay {
                    nth: Some(n),
                    weekday,
                }],
            ) => MonthlyOn::Weekday(Nth::from_number(*n)?, *weekday),
            _ => return None,
        }),
        Freq::Yearly => Pattern::Yearly(
            match (f.by_month.as_slice(), f.by_month_day, f.by_day.as_slice()) {
                ([], None, []) => YearlyOn::Date,
                // The start's own month (and day): what the crate, and RFC 5545, would take them as.
                ([m], None, []) if u32::from(*m) == first.month() => YearlyOn::Date,
                ([m], Some(d), [])
                    if u32::from(*m) == first.month() && i64::from(d) == i64::from(first.day()) =>
                {
                    YearlyOn::Date
                }
                (
                    [m],
                    None,
                    [ByDay {
                        nth: Some(n),
                        weekday,
                    }],
                ) => YearlyOn::Weekday {
                    month: *m,
                    nth: Nth::from_number(*n)?,
                    weekday: *weekday,
                },
                _ => return None,
            },
        ),
    };
    Some(RecurrenceSpec {
        pattern,
        interval: f.interval,
        ends,
    })
}

/// The rule as Google gets it: one `RRULE:` line in PM's canonical form, never with BYHOUR, BYMINUTE,
/// BYSECOND or WKST. "Ends on" a day is UNTIL at that day's last second in the series' zone, as UTC
/// (RFC 5545 wants UTC when the start names a zone), or the date itself for an all-day series.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C10/C11, creating and changing rules"
    )
)]
pub fn rule_from_spec(spec: &RecurrenceSpec, start: &SeriesStart) -> Result<String, SpecError> {
    if !(1..=MAX_INTERVAL).contains(&spec.interval) {
        return Err(SpecError::Interval);
    }
    let freq = match spec.pattern {
        Pattern::Daily => "DAILY",
        Pattern::Weekly(_) => "WEEKLY",
        Pattern::Monthly(_) => "MONTHLY",
        Pattern::Yearly(_) => "YEARLY",
    };
    let mut parts = vec![format!("FREQ={freq}")];
    if spec.interval != 1 {
        parts.push(format!("INTERVAL={}", spec.interval));
    }
    let numbered = |nth: Nth, weekday: Weekday| {
        ByDay {
            nth: Some(nth.number()),
            weekday,
        }
        .text()
    };
    match &spec.pattern {
        Pattern::Daily | Pattern::Yearly(YearlyOn::Date) => {}
        Pattern::Weekly(days) => {
            if days.is_empty() {
                return Err(SpecError::NoWeekdays);
            }
            let codes: Vec<&str> = weekdays_sorted(days)
                .into_iter()
                .map(weekday_code)
                .collect();
            parts.push(format!("BYDAY={}", codes.join(",")));
        }
        Pattern::Monthly(MonthlyOn::Day(day)) => {
            if !(1..=31).contains(day) {
                return Err(SpecError::DayOfMonth);
            }
            parts.push(format!("BYMONTHDAY={day}"));
        }
        Pattern::Monthly(MonthlyOn::Weekday(nth, weekday)) => {
            parts.push(format!("BYDAY={}", numbered(*nth, *weekday)));
        }
        Pattern::Yearly(YearlyOn::Weekday {
            month,
            nth,
            weekday,
        }) => {
            if !(1..=12).contains(month) {
                return Err(SpecError::Month);
            }
            parts.push(format!("BYMONTH={month}"));
            parts.push(format!("BYDAY={}", numbered(*nth, *weekday)));
        }
    }
    match spec.ends {
        Ends::Never => {}
        Ends::After(0) => return Err(SpecError::Count),
        Ends::After(n) if n > MAX_OCCURRENCES => return Err(SpecError::TooMany),
        Ends::After(n) => parts.push(format!("COUNT={n}")),
        Ends::OnDate(day) if day < start.date() => return Err(SpecError::EndsBeforeStart),
        Ends::OnDate(day) => parts.push(format!(
            "UNTIL={}",
            match start {
                SeriesStart::AllDay(_) => day.format("%Y%m%d").to_string(),
                // The day's last second, or its occurrence if later: where the clocks skip the
                // evening's last hour (Nuuk's 23:00 on the night they go forward), Google puts a
                // 23:30 occurrence after midnight.
                SeriesStart::Timed { local, zone } => end_of_day(*zone, day)
                    .zip(rfc_instant(*zone, day.and_time(local.time())))
                    .map(|(end, occurrence)| end.max(occurrence))
                    .ok_or(SpecError::EndsBeforeStart)?
                    .format("%Y%m%dT%H%M%SZ")
                    .to_string(),
            }
        )),
    }
    Ok(format!("RRULE:{}", parts.join(";")))
}

/// The rule in words, as Google Calendar says it: "Weekly on Monday and Wednesday", "Monthly on the
/// last Friday, 10 times", "Every 2 years on 13 October, until 31-12-2030".
pub fn describe(spec: &RecurrenceSpec, start: &SeriesStart) -> String {
    let every = |one: &str, unit: &str| match spec.interval {
        1 => one.to_string(),
        n => format!("Every {n} {unit}"),
    };
    let mut text = match &spec.pattern {
        Pattern::Daily => every("Daily", "days"),
        Pattern::Weekly(days) if spec.interval == 1 && weekdays_sorted(days) == WEEKDAYS => {
            "Every weekday (Monday to Friday)".to_string()
        }
        Pattern::Weekly(days) => {
            let names: Vec<&str> = weekdays_sorted(days).into_iter().map(day_name).collect();
            format!("{} on {}", every("Weekly", "weeks"), and_list(&names))
        }
        Pattern::Monthly(MonthlyOn::Day(day)) => {
            format!("{} on day {day}", every("Monthly", "months"))
        }
        Pattern::Monthly(MonthlyOn::Weekday(nth, weekday)) => format!(
            "{} on the {} {}",
            every("Monthly", "months"),
            nth.word(),
            day_name(*weekday)
        ),
        Pattern::Yearly(YearlyOn::Date) => format!(
            "{} on {} {}",
            every("Annually", "years"),
            start.date().day(),
            month_name(start.date().month())
        ),
        Pattern::Yearly(YearlyOn::Weekday {
            month,
            nth,
            weekday,
        }) => format!(
            "{} on the {} {} of {}",
            every("Annually", "years"),
            nth.word(),
            day_name(*weekday),
            month_name(u32::from(*month))
        ),
    };
    match spec.ends {
        Ends::Never => {}
        Ends::After(1) => text.push_str(", once"),
        Ends::After(n) => text.push_str(&format!(", {n} times")),
        Ends::OnDate(day) => text.push_str(&format!(", until {}", day.format("%d-%m-%Y"))),
    }
    text
}

/// How a series repeats, in words, from its `recurrence` lines (or an iCal block's): its one RRULE
/// described, or [`CUSTOM_RULE`] for anything outside the editor's set. EXDATEs and RDATEs don't
/// change the words.
pub fn describe_lines<S: AsRef<str>>(lines: &[S], start: &SeriesStart) -> String {
    let lines = lines::parse(lines);
    let mut rules = lines.iter().filter_map(|l| match &l.kind {
        Kind::Rule(rule) => Some(rule),
        _ => None,
    });
    let spec = match (rules.next(), rules.next()) {
        (Some(rule), None) if !lines.iter().any(|l| l.kind == Kind::ExRule) => rule
            .facts()
            .ok()
            .and_then(|facts| spec_from_rule(&facts, start)),
        _ => None,
    };
    spec.map_or_else(|| CUSTOM_RULE.to_string(), |spec| describe(&spec, start))
}

/// One of the repeat menu's ready-made rules.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C11, repeat rules")
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub label: String,
    pub spec: RecurrenceSpec,
}

/// The repeat menu's ready-made rules for a series starting on `day`, as Google Calendar offers them:
/// daily, weekly on that weekday, monthly on its numbered weekday (the first to the fourth, and also
/// the last when `day` is in its month's last seven days; never a fifth), annually on that date, and
/// every weekday. "Does not repeat" and "Custom…" are the menu's own rows.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C11, repeat rules")
)]
pub fn presets(day: NaiveDate) -> Vec<Preset> {
    let weekday = day.weekday();
    let mut patterns = vec![Pattern::Daily, Pattern::Weekly(vec![weekday])];
    if let Some(nth) = Nth::from_number(((day.day() - 1) / 7 + 1) as i8) {
        patterns.push(Pattern::Monthly(MonthlyOn::Weekday(nth, weekday)));
    }
    if day.day() + 7 > days_in_month(day) {
        patterns.push(Pattern::Monthly(MonthlyOn::Weekday(Nth::Last, weekday)));
    }
    patterns.push(Pattern::Yearly(YearlyOn::Date));
    patterns.push(Pattern::Weekly(WEEKDAYS.to_vec()));
    let start = SeriesStart::AllDay(day);
    patterns
        .into_iter()
        .map(|pattern| {
            let spec = RecurrenceSpec {
                pattern,
                interval: 1,
                ends: Ends::Never,
            };
            Preset {
                label: describe(&spec, &start),
                spec,
            }
        })
        .collect()
}

/// The last day an occurrence can start on under an UNTIL, in the series' zone: the UNTIL's own day
/// when that day's occurrence is no later than it, else the day before (so a split's "one second
/// before" ends on the previous day). The occurrence is placed as Google and RFC 5545 place it, which
/// on the night the clocks skip its hour is not its wall time. An all-day series names days, so only a
/// date UNTIL is one PM can place there; a time (which RFC 5545 doesn't allow) is a custom rule rather
/// than a guess that can be a day out.
fn last_day(until: Stamp, start: &SeriesStart) -> Option<NaiveDate> {
    let SeriesStart::Timed { local, zone } = *start else {
        return match until {
            Stamp::Day(day) => Some(day),
            Stamp::Utc(_) | Stamp::Wall(_) => None,
        };
    };
    let until = match until {
        Stamp::Day(day) => return Some(day),
        Stamp::Utc(t) => Utc.from_utc_datetime(&t),
        // An UNTIL includes all of a repeated hour.
        Stamp::Wall(t) => match zone.from_local_datetime(&t).latest() {
            Some(t) => t.with_timezone(&Utc),
            None => rfc_instant(zone, t)?,
        },
    };
    // Usually the UNTIL's own day or the one before; a day whose occurrence the clocks pushed past
    // midnight can make it the one before that.
    let mut day = until.with_timezone(&zone).date_naive();
    for _ in 0..3 {
        if rfc_instant(zone, day.and_time(local.time()))? <= until {
            return Some(day);
        }
        day = day.pred_opt()?;
    }
    None
}

/// Whether a week starting on `week_start` groups `days` differently from a Monday-start week: it does
/// when some fall before that weekday and some on or after it. PM writes no WKST, so a rule like that,
/// repeating every few weeks, is one it would change by rewriting.
fn straddles(days: &[Weekday], week_start: Option<Weekday>) -> bool {
    let Some(first) = week_start.map(|w| w.num_days_from_monday()) else {
        return false;
    };
    first > 0
        && days.iter().any(|d| d.num_days_from_monday() < first)
        && days.iter().any(|d| d.num_days_from_monday() >= first)
}

fn weekdays(by_day: &[ByDay]) -> Vec<Weekday> {
    weekdays_sorted(&by_day.iter().map(|d| d.weekday).collect::<Vec<_>>())
}

/// Monday first, each once.
fn weekdays_sorted(days: &[Weekday]) -> Vec<Weekday> {
    let mut days = days.to_vec();
    days.sort_by_key(|d| d.num_days_from_monday());
    days.dedup();
    days
}

fn days_in_month(day: NaiveDate) -> u32 {
    let next = match day.month() {
        12 => NaiveDate::from_ymd_opt(day.year() + 1, 1, 1),
        m => NaiveDate::from_ymd_opt(day.year(), m + 1, 1),
    };
    next.and_then(|n| n.pred_opt()).map_or(31, |d| d.day())
}

fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn day_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;
    use chrono_tz::Tz;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn timed(y: i32, m: u32, d: u32, h: u32, zone: Tz) -> SeriesStart {
        SeriesStart::Timed {
            local: day(y, m, d).and_hms_opt(h, 0, 0).unwrap(),
            zone,
        }
    }

    /// Tue 13-10-2026 09:00 in London: the start every T2 row is read against.
    fn tuesday() -> SeriesStart {
        timed(2026, 10, 13, 9, chrono_tz::Europe::London)
    }

    fn words(rule: &str, start: &SeriesStart) -> String {
        describe_lines(&[rule], start)
    }

    fn spec(rule: &str, start: &SeriesStart) -> Option<RecurrenceSpec> {
        let lines = lines::parse(&[rule]);
        let Kind::Rule(rule) = &lines[0].kind else {
            return None;
        };
        spec_from_rule(&rule.facts().ok()?, start)
    }

    /// T2: the rules Google's editor writes, read back as it says them.
    #[test]
    fn rules_read_as_google_says_them() {
        let t = tuesday();
        for (rule, expected) in [
            ("RRULE:FREQ=DAILY", "Daily"),
            (
                "RRULE:FREQ=DAILY;INTERVAL=3;COUNT=10",
                "Every 3 days, 10 times",
            ),
            ("RRULE:FREQ=WEEKLY", "Weekly on Tuesday"),
            ("RRULE:FREQ=WEEKLY;BYDAY=TU", "Weekly on Tuesday"),
            (
                "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
                "Every weekday (Monday to Friday)",
            ),
            (
                "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20261231T235959Z",
                "Every 2 weeks on Monday and Wednesday, until 31-12-2026",
            ),
            (
                "RRULE:FREQ=WEEKLY;BYDAY=FR,MO,WE",
                "Weekly on Monday, Wednesday and Friday",
            ),
            ("RRULE:FREQ=MONTHLY", "Monthly on day 13"),
            ("RRULE:FREQ=MONTHLY;BYMONTHDAY=13", "Monthly on day 13"),
            (
                "RRULE:FREQ=MONTHLY;BYDAY=2TU",
                "Monthly on the second Tuesday",
            ),
            (
                "RRULE:FREQ=MONTHLY;BYDAY=-1TU",
                "Monthly on the last Tuesday",
            ),
            ("RRULE:FREQ=YEARLY", "Annually on 13 October"),
            (
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYMONTHDAY=13",
                "Annually on 13 October",
            ),
            (
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=2TU",
                "Annually on the second Tuesday of October",
            ),
            ("RRULE:FREQ=DAILY;COUNT=1", "Daily, once"),
            (
                "RRULE:FREQ=YEARLY;INTERVAL=2",
                "Every 2 years on 13 October",
            ),
        ] {
            assert_eq!(words(rule, &t), expected, "{rule}");
        }
    }

    #[test]
    fn rules_outside_the_set_read_as_a_custom_rule() {
        let t = tuesday();
        for rule in [
            "RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2",
            "RRULE:FREQ=MONTHLY;BYDAY=5TU",
            "RRULE:FREQ=MONTHLY;BYDAY=TU",
            "RRULE:FREQ=MONTHLY;BYMONTHDAY=-1",
            "RRULE:FREQ=YEARLY;BYMONTHDAY=13",
            "RRULE:FREQ=YEARLY;BYMONTH=11",
            "RRULE:FREQ=DAILY;INTERVAL=2;BYDAY=MO,WE",
            "RRULE:FREQ=DAILY;INTERVAL=100",
            "RRULE:FREQ=DAILY;COUNT=731",
            "RRULE:FREQ=HOURLY",
            "not a rule",
        ] {
            assert_eq!(spec(rule, &t), None, "{rule}");
            assert_eq!(words(rule, &t), CUSTOM_RULE, "{rule}");
        }
        assert_eq!(words("RRULE:FREQ=DAILY", &t), "Daily");
        // A series with an EXRULE, or with two rules, is custom whatever its rules say.
        for lines in [
            ["RRULE:FREQ=DAILY", "EXRULE:FREQ=WEEKLY;BYDAY=SA,SU"],
            ["RRULE:FREQ=DAILY", "RRULE:FREQ=WEEKLY"],
        ] {
            assert_eq!(describe_lines(&lines, &t), CUSTOM_RULE);
        }
        // EXDATEs and RDATEs, even ones PM can't read, leave the words alone.
        assert_eq!(
            describe_lines(
                &[
                    "EXDATE;VALUE=DATE:20261020",
                    "RDATE;VALUE=PERIOD:x",
                    "RRULE:FREQ=DAILY"
                ],
                &t
            ),
            "Daily"
        );
    }

    /// An UNTIL is shown as the last day in the series' zone an occurrence can start on.
    #[test]
    fn an_until_reads_as_the_last_day_in_the_series_zone() {
        let ny = timed(2026, 10, 13, 9, chrono_tz::America::New_York);
        assert_eq!(
            words("RRULE:FREQ=DAILY;UNTIL=20270101T045959Z", &ny),
            "Daily, until 31-12-2026"
        );
        // A split's "one second before" the 12-10 09:00 occurrence ends on the day before it.
        let london = timed(2026, 10, 5, 9, chrono_tz::Europe::London);
        assert_eq!(
            words("RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261012T075959Z", &london),
            "Weekly on Monday, until 11-10-2026"
        );
        assert_eq!(
            words(
                "RRULE:FREQ=DAILY;UNTIL=20270630",
                &SeriesStart::AllDay(day(2026, 10, 13))
            ),
            "Daily, until 30-06-2027"
        );
        // An UNTIL before the start is no rule the editor could have written.
        assert_eq!(
            words("RRULE:FREQ=DAILY;UNTIL=20261001T000000Z", &tuesday()),
            CUSTOM_RULE
        );
        // London skips 01:00-02:00 on 29-03-2026, so that day's 01:30 is at 01:30 UTC (02:30 BST). A
        // split there ends one second before it, which is 02:29:59 on the wall: still the 28th's series.
        let small_hours = SeriesStart::Timed {
            local: day(2026, 3, 22).and_hms_opt(1, 30, 0).unwrap(),
            zone: chrono_tz::Europe::London,
        };
        assert_eq!(
            words("RRULE:FREQ=DAILY;UNTIL=20260329T012959Z", &small_hours),
            "Daily, until 28-03-2026"
        );
        assert_eq!(
            words("RRULE:FREQ=DAILY;UNTIL=20260329T013000Z", &small_hours),
            "Daily, until 29-03-2026"
        );
        // Nuuk skips 23:00-23:59 on Saturday 28-03-2026, so that day's 23:30 is 01:30 UTC on the
        // 29th. A split just before it ends on the 27th, two days before the UNTIL's own day there.
        let nuuk = chrono_tz::America::Nuuk;
        let late = SeriesStart::Timed {
            local: day(2026, 3, 20).and_hms_opt(23, 30, 0).unwrap(),
            zone: nuuk,
        };
        assert_eq!(
            words("RRULE:FREQ=DAILY;UNTIL=20260329T012959Z", &late),
            "Daily, until 27-03-2026"
        );
        // And "ends on the 28th" keeps the 28th's occurrence, after midnight as it is.
        let saturdays = RecurrenceSpec {
            pattern: Pattern::Weekly(vec![Weekday::Sat]),
            interval: 1,
            ends: Ends::OnDate(day(2026, 3, 28)),
        };
        let saturday_late = SeriesStart::Timed {
            local: day(2026, 3, 7).and_hms_opt(23, 30, 0).unwrap(),
            zone: nuuk,
        };
        let rule = rule_from_spec(&saturdays, &saturday_late).unwrap();
        assert_eq!(rule, "RRULE:FREQ=WEEKLY;BYDAY=SA;UNTIL=20260329T013000Z");
        assert_eq!(spec(&rule, &saturday_late), Some(saturdays));
        // An all-day series with a timed UNTIL (which RFC 5545 doesn't allow) can't be placed to the
        // day without guessing a zone.
        assert_eq!(
            words(
                "RRULE:FREQ=WEEKLY;UNTIL=20261028T230000Z",
                &SeriesStart::AllDay(day(2026, 10, 1))
            ),
            CUSTOM_RULE
        );
    }

    /// A WKST matters only to a rule repeating every few weeks whose days it groups differently.
    #[test]
    fn a_week_start_other_than_monday_is_kept_only_where_it_changes_nothing() {
        let t = tuesday();
        assert_eq!(
            words("RRULE:FREQ=WEEKLY;WKST=SU;INTERVAL=2;BYDAY=MO,WE", &t),
            "Every 2 weeks on Monday and Wednesday"
        );
        assert_eq!(
            words("RRULE:FREQ=WEEKLY;WKST=SU;BYDAY=SU,MO", &t),
            "Weekly on Monday and Sunday"
        );
        assert_eq!(
            words("RRULE:FREQ=WEEKLY;WKST=SU;INTERVAL=2;BYDAY=SU,MO", &t),
            CUSTOM_RULE
        );
        // The start's week decides which fortnights are on: from Sunday 11-10 a Sunday-start week holds
        // Mon 12 and Wed 14, a Monday-start one skips to Mon 19. Rewriting it would move every one.
        let sunday = timed(2026, 10, 11, 9, chrono_tz::Europe::London);
        assert_eq!(
            words("RRULE:FREQ=WEEKLY;WKST=SU;INTERVAL=2;BYDAY=MO,WE", &sunday),
            CUSTOM_RULE
        );
        assert_eq!(
            words("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE", &sunday),
            "Every 2 weeks on Monday and Wednesday"
        );
    }

    #[test]
    fn a_daily_rule_kept_to_weekdays_is_a_weekly_one() {
        assert_eq!(
            words("RRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR", &tuesday()),
            "Every weekday (Monday to Friday)"
        );
    }

    fn labels(d: NaiveDate) -> Vec<String> {
        presets(d).into_iter().map(|p| p.label).collect()
    }

    /// T3: the ready-made rules for a start date, as Google's menu offers them.
    #[test]
    fn presets_follow_the_start_date() {
        assert_eq!(
            labels(day(2026, 10, 13)),
            [
                "Daily",
                "Weekly on Tuesday",
                "Monthly on the second Tuesday",
                "Annually on 13 October",
                "Every weekday (Monday to Friday)",
            ]
        );
        // In the month's last seven days the last weekday is offered as well.
        assert_eq!(
            labels(day(2026, 10, 27))[2..4],
            [
                "Monthly on the fourth Tuesday",
                "Monthly on the last Tuesday"
            ]
        );
        // A fifth Saturday is only ever "the last" one.
        assert_eq!(
            labels(day(2026, 10, 31)),
            [
                "Daily",
                "Weekly on Saturday",
                "Monthly on the last Saturday",
                "Annually on 31 October",
                "Every weekday (Monday to Friday)",
            ]
        );
        assert_eq!(labels(day(2028, 2, 29))[3], "Annually on 29 February");
    }

    #[test]
    fn every_preset_writes_a_rule_that_reads_back_the_same() {
        for d in [
            day(2026, 10, 13),
            day(2026, 10, 27),
            day(2026, 10, 31),
            day(2028, 2, 29),
        ] {
            let start = SeriesStart::AllDay(d);
            for preset in presets(d) {
                let rule = rule_from_spec(&preset.spec, &start).unwrap();
                assert_eq!(spec(&rule, &start), Some(preset.spec.clone()), "{rule}");
                assert_eq!(words(&rule, &start), preset.label, "{rule}");
            }
        }
    }

    fn weekly(days: &[Weekday], interval: u16, ends: Ends) -> RecurrenceSpec {
        RecurrenceSpec {
            pattern: Pattern::Weekly(days.to_vec()),
            interval,
            ends,
        }
    }

    /// T4: the exact rule PM writes for each kind of spec.
    #[test]
    fn rules_are_written_in_one_canonical_form() {
        let mon_wed = weekly(
            &[Weekday::Wed, Weekday::Mon],
            2,
            Ends::OnDate(day(2026, 12, 31)),
        );
        assert_eq!(
            rule_from_spec(&mon_wed, &tuesday()).unwrap(),
            "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20261231T235959Z"
        );
        assert_eq!(
            rule_from_spec(
                &mon_wed,
                &timed(2026, 10, 13, 9, chrono_tz::America::New_York)
            )
            .unwrap(),
            "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20270101T045959Z"
        );
        let all_day = SeriesStart::AllDay(day(2026, 10, 13));
        let cases = [
            (Pattern::Daily, Ends::After(5), "RRULE:FREQ=DAILY;COUNT=5"),
            (
                Pattern::Monthly(MonthlyOn::Weekday(Nth::Last, Weekday::Fri)),
                Ends::OnDate(day(2027, 6, 30)),
                "RRULE:FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20270630",
            ),
            (
                Pattern::Monthly(MonthlyOn::Day(13)),
                Ends::Never,
                "RRULE:FREQ=MONTHLY;BYMONTHDAY=13",
            ),
            (
                Pattern::Yearly(YearlyOn::Date),
                Ends::Never,
                "RRULE:FREQ=YEARLY",
            ),
            (
                Pattern::Yearly(YearlyOn::Weekday {
                    month: 10,
                    nth: Nth::Second,
                    weekday: Weekday::Tue,
                }),
                Ends::After(3),
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=2TU;COUNT=3",
            ),
        ];
        for (pattern, ends, expected) in cases {
            let wanted = RecurrenceSpec {
                pattern,
                interval: 1,
                ends,
            };
            let rule = rule_from_spec(&wanted, &all_day).unwrap();
            assert_eq!(rule, expected);
            // Trap (a): what PM writes never pins the hour, and never sets the week's start.
            for part in ["BYHOUR", "BYMINUTE", "BYSECOND", "WKST"] {
                assert!(!rule.contains(part), "{rule}");
            }
            assert_eq!(spec(&rule, &all_day), Some(wanted));
        }
    }

    #[test]
    fn a_spec_outside_the_limits_is_refused() {
        let t = tuesday();
        let daily = |interval, ends| RecurrenceSpec {
            pattern: Pattern::Daily,
            interval,
            ends,
        };
        assert_eq!(
            rule_from_spec(&daily(1, Ends::After(731)), &t),
            Err(SpecError::TooMany)
        );
        assert!(rule_from_spec(&daily(1, Ends::After(730)), &t).is_ok());
        assert_eq!(
            rule_from_spec(&daily(1, Ends::After(0)), &t),
            Err(SpecError::Count)
        );
        assert_eq!(
            rule_from_spec(&daily(0, Ends::Never), &t),
            Err(SpecError::Interval)
        );
        assert_eq!(
            rule_from_spec(&daily(100, Ends::Never), &t),
            Err(SpecError::Interval)
        );
        assert_eq!(
            rule_from_spec(&daily(1, Ends::OnDate(day(2026, 10, 12))), &t),
            Err(SpecError::EndsBeforeStart)
        );
        // Ending on the start's own day is one occurrence, and allowed.
        assert!(rule_from_spec(&daily(1, Ends::OnDate(day(2026, 10, 13))), &t).is_ok());
        assert_eq!(
            rule_from_spec(&weekly(&[], 1, Ends::Never), &t),
            Err(SpecError::NoWeekdays)
        );
        let monthly = |on| RecurrenceSpec {
            pattern: Pattern::Monthly(on),
            interval: 1,
            ends: Ends::Never,
        };
        assert_eq!(
            rule_from_spec(&monthly(MonthlyOn::Day(0)), &t),
            Err(SpecError::DayOfMonth)
        );
        assert_eq!(
            rule_from_spec(&monthly(MonthlyOn::Day(32)), &t),
            Err(SpecError::DayOfMonth)
        );
        let yearly = RecurrenceSpec {
            pattern: Pattern::Yearly(YearlyOn::Weekday {
                month: 13,
                nth: Nth::First,
                weekday: Weekday::Mon,
            }),
            interval: 1,
            ends: Ends::Never,
        };
        assert_eq!(rule_from_spec(&yearly, &t), Err(SpecError::Month));
    }

    /// "Ends on" a day becomes a UTC UNTIL and reads back as that same day, east or west of UTC, even
    /// for a series starting late in the evening.
    #[test]
    fn an_end_date_round_trips_through_until_in_any_zone() {
        for zone in [
            chrono_tz::Europe::London,
            chrono_tz::America::New_York,
            chrono_tz::Pacific::Auckland,
            chrono_tz::Pacific::Kiritimati,
        ] {
            let start = SeriesStart::Timed {
                local: NaiveDateTime::new(
                    day(2026, 10, 13),
                    chrono::NaiveTime::from_hms_opt(23, 30, 0).unwrap(),
                ),
                zone,
            };
            let wanted = weekly(&[Weekday::Tue], 1, Ends::OnDate(day(2027, 3, 30)));
            let rule = rule_from_spec(&wanted, &start).unwrap();
            assert_eq!(spec(&rule, &start), Some(wanted), "{zone}: {rule}");
        }
    }
}
