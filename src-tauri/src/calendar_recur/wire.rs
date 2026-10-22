// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A repeat rule as the webview holds it (#884, C11): the editor's set ([`super::spec`]) with weekdays
//! and nths as words and dates as `YYYY-MM-DD`, since chrono has no serde feature here (A22). Read by
//! the repeat menu's commands and by the editor's drafts, so both mean one thing by a rule.
//!
//! Where a new series starts is read here too ([`series_start`]), with the one date, time and zone
//! parser the event save reads its times with, so the menu's words and the save can't read a start
//! differently.

use std::collections::BTreeSet;
use std::fmt;

use chrono::{NaiveDate, NaiveTime, TimeDelta, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::spec::{Ends, MonthlyOn, Nth, Pattern, RecurrenceSpec, SpecError, YearlyOn};
use super::{first_instant, SeriesStart};

/// A repeat rule from the editor's set, as the webview sends and shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatRule {
    pub pattern: RepeatPattern,
    /// Every this many days, weeks, months or years: 1 to 99.
    pub interval: u16,
    pub ends: RepeatEnds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepeatPattern {
    Daily,
    /// Written Monday first, each day once.
    Weekly {
        days: Vec<DayCode>,
    },
    MonthlyDay {
        day: u8,
    },
    MonthlyWeekday {
        nth: NthCode,
        weekday: DayCode,
    },
    /// On the first occurrence's own date.
    YearlyDate,
    YearlyWeekday {
        month: u8,
        nth: NthCode,
        weekday: DayCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepeatEnds {
    Never,
    /// The last day an occurrence can start on, `YYYY-MM-DD`, in the event's zone.
    OnDate {
        date: String,
    },
    After {
        count: u16,
    },
}

/// A day of the week. Ordered Monday first, so a set of them is written in PM's week order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayCode {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// Which of a month's weekdays: the first four or the last, never a fifth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NthCode {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

/// Where a new repeating event starts, as the editor's fields hold it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepeatStart {
    Timed {
        date: String,
        time: String,
        zone: String,
        /// Where the first occurrence ends, so the day the rule moves it to is checked for a skipped
        /// hour at the end too (C11 critique 2 L7). Left out, only the start is checked.
        #[serde(default)]
        end: Option<WallTime>,
    },
    AllDay {
        date: String,
    },
}

/// A wall-clock time as the editor's fields hold it: `YYYY-MM-DD`, `HH:MM` and an IANA zone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WallTime {
    pub date: String,
    pub time: String,
    pub zone: String,
}

impl WallTime {
    /// The day `days` days after this one when the clocks skip this time on it, in this zone: where
    /// a first occurrence the rule moved would end, as the save moves its end with its start. `None`
    /// when that time happens, or when PM can't read it (the editor's own time check says so).
    pub fn skipped_after(&self, days: i64) -> Option<NaiveDate> {
        let day = parse_date(&self.date)?.checked_add_signed(TimeDelta::try_days(days)?)?;
        let time = parse_time(&self.time)?;
        let zone = parse_zone(&self.zone)?;
        first_instant(zone, day.and_time(time))
            .is_none()
            .then_some(day)
    }
}

/// One ready-made rule of the repeat menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepeatPreset {
    pub label: String,
    pub rule: RepeatRule,
}

/// A rule in words, and where it would start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RepeatView {
    /// `starts`: the first occurrence's day, `YYYY-MM-DD`; `moved`: it isn't the day asked for.
    Ready {
        summary: String,
        starts: String,
        moved: bool,
    },
    /// Why it can't be saved, in words (a [`SpecError`] or a start PM can't read).
    Refused { message: String },
}

/// Why the editor's start can't be read. Shown as is, never echoing the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    Date,
    Time,
    Zone,
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::Date => write!(f, "PM couldn't read that date."),
            StartError::Time => write!(f, "PM couldn't read that time."),
            StartError::Zone => write!(f, "PM doesn't know that time zone."),
        }
    }
}

impl RepeatRule {
    /// The rule as one from the editor's set. Days are sorted Monday first and deduplicated; an empty
    /// day list stays empty for `rule_from_spec` to refuse (`NoWeekdays`). An end date PM can't read
    /// is [`SpecError::BadDate`]; every other limit is `rule_from_spec`'s to check.
    pub fn to_spec(&self) -> Result<RecurrenceSpec, SpecError> {
        let pattern = match &self.pattern {
            RepeatPattern::Daily => Pattern::Daily,
            RepeatPattern::Weekly { days } => {
                let days: BTreeSet<DayCode> = days.iter().copied().collect();
                Pattern::Weekly(days.into_iter().map(Weekday::from).collect())
            }
            RepeatPattern::MonthlyDay { day } => Pattern::Monthly(MonthlyOn::Day(*day)),
            RepeatPattern::MonthlyWeekday { nth, weekday } => {
                Pattern::Monthly(MonthlyOn::Weekday((*nth).into(), (*weekday).into()))
            }
            RepeatPattern::YearlyDate => Pattern::Yearly(YearlyOn::Date),
            RepeatPattern::YearlyWeekday {
                month,
                nth,
                weekday,
            } => Pattern::Yearly(YearlyOn::Weekday {
                month: *month,
                nth: (*nth).into(),
                weekday: (*weekday).into(),
            }),
        };
        let ends = match &self.ends {
            RepeatEnds::Never => Ends::Never,
            RepeatEnds::OnDate { date } => {
                Ends::OnDate(parse_date(date).ok_or(SpecError::BadDate)?)
            }
            RepeatEnds::After { count } => Ends::After(*count),
        };
        Ok(RecurrenceSpec {
            pattern,
            interval: self.interval,
            ends,
        })
    }

    pub fn from_spec(spec: &RecurrenceSpec) -> RepeatRule {
        let pattern = match &spec.pattern {
            Pattern::Daily => RepeatPattern::Daily,
            Pattern::Weekly(days) => RepeatPattern::Weekly {
                days: days.iter().map(|d| DayCode::from(*d)).collect(),
            },
            Pattern::Monthly(MonthlyOn::Day(day)) => RepeatPattern::MonthlyDay { day: *day },
            Pattern::Monthly(MonthlyOn::Weekday(nth, weekday)) => RepeatPattern::MonthlyWeekday {
                nth: (*nth).into(),
                weekday: (*weekday).into(),
            },
            Pattern::Yearly(YearlyOn::Date) => RepeatPattern::YearlyDate,
            Pattern::Yearly(YearlyOn::Weekday {
                month,
                nth,
                weekday,
            }) => RepeatPattern::YearlyWeekday {
                month: *month,
                nth: (*nth).into(),
                weekday: (*weekday).into(),
            },
        };
        let ends = match spec.ends {
            Ends::Never => RepeatEnds::Never,
            Ends::OnDate(day) => RepeatEnds::OnDate {
                date: day.format("%Y-%m-%d").to_string(),
            },
            Ends::After(count) => RepeatEnds::After { count },
        };
        RepeatRule {
            pattern,
            interval: spec.interval,
            ends,
        }
    }
}

impl From<Weekday> for DayCode {
    fn from(day: Weekday) -> DayCode {
        match day {
            Weekday::Mon => DayCode::Mon,
            Weekday::Tue => DayCode::Tue,
            Weekday::Wed => DayCode::Wed,
            Weekday::Thu => DayCode::Thu,
            Weekday::Fri => DayCode::Fri,
            Weekday::Sat => DayCode::Sat,
            Weekday::Sun => DayCode::Sun,
        }
    }
}

impl From<DayCode> for Weekday {
    fn from(day: DayCode) -> Weekday {
        match day {
            DayCode::Mon => Weekday::Mon,
            DayCode::Tue => Weekday::Tue,
            DayCode::Wed => Weekday::Wed,
            DayCode::Thu => Weekday::Thu,
            DayCode::Fri => Weekday::Fri,
            DayCode::Sat => Weekday::Sat,
            DayCode::Sun => Weekday::Sun,
        }
    }
}

impl From<Nth> for NthCode {
    fn from(nth: Nth) -> NthCode {
        match nth {
            Nth::First => NthCode::First,
            Nth::Second => NthCode::Second,
            Nth::Third => NthCode::Third,
            Nth::Fourth => NthCode::Fourth,
            Nth::Last => NthCode::Last,
        }
    }
}

impl From<NthCode> for Nth {
    fn from(nth: NthCode) -> Nth {
        match nth {
            NthCode::First => Nth::First,
            NthCode::Second => Nth::Second,
            NthCode::Third => Nth::Third,
            NthCode::Fourth => Nth::Fourth,
            NthCode::Last => Nth::Last,
        }
    }
}

/// A `YYYY-MM-DD` date, spaces around it allowed: how PM reads every date the editor sends.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// An `HH:MM` time (or `HH:MM:SS`), spaces around it allowed: how PM reads every time the editor
/// sends.
pub fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    NaiveTime::parse_from_str(s, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S"))
        .ok()
}

/// An IANA zone name, spaces around it allowed.
pub fn parse_zone(s: &str) -> Option<Tz> {
    s.trim().parse::<Tz>().ok()
}

/// Where a series starts, from the editor's text: a date, time and zone for a timed start, the date
/// alone for an all-day one. A time the clocks skip isn't refused here: the rule may move the start
/// to another day, so `sync_start` checks the day it lands on.
pub fn series_start(start: &RepeatStart) -> Result<SeriesStart, StartError> {
    match start {
        RepeatStart::Timed {
            date, time, zone, ..
        } => {
            let date = parse_date(date).ok_or(StartError::Date)?;
            let time = parse_time(time).ok_or(StartError::Time)?;
            let zone = parse_zone(zone).ok_or(StartError::Zone)?;
            Ok(SeriesStart::Timed {
                local: date.and_time(time),
                zone,
            })
        }
        RepeatStart::AllDay { date } => Ok(SeriesStart::AllDay(
            parse_date(date).ok_or(StartError::Date)?,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_recur::lines::{self, Kind};
    use crate::calendar_recur::spec::{describe, presets, spec_from_rule, T2_RULES};
    use serde_json::{json, Value};

    /// The T2 rules, their words and their wire form, shared with the webview's tests.
    const FIXTURE: &str = include_str!("../../../src/lib/calendarEdit/repeatRules.fixture.json");

    fn fixture() -> (RepeatStart, Vec<(String, String, RepeatRule)>) {
        let v: Value = serde_json::from_str(FIXTURE).unwrap();
        let start: RepeatStart = serde_json::from_value(v["start"].clone()).unwrap();
        let rows = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["rule"].as_str().unwrap().to_string(),
                    row["words"].as_str().unwrap().to_string(),
                    serde_json::from_value(row["spec"].clone()).unwrap(),
                )
            })
            .collect();
        (start, rows)
    }

    /// `rule` read as one from the editor's set, as a series' words read it.
    fn read(rule: &str, start: &SeriesStart) -> Option<RecurrenceSpec> {
        let lines = lines::parse(&[rule]);
        let Kind::Rule(rule) = &lines[0].kind else {
            return None;
        };
        spec_from_rule(&rule.facts().ok()?, start)
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Every row of `rules_read_as_google_says_them`, in its order, with the wire form the webview
    /// reads it as: the one file both languages test against (C11 §1.5).
    #[test]
    fn t2_rules_match_the_shared_fixture() {
        let (start, rows) = fixture();
        let start = series_start(&start).unwrap();
        let named: Vec<(&str, &str)> = rows
            .iter()
            .map(|(rule, words, _)| (rule.as_str(), words.as_str()))
            .collect();
        assert_eq!(named, T2_RULES, "the fixture holds every T2 row, in order");
        for (rule, words, wire) in &rows {
            let spec = read(rule, &start).unwrap();
            assert_eq!(&RepeatRule::from_spec(&spec), wire, "{rule}");
            assert_eq!(&describe(&spec, &start), words, "{rule}");
        }
    }

    #[test]
    fn every_spec_round_trips_through_the_wire() {
        let (_, rows) = fixture();
        for (rule, _, wire) in &rows {
            let spec = wire.to_spec().unwrap();
            assert_eq!(&RepeatRule::from_spec(&spec), wire, "{rule}");
            assert_eq!(RepeatRule::from_spec(&spec).to_spec(), Ok(spec), "{rule}");
        }
        for d in [
            day(2026, 10, 13),
            day(2026, 10, 27),
            day(2026, 10, 31),
            day(2028, 2, 29),
        ] {
            for preset in presets(d) {
                assert_eq!(
                    RepeatRule::from_spec(&preset.spec).to_spec(),
                    Ok(preset.spec.clone()),
                    "{}",
                    preset.label
                );
            }
        }
    }

    /// The exact strings the webview's types name.
    #[test]
    fn the_wire_strings_are_the_ones_the_webview_names() {
        let wire = |rule: &RepeatRule| serde_json::to_value(rule).unwrap();
        let cases = [
            (
                RepeatPattern::Weekly {
                    days: vec![DayCode::Mon, DayCode::Wed],
                },
                RepeatEnds::OnDate {
                    date: "2026-12-31".into(),
                },
                json!({"pattern":{"kind":"weekly","days":["mon","wed"]},"interval":2,
                       "ends":{"kind":"on_date","date":"2026-12-31"}}),
            ),
            (
                RepeatPattern::Daily,
                RepeatEnds::Never,
                json!({"pattern":{"kind":"daily"},"interval":2,"ends":{"kind":"never"}}),
            ),
            (
                RepeatPattern::MonthlyDay { day: 31 },
                RepeatEnds::After { count: 10 },
                json!({"pattern":{"kind":"monthly_day","day":31},"interval":2,
                       "ends":{"kind":"after","count":10}}),
            ),
            (
                RepeatPattern::MonthlyWeekday {
                    nth: NthCode::Last,
                    weekday: DayCode::Fri,
                },
                RepeatEnds::Never,
                json!({"pattern":{"kind":"monthly_weekday","nth":"last","weekday":"fri"},
                       "interval":2,"ends":{"kind":"never"}}),
            ),
            (
                RepeatPattern::YearlyDate,
                RepeatEnds::Never,
                json!({"pattern":{"kind":"yearly_date"},"interval":2,"ends":{"kind":"never"}}),
            ),
            (
                RepeatPattern::YearlyWeekday {
                    month: 10,
                    nth: NthCode::Second,
                    weekday: DayCode::Tue,
                },
                RepeatEnds::Never,
                json!({"pattern":{"kind":"yearly_weekday","month":10,"nth":"second",
                       "weekday":"tue"},"interval":2,"ends":{"kind":"never"}}),
            ),
        ];
        for (pattern, ends, expected) in cases {
            let rule = RepeatRule {
                pattern,
                interval: 2,
                ends,
            };
            assert_eq!(wire(&rule), expected);
            assert_eq!(
                serde_json::from_value::<RepeatRule>(expected).unwrap(),
                rule
            );
        }
        for (start, expected) in [
            (
                json!({"kind":"timed","date":"2026-10-13","time":"09:00","zone":"Europe/London"}),
                RepeatStart::Timed {
                    date: "2026-10-13".into(),
                    time: "09:00".into(),
                    zone: "Europe/London".into(),
                    end: None,
                },
            ),
            (
                json!({"kind":"timed","date":"2026-10-13","time":"09:00","zone":"Europe/London",
                       "end":{"date":"2026-10-13","time":"10:00","zone":"Europe/London"}}),
                RepeatStart::Timed {
                    date: "2026-10-13".into(),
                    time: "09:00".into(),
                    zone: "Europe/London".into(),
                    end: Some(WallTime {
                        date: "2026-10-13".into(),
                        time: "10:00".into(),
                        zone: "Europe/London".into(),
                    }),
                },
            ),
            (
                json!({"kind":"all_day","date":"2026-10-13"}),
                RepeatStart::AllDay {
                    date: "2026-10-13".into(),
                },
            ),
        ] {
            assert_eq!(
                serde_json::from_value::<RepeatStart>(start).unwrap(),
                expected
            );
        }
        let views = [
            (
                RepeatView::Ready {
                    summary: "Daily".into(),
                    starts: "2026-10-14".into(),
                    moved: true,
                },
                json!({"outcome":"ready","summary":"Daily","starts":"2026-10-14","moved":true}),
            ),
            (
                RepeatView::Refused {
                    message: "No.".into(),
                },
                json!({"outcome":"refused","message":"No."}),
            ),
        ];
        for (view, expected) in views {
            assert_eq!(serde_json::to_value(&view).unwrap(), expected);
        }
        // A kind, day or nth PM doesn't know is refused, never guessed.
        for unknown in [
            json!({"pattern":{"kind":"hourly"},"interval":1,"ends":{"kind":"never"}}),
            json!({"pattern":{"kind":"weekly","days":["monday"]},"interval":1,"ends":{"kind":"never"}}),
            json!({"pattern":{"kind":"monthly_weekday","nth":"fifth","weekday":"fri"},"interval":1,
                   "ends":{"kind":"never"}}),
        ] {
            assert!(serde_json::from_value::<RepeatRule>(unknown).is_err());
        }
    }

    #[test]
    fn weekdays_are_written_monday_first_and_once() {
        let rule = RepeatRule {
            pattern: RepeatPattern::Weekly {
                days: vec![DayCode::Wed, DayCode::Mon, DayCode::Wed],
            },
            interval: 1,
            ends: RepeatEnds::Never,
        };
        assert_eq!(
            rule.to_spec().unwrap().pattern,
            Pattern::Weekly(vec![Weekday::Mon, Weekday::Wed])
        );
        // No day at all stays empty, for the rule's own check to refuse in words.
        let none = RepeatRule {
            pattern: RepeatPattern::Weekly { days: Vec::new() },
            ..rule
        };
        assert_eq!(none.to_spec().unwrap().pattern, Pattern::Weekly(Vec::new()));
    }

    #[test]
    fn an_unreadable_end_date_is_refused() {
        let ending = |date: &str| RepeatRule {
            pattern: RepeatPattern::Daily,
            interval: 1,
            ends: RepeatEnds::OnDate { date: date.into() },
        };
        assert_eq!(ending("31-12-2026").to_spec(), Err(SpecError::BadDate));
        assert_eq!(ending("").to_spec(), Err(SpecError::BadDate));
        assert_eq!(
            ending("2026-12-31").to_spec().unwrap().ends,
            Ends::OnDate(day(2026, 12, 31))
        );
        assert_eq!(
            SpecError::BadDate.to_string(),
            "PM couldn't read that end date."
        );
    }

    #[test]
    fn a_start_is_read_or_refused_in_words() {
        let timed = |date: &str, time: &str, zone: &str| RepeatStart::Timed {
            date: date.into(),
            time: time.into(),
            zone: zone.into(),
            end: None,
        };
        assert_eq!(
            series_start(&timed(" 2026-10-13 ", "09:00:30", "Europe/London")),
            Ok(SeriesStart::Timed {
                local: day(2026, 10, 13).and_hms_opt(9, 0, 30).unwrap(),
                zone: chrono_tz::Europe::London,
            })
        );
        assert_eq!(
            series_start(&RepeatStart::AllDay {
                date: "2026-10-13".into()
            }),
            Ok(SeriesStart::AllDay(day(2026, 10, 13)))
        );
        // A time the clocks skip is the rule's day's question, not the start's.
        assert!(series_start(&timed("2026-03-29", "01:30", "Europe/London")).is_ok());
        for (start, why, words) in [
            (
                timed("13/10/2026", "09:00", "Europe/London"),
                StartError::Date,
                "PM couldn't read that date.",
            ),
            (
                timed("2026-10-13", "9am", "Europe/London"),
                StartError::Time,
                "PM couldn't read that time.",
            ),
            (
                timed("2026-10-13", "09:00", "Mars/Olympus"),
                StartError::Zone,
                "PM doesn't know that time zone.",
            ),
            (
                RepeatStart::AllDay {
                    date: "soon".into(),
                },
                StartError::Date,
                "PM couldn't read that date.",
            ),
        ] {
            assert_eq!(series_start(&start), Err(why));
            assert_eq!(why.to_string(), words);
        }
    }

    /// London skips 01:00-02:00 on 29-03-2026: an end at 01:30 the day before is found there once
    /// the rule moves it a day on, and not before; an end PM can't read is the editor's to say.
    #[test]
    fn an_end_the_clocks_skip_is_found_where_the_rule_moves_it() {
        let end = |date: &str, time: &str, zone: &str| WallTime {
            date: date.into(),
            time: time.into(),
            zone: zone.into(),
        };
        let night = end("2026-03-28", "01:30", "Europe/London");
        assert_eq!(night.skipped_after(1), Some(day(2026, 3, 29)));
        assert_eq!(night.skipped_after(0), None);
        assert_eq!(night.skipped_after(2), None);
        // The same night in New York is an ordinary one.
        assert_eq!(
            end("2026-03-28", "01:30", "America/New_York").skipped_after(1),
            None
        );
        for unread in [
            end("28-03-2026", "01:30", "Europe/London"),
            end("2026-03-28", "late", "Europe/London"),
            end("2026-03-28", "01:30", "Mars/Olympus"),
        ] {
            assert_eq!(unread.skipped_after(1), None);
        }
    }
}
