// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a series' rule becomes when one of its occurrences moves to another day and every occurrence
//! is to follow ("all events", or the rest of a split): only the part of the rule that the date
//! implied changes (Bobby, Q4: "shift the repeat day", Tuesdays → Wednesdays), and [`DayMap`] says
//! where each old occurrence lands. A move PM can't carry to every occurrence the same way is refused
//! with a reason, never guessed at: the editor's "This event" still moves the one.
//!
//! The map is per occurrence, not one shift for all: on "Monday and Wednesday", moving the Wednesday
//! to Thursday moves every Wednesday a day and leaves the Mondays where they are, and "the second
//! Tuesday" becoming "the second Wednesday" is a day later in some months and six days earlier in
//! others.

use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use std::fmt;

use super::lines::{ByDay, Freq, Rule, RuleFacts};
use super::spec::days_in_month;

/// Where each old occurrence's day lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayMap {
    /// Only the time changes.
    Same,
    /// Every occurrence moves this many days (DAILY).
    Days(i64),
    /// Occurrences on `from` move `days` (within their week); the rest stay (WEEKLY).
    Weekday { from: Weekday, days: i64 },
    /// Every occurrence moves to this day of its own month (MONTHLY by the day of the month).
    MonthDay(u32),
    /// Occurrences that are the `from` weekday of their month move to the `to` one in the same month;
    /// the rest stay (MONTHLY, or YEARLY in one month, by "the second Tuesday").
    Nth { from: ByDay, to: ByDay },
    /// Every occurrence moves to this month and day, `years` later (YEARLY by date).
    YearDate { years: i32, month: u32, day: u32 },
}

impl DayMap {
    /// The day `day` (an old occurrence's, in the series' zone) lands on, or `None` when that month
    /// or year has no such day.
    pub fn day_of(&self, day: NaiveDate) -> Option<NaiveDate> {
        match *self {
            DayMap::Same => Some(day),
            DayMap::Days(n) => day.checked_add_signed(TimeDelta::days(n)),
            DayMap::Weekday { from, days } if day.weekday() == from => {
                day.checked_add_signed(TimeDelta::days(days))
            }
            DayMap::Weekday { .. } => Some(day),
            DayMap::MonthDay(d) => NaiveDate::from_ymd_opt(day.year(), day.month(), d),
            DayMap::Nth { from, to } if is_nth(day, from) => {
                nth_of_month(day.year(), day.month(), to)
            }
            DayMap::Nth { .. } => Some(day),
            DayMap::YearDate {
                years,
                month,
                day: d,
            } => NaiveDate::from_ymd_opt(day.year() + years, month, d),
        }
    }
}

/// Why a move can't be carried to every occurrence. Shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReanchorError {
    /// The series already repeats on that day.
    Collision(Weekday),
    /// The move passes another day the series repeats on, which would then be lost or doubled.
    Crosses(Weekday),
    CrossWeek,
    CrossMonth,
    CrossYear,
    /// "The fifth Tuesday": not every month has one, and Google's editor never offers it.
    Fifth,
    /// A "last Friday" moved to a day that isn't the last of its kind.
    NotLast,
    /// PM can't tell how the rule would read after the move.
    NotDerivable,
}

impl fmt::Display for ReanchorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReanchorError::Collision(day) => write!(
                f,
                "The series already repeats on {}, so every occurrence can't move there.",
                weekday_name(*day)
            ),
            ReanchorError::Crosses(day) => write!(
                f,
                "That moves it past {}, which the series also repeats on.",
                weekday_name(*day)
            ),
            ReanchorError::CrossWeek => write!(
                f,
                "That moves it into another week, which PM can't carry to every occurrence."
            ),
            ReanchorError::CrossMonth => write!(
                f,
                "That moves it into another month, which PM can't carry to every occurrence."
            ),
            ReanchorError::CrossYear => write!(
                f,
                "That moves it into another year, which PM can't carry to every occurrence."
            ),
            ReanchorError::Fifth => write!(
                f,
                "Not every month has a fifth of that weekday, so the series can't repeat on it."
            ),
            ReanchorError::NotLast => write!(
                f,
                "The series repeats on the last of a weekday each month, and that day isn't one."
            ),
            ReanchorError::NotDerivable => write!(
                f,
                "PM can't work out how this series would repeat after that move."
            ),
        }
    }
}

fn weekday_name(day: Weekday) -> &'static str {
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

/// Which of its month's weekdays `day` is: 1 for the first … 5 for a fifth.
fn week_of_month(day: NaiveDate) -> i8 {
    ((day.day() - 1) / 7 + 1) as i8
}

fn is_last_of_month(day: NaiveDate) -> bool {
    day.day() + 7 > days_in_month(day)
}

/// Whether `day` is the weekday `by` names in its month ("2TU", "-1FR").
fn is_nth(day: NaiveDate, by: ByDay) -> bool {
    day.weekday() == by.weekday
        && match by.nth {
            Some(-1) => is_last_of_month(day),
            Some(n) => week_of_month(day) == n,
            None => false,
        }
}

/// The day `by` names in a month ("the second Tuesday of October 2026").
fn nth_of_month(year: i32, month: u32, by: ByDay) -> Option<NaiveDate> {
    match by.nth? {
        -1 => {
            let last = NaiveDate::from_ymd_opt(year, month, 1).map(days_in_month)?;
            (0..7)
                .map(|back| NaiveDate::from_ymd_opt(year, month, last - back))
                .find(|d| d.is_some_and(|d| d.weekday() == by.weekday))
                .flatten()
        }
        n @ 1..=5 => NaiveDate::from_weekday_of_month_opt(year, month, by.weekday, n as u8),
        _ => None,
    }
}

/// The first day of `day`'s week, the week starting on `week_start` (RFC 5545's WKST, Monday by default).
fn week_start(day: NaiveDate, week_start: Weekday) -> NaiveDate {
    let back = (day.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7;
    day - TimeDelta::days(i64::from(back))
}

fn by_day_text(entries: &[ByDay]) -> String {
    entries
        .iter()
        .map(ByDay::text)
        .collect::<Vec<_>>()
        .join(",")
}

/// `rule` (read as `facts`) with the occurrence on `from` moved to `to`, and where every old
/// occurrence lands. `from` must be an occurrence of the rule; the caller checks that, and checks the
/// result occurrence by occurrence ([`super::remap::retime`]), so a row here only has to be right for
/// the moves it accepts.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9, editing repeating events"
    )
)]
pub fn reanchor(
    rule: &Rule,
    facts: &RuleFacts,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(Rule, DayMap), ReanchorError> {
    if from == to {
        return Ok((rule.clone(), DayMap::Same));
    }
    let days = (to - from).num_days();
    let same_month = from.year() == to.year() && from.month() == to.month();
    match facts.freq {
        Freq::Daily => {
            if !facts.by_day.is_empty()
                || facts.by_month_day.is_some()
                || !facts.by_month.is_empty()
            {
                return Err(ReanchorError::NotDerivable);
            }
            Ok((rule.clone(), DayMap::Days(days)))
        }
        Freq::Weekly => {
            if facts.by_month_day.is_some() || !facts.by_month.is_empty() {
                return Err(ReanchorError::NotDerivable);
            }
            let wkst = facts.week_start.unwrap_or(Weekday::Mon);
            if week_start(from, wkst) != week_start(to, wkst) {
                return Err(ReanchorError::CrossWeek);
            }
            let map = DayMap::Weekday {
                from: from.weekday(),
                days,
            };
            if facts.by_day.is_empty() {
                // The day comes from the series' start, which moves with it.
                return Ok((rule.clone(), map));
            }
            let at = facts
                .by_day
                .iter()
                .position(|d| d.weekday == from.weekday())
                .ok_or(ReanchorError::NotDerivable)?;
            if facts.by_day.iter().any(|d| d.weekday == to.weekday()) {
                return Err(ReanchorError::Collision(to.weekday()));
            }
            let (low, high) = (from.min(to), from.max(to));
            if let Some(between) = low
                .iter_days()
                .skip(1)
                .take_while(|d| *d < high)
                .find(|d| facts.by_day.iter().any(|b| b.weekday == d.weekday()))
            {
                return Err(ReanchorError::Crosses(between.weekday()));
            }
            let mut entries = facts.by_day.clone();
            entries[at].weekday = to.weekday();
            Ok((rule.with("BYDAY", &by_day_text(&entries)), map))
        }
        Freq::Monthly => {
            if !facts.by_month.is_empty() {
                return Err(ReanchorError::NotDerivable);
            }
            if !same_month {
                return Err(ReanchorError::CrossMonth);
            }
            by_month_part(rule, facts, from, to)
        }
        Freq::Yearly => {
            if facts.by_day.is_empty() && facts.by_month_day.is_none() && facts.by_month.is_empty()
            {
                // Month and day both come from the series' start.
                return Ok((
                    rule.clone(),
                    DayMap::YearDate {
                        years: to.year() - from.year(),
                        month: to.month(),
                        day: to.day(),
                    },
                ));
            }
            if facts.by_month != [from.month() as u8] {
                return Err(ReanchorError::NotDerivable);
            }
            if !facts.by_day.is_empty() {
                if !same_month {
                    return Err(ReanchorError::CrossMonth);
                }
                return by_month_part(rule, facts, from, to);
            }
            if facts.by_month_day != Some(from.day() as i8) {
                return Err(ReanchorError::NotDerivable);
            }
            if from.year() != to.year() {
                return Err(ReanchorError::CrossYear);
            }
            let rule = rule
                .with("BYMONTH", &to.month().to_string())
                .with("BYMONTHDAY", &to.day().to_string());
            Ok((
                rule,
                DayMap::YearDate {
                    years: 0,
                    month: to.month(),
                    day: to.day(),
                },
            ))
        }
    }
}

/// A move within one month of a rule that picks its days of the month: by the day's number
/// (written or implied by the start) or by "the nth weekday".
fn by_month_part(
    rule: &Rule,
    facts: &RuleFacts,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(Rule, DayMap), ReanchorError> {
    if facts.by_day.is_empty() {
        return match facts.by_month_day {
            None => Ok((rule.clone(), DayMap::MonthDay(to.day()))),
            Some(d) if d == from.day() as i8 => Ok((
                rule.with("BYMONTHDAY", &to.day().to_string()),
                DayMap::MonthDay(to.day()),
            )),
            Some(_) => Err(ReanchorError::NotDerivable),
        };
    }
    if facts.by_month_day.is_some() {
        return Err(ReanchorError::NotDerivable);
    }
    let at = facts
        .by_day
        .iter()
        .position(|d| is_nth(from, *d))
        .ok_or(ReanchorError::NotDerivable)?;
    let old = facts.by_day[at];
    let nth = if old.nth == Some(-1) {
        if !is_last_of_month(to) {
            return Err(ReanchorError::NotLast);
        }
        -1
    } else {
        match week_of_month(to) {
            5 => return Err(ReanchorError::Fifth),
            n => n,
        }
    };
    let new = ByDay {
        nth: Some(nth),
        weekday: to.weekday(),
    };
    if facts.by_day.contains(&new) {
        return Err(ReanchorError::Collision(to.weekday()));
    }
    let mut entries = facts.by_day.clone();
    entries[at] = new;
    Ok((
        rule.with("BYDAY", &by_day_text(&entries)),
        DayMap::Nth { from: old, to: new },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_recur::lines::{self, Kind};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The rule written after moving `from` to `to`, or why not.
    fn moved(
        rule: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<(String, DayMap), ReanchorError> {
        let parsed = lines::parse(&[rule]);
        let Kind::Rule(r) = &parsed[0].kind else {
            panic!("{rule}")
        };
        let facts = r.facts().unwrap();
        reanchor(r, &facts, from, to).map(|(r, map)| (r.line(), map))
    }

    /// T7 and Q4: a weekly series' repeat day moves with the occurrence, and only that day.
    #[test]
    fn a_weekly_repeat_day_moves_with_its_occurrence() {
        // Mon 12-10 → Tue 13-10 on "every Monday".
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                day(2026, 10, 12),
                day(2026, 10, 13)
            ),
            Ok((
                "RRULE:FREQ=WEEKLY;BYDAY=TU".into(),
                DayMap::Weekday {
                    from: Weekday::Mon,
                    days: 1
                }
            ))
        );
        // Wed 07-10 → Thu 08-10 on "Monday and Wednesday": in place, Mondays untouched.
        let (rule, map) = moved(
            "RRULE:FREQ=WEEKLY;BYDAY=MO,WE",
            day(2026, 10, 7),
            day(2026, 10, 8),
        )
        .unwrap();
        assert_eq!(rule, "RRULE:FREQ=WEEKLY;BYDAY=MO,TH");
        assert_eq!(map.day_of(day(2026, 10, 19)), Some(day(2026, 10, 19)));
        assert_eq!(map.day_of(day(2026, 10, 21)), Some(day(2026, 10, 22)));
        // The day implied by the start moves with the start.
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;INTERVAL=2",
                day(2026, 10, 12),
                day(2026, 10, 16)
            ),
            Ok((
                "RRULE:FREQ=WEEKLY;INTERVAL=2".into(),
                DayMap::Weekday {
                    from: Weekday::Mon,
                    days: 4
                }
            ))
        );
        // WKST and every other part kept as they were.
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;WKST=SU;BYDAY=MO;COUNT=5",
                day(2026, 10, 12),
                day(2026, 10, 13)
            )
            .unwrap()
            .0,
            "RRULE:FREQ=WEEKLY;WKST=SU;BYDAY=TU;COUNT=5"
        );
    }

    #[test]
    fn a_weekly_move_that_would_lose_or_double_an_occurrence_is_refused() {
        let mo_we = "RRULE:FREQ=WEEKLY;BYDAY=MO,WE";
        // Onto the other day.
        assert_eq!(
            moved(mo_we, day(2026, 10, 7), day(2026, 10, 5)),
            Err(ReanchorError::Collision(Weekday::Mon))
        );
        // Past the other day, either way (RC3/AS3).
        assert_eq!(
            moved(mo_we, day(2026, 10, 12), day(2026, 10, 15)),
            Err(ReanchorError::Crosses(Weekday::Wed))
        );
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR",
                day(2026, 10, 23),
                day(2026, 10, 20)
            ),
            Err(ReanchorError::Crosses(Weekday::Wed))
        );
        // Into another week: +8 days, and Sunday → Monday with the week starting on Monday.
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                day(2026, 10, 12),
                day(2026, 10, 20)
            ),
            Err(ReanchorError::CrossWeek)
        );
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;BYDAY=SU",
                day(2026, 10, 18),
                day(2026, 10, 19)
            ),
            Err(ReanchorError::CrossWeek)
        );
        // …but the same move is within one week when the week starts on Sunday.
        assert!(moved(
            "RRULE:FREQ=WEEKLY;WKST=SU;BYDAY=SU",
            day(2026, 10, 18),
            day(2026, 10, 19)
        )
        .is_ok());
    }

    #[test]
    fn a_monthly_day_moves_within_its_month() {
        // T7: BYMONTHDAY 13 → 14.
        assert_eq!(
            moved(
                "RRULE:FREQ=MONTHLY;BYMONTHDAY=13",
                day(2026, 10, 13),
                day(2026, 10, 14)
            ),
            Ok((
                "RRULE:FREQ=MONTHLY;BYMONTHDAY=14".into(),
                DayMap::MonthDay(14)
            ))
        );
        // The day implied by the start: the rule stays as it is, every occurrence takes the new day
        // of its own month (RM3: not a day count, which a short February would throw off).
        let (rule, map) =
            moved("RRULE:FREQ=MONTHLY", day(2026, 10, 15), day(2026, 10, 30)).unwrap();
        assert_eq!(rule, "RRULE:FREQ=MONTHLY");
        assert_eq!(map.day_of(day(2026, 2, 15)), None);
        assert_eq!(map.day_of(day(2026, 3, 15)), Some(day(2026, 3, 30)));
        // T7: 30-10 → 02-11.
        assert_eq!(
            moved(
                "RRULE:FREQ=MONTHLY;BYMONTHDAY=30",
                day(2026, 10, 30),
                day(2026, 11, 2)
            ),
            Err(ReanchorError::CrossMonth)
        );
    }

    /// T7: "the second Tuesday" becomes "the second Wednesday", which in September 2027 (it starts on
    /// a Wednesday) is six days before the second Tuesday (RM5).
    #[test]
    fn an_nth_weekday_moves_to_the_same_nth_in_each_month() {
        let (rule, map) = moved(
            "RRULE:FREQ=MONTHLY;BYDAY=2TU",
            day(2026, 10, 13),
            day(2026, 10, 14),
        )
        .unwrap();
        assert_eq!(rule, "RRULE:FREQ=MONTHLY;BYDAY=2WE");
        assert_eq!(map.day_of(day(2026, 11, 10)), Some(day(2026, 11, 11)));
        assert_eq!(map.day_of(day(2027, 9, 14)), Some(day(2027, 9, 8)));
        // The last Friday → the last Saturday (T7), and April 2027 ends on a Friday.
        let (rule, map) = moved(
            "RRULE:FREQ=MONTHLY;BYDAY=-1FR",
            day(2026, 11, 27),
            day(2026, 11, 28),
        )
        .unwrap();
        assert_eq!(rule, "RRULE:FREQ=MONTHLY;BYDAY=-1SA");
        assert_eq!(map.day_of(day(2027, 4, 30)), Some(day(2027, 4, 24)));
        // A "last" can only move to another last.
        assert_eq!(
            moved(
                "RRULE:FREQ=MONTHLY;BYDAY=-1FR",
                day(2026, 11, 27),
                day(2026, 11, 26)
            ),
            Ok((
                "RRULE:FREQ=MONTHLY;BYDAY=-1TH".into(),
                DayMap::Nth {
                    from: ByDay {
                        nth: Some(-1),
                        weekday: Weekday::Fri
                    },
                    to: ByDay {
                        nth: Some(-1),
                        weekday: Weekday::Thu
                    }
                }
            ))
        );
        assert_eq!(
            moved(
                "RRULE:FREQ=MONTHLY;BYDAY=-1FR",
                day(2026, 11, 27),
                day(2026, 11, 20)
            ),
            Err(ReanchorError::NotLast)
        );
        // No fifth (the 29th of October 2026 is its fifth Thursday).
        assert_eq!(
            moved(
                "RRULE:FREQ=MONTHLY;BYDAY=4TH",
                day(2026, 10, 22),
                day(2026, 10, 29)
            ),
            Err(ReanchorError::Fifth)
        );
        // Two entries: only the matching one moves, the other's occurrences stay.
        let (rule, map) = moved(
            "RRULE:FREQ=MONTHLY;BYDAY=1MO,3MO",
            day(2026, 10, 19),
            day(2026, 10, 20),
        )
        .unwrap();
        assert_eq!(rule, "RRULE:FREQ=MONTHLY;BYDAY=1MO,3TU");
        assert_eq!(map.day_of(day(2026, 11, 2)), Some(day(2026, 11, 2)));
        assert_eq!(map.day_of(day(2026, 11, 16)), Some(day(2026, 11, 17)));
    }

    #[test]
    fn a_yearly_date_moves_in_every_year() {
        // T7: implied, 13-10 → 20-10.
        let (rule, map) = moved("RRULE:FREQ=YEARLY", day(2026, 10, 13), day(2026, 10, 20)).unwrap();
        assert_eq!(rule, "RRULE:FREQ=YEARLY");
        assert_eq!(map.day_of(day(2030, 10, 13)), Some(day(2030, 10, 20)));
        // RM3 case B: across a leap February the date, not a day count, carries.
        let (_, map) = moved("RRULE:FREQ=YEARLY", day(2028, 2, 15), day(2028, 3, 15)).unwrap();
        assert_eq!(map.day_of(day(2026, 2, 15)), Some(day(2026, 3, 15)));
        // Across New Year, every occurrence moves into the next year.
        let (_, map) = moved("RRULE:FREQ=YEARLY", day(2027, 12, 30), day(2028, 1, 2)).unwrap();
        assert_eq!(map.day_of(day(2026, 12, 30)), Some(day(2027, 1, 2)));
        // A written month and day: both rewritten, within the year (RM9).
        assert_eq!(
            moved(
                "RRULE:FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=30",
                day(2027, 12, 30),
                day(2027, 12, 31)
            )
            .unwrap()
            .0,
            "RRULE:FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=31"
        );
        assert_eq!(
            moved(
                "RRULE:FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=30",
                day(2027, 12, 30),
                day(2028, 1, 2)
            ),
            Err(ReanchorError::CrossYear)
        );
        // "The second Tuesday of October".
        assert_eq!(
            moved(
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=2TU",
                day(2026, 10, 13),
                day(2026, 10, 14)
            )
            .unwrap()
            .0,
            "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=2WE"
        );
    }

    #[test]
    fn a_daily_series_moves_by_days_and_a_rule_pm_cant_read_back_is_refused() {
        assert_eq!(
            moved(
                "RRULE:FREQ=DAILY;INTERVAL=2",
                day(2026, 10, 12),
                day(2026, 10, 13)
            ),
            Ok(("RRULE:FREQ=DAILY;INTERVAL=2".into(), DayMap::Days(1)))
        );
        assert_eq!(
            moved(
                "RRULE:FREQ=DAILY;BYDAY=MO,TU",
                day(2026, 10, 12),
                day(2026, 10, 13)
            ),
            Err(ReanchorError::NotDerivable)
        );
        // A weekly rule whose list doesn't hold the day being moved.
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                day(2026, 10, 13),
                day(2026, 10, 14)
            ),
            Err(ReanchorError::NotDerivable)
        );
        // Unchanged day: unchanged rule.
        assert_eq!(
            moved(
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                day(2026, 10, 12),
                day(2026, 10, 12)
            ),
            Ok(("RRULE:FREQ=WEEKLY;BYDAY=MO".into(), DayMap::Same))
        );
    }

    /// C8a review V6: the refusals no other test reaches, one row each.
    #[test]
    fn every_other_refusal_has_its_row() {
        let rows = [
            // "The first and third Monday": the first moved onto the third.
            (
                "RRULE:FREQ=MONTHLY;BYDAY=1MO,3MO",
                day(2026, 10, 5),
                day(2026, 10, 19),
                ReanchorError::Collision(Weekday::Mon),
            ),
            // "The second Tuesday of October" moved into November, or onto a fifth Thursday; a
            // last Friday moved onto a day that isn't the last.
            (
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=2TU",
                day(2026, 10, 13),
                day(2026, 11, 3),
                ReanchorError::CrossMonth,
            ),
            (
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=2TU",
                day(2026, 10, 13),
                day(2026, 10, 29),
                ReanchorError::Fifth,
            ),
            (
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1FR",
                day(2026, 10, 30),
                day(2026, 10, 22),
                ReanchorError::NotLast,
            ),
            // Rules whose written parts don't name the day being moved, or pick days two ways.
            (
                "RRULE:FREQ=YEARLY;BYMONTH=11;BYMONTHDAY=13",
                day(2026, 10, 13),
                day(2026, 10, 14),
                ReanchorError::NotDerivable,
            ),
            (
                "RRULE:FREQ=MONTHLY;BYMONTHDAY=-1",
                day(2026, 10, 31),
                day(2026, 10, 30),
                ReanchorError::NotDerivable,
            ),
            (
                "RRULE:FREQ=YEARLY;BYMONTH=10;BYMONTHDAY=-1",
                day(2026, 10, 31),
                day(2026, 10, 30),
                ReanchorError::NotDerivable,
            ),
            (
                "RRULE:FREQ=MONTHLY;BYDAY=FR;BYMONTHDAY=13",
                day(2026, 11, 13),
                day(2026, 11, 14),
                ReanchorError::NotDerivable,
            ),
            (
                "RRULE:FREQ=MONTHLY;BYMONTH=10;BYMONTHDAY=13",
                day(2026, 10, 13),
                day(2026, 10, 14),
                ReanchorError::NotDerivable,
            ),
            (
                "RRULE:FREQ=WEEKLY;BYDAY=MO;BYMONTH=10",
                day(2026, 10, 12),
                day(2026, 10, 13),
                ReanchorError::NotDerivable,
            ),
        ];
        for (rule, from, to, refused) in rows {
            assert_eq!(moved(rule, from, to), Err(refused), "{rule}");
        }
    }
}
