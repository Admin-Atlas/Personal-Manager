// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A `recurrence` array (or an iCal block's repeat lines) parsed in order. Every line keeps the exact
//! text it came as, so a change to one part of one line (a new UNTIL, say) writes every other line,
//! and every other part of that rule, back exactly as Google sent it.
//!
//! Google's array holds "RRULE, EXRULE, RDATE and EXDATE lines" (events reference). Property names are
//! case-insensitive (RFC 5545 §3.1), and so are a rule's parts and values, so both are read
//! upper-cased; a line PM changes is written back canonical.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use std::fmt;

use super::{rfc_instant, Point, SeriesStart};

/// One line of a `recurrence` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Exactly as it came: an untouched line is written back byte for byte.
    pub raw: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Rule(Rule),
    /// Kept as text and never expanded or changed here: RFC 5545 deprecates EXRULE, and a split or a
    /// time change can't carry one safely.
    ExRule,
    ExDate(Dates),
    RDate(Dates),
    /// A line PM can't read: an unknown property, an unknown zone, a PERIOD, a malformed value.
    Unreadable,
}

/// The lines of a `recurrence` array, in order.
pub fn parse<S: AsRef<str>>(lines: &[S]) -> Vec<Line> {
    lines.iter().map(|l| parse_line(l.as_ref())).collect()
}

fn parse_line(raw: &str) -> Line {
    let kind = (|| {
        let (head, value) = raw.split_once(':')?;
        let (name, params) = match head.find(';') {
            Some(i) => (&head[..i], &head[i..]),
            None => (head, ""),
        };
        Some(match name.trim().to_ascii_uppercase().as_str() {
            "RRULE" => Kind::Rule(Rule::parse(params, value)?),
            "EXRULE" => Kind::ExRule,
            "EXDATE" => Kind::ExDate(Dates::parse("EXDATE", params, value)?),
            "RDATE" => Kind::RDate(Dates::parse("RDATE", params, value)?),
            _ => return None,
        })
    })()
    .unwrap_or(Kind::Unreadable);
    Line {
        raw: raw.to_string(),
        kind,
    }
}

/// An RRULE as its parts, in the order they came, names and values upper-cased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// `;`-led parameters on the RRULE property itself, as they came (Google sends none).
    params: String,
    parts: Vec<(String, String)>,
}

/// More parts than any real rule has: RFC 5545 defines 14, RFC 7529 adds 2. Feed text reaches the
/// parse below, and its duplicate check looks back over the parts kept, so the bound keeps a hostile
/// line of a million parts from costing a million squared.
const MAX_PARTS: usize = 32;

impl Rule {
    fn parse(params: &str, value: &str) -> Option<Rule> {
        let mut parts: Vec<(String, String)> = Vec::new();
        for part in value.split(';').filter(|p| !p.trim().is_empty()) {
            if parts.len() == MAX_PARTS {
                return None;
            }
            let (k, v) = part.split_once('=')?;
            let (k, v) = (k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase());
            if k.is_empty() || v.is_empty() || parts.iter().any(|(seen, _)| *seen == k) {
                return None;
            }
            parts.push((k, v));
        }
        (!parts.is_empty()).then(|| Rule {
            params: params.to_string(),
            parts,
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.parts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// This rule with `key` set to `value`: in its own place if it has one, otherwise last.
    pub fn with(&self, key: &str, value: &str) -> Rule {
        let mut rule = self.clone();
        match rule.parts.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.to_string(),
            None => rule.parts.push((key.to_string(), value.to_string())),
        }
        rule
    }

    pub fn without(&self, key: &str) -> Rule {
        let mut rule = self.clone();
        rule.parts.retain(|(k, _)| k != key);
        rule
    }

    /// The rule as an RRULE line, canonical: upper-case, parts in their order.
    pub fn line(&self) -> String {
        let parts: Vec<String> = self.parts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("RRULE{}:{}", self.params, parts.join(";"))
    }

    /// The parts read, or why PM can only show this rule as text.
    pub fn facts(&self) -> Result<RuleFacts, TextOnly> {
        // FREQ first, so a sub-daily rule says so whatever other parts it carries.
        let freq = match self.get("FREQ") {
            Some("DAILY") => Freq::Daily,
            Some("WEEKLY") => Freq::Weekly,
            Some("MONTHLY") => Freq::Monthly,
            Some("YEARLY") => Freq::Yearly,
            Some("HOURLY" | "MINUTELY" | "SECONDLY") => return Err(TextOnly::SubDaily),
            _ => return Err(TextOnly::Unreadable),
        };
        let mut facts = RuleFacts {
            freq,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: None,
            by_month: Vec::new(),
            week_start: None,
        };
        let bad = || TextOnly::Unreadable;
        for (k, v) in &self.parts {
            match k.as_str() {
                "FREQ" => {}
                "INTERVAL" => {
                    facts.interval = v.parse::<u16>().ok().filter(|n| *n >= 1).ok_or_else(bad)?
                }
                "COUNT" => {
                    facts.count = Some(v.parse::<u32>().ok().filter(|n| *n >= 1).ok_or_else(bad)?)
                }
                "UNTIL" => facts.until = Some(Stamp::parse(v).ok_or_else(bad)?),
                "BYDAY" => {
                    facts.by_day = v
                        .split(',')
                        .map(ByDay::parse)
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(bad)?
                }
                // One day of the month is what PM's editor offers and what a date move re-anchors;
                // a list is a rule only the crate can expand.
                "BYMONTHDAY" if v.contains(',') => return Err(TextOnly::Part(k.clone())),
                "BYMONTHDAY" => {
                    facts.by_month_day = Some(
                        v.parse::<i8>()
                            .ok()
                            .filter(|d| (1..=31).contains(&d.abs()))
                            .ok_or_else(bad)?,
                    )
                }
                "BYMONTH" => {
                    facts.by_month = v
                        .split(',')
                        .map(|m| m.parse::<u8>().ok().filter(|m| (1..=12).contains(m)))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(bad)?
                }
                "WKST" => facts.week_start = Some(weekday_of(v).ok_or_else(bad)?),
                // BYSETPOS, BYWEEKNO, BYYEARDAY, BYHOUR… : the crate can expand them, but a time or a
                // date change can't be carried through them safely, and the editor can't show them.
                _ => return Err(TextOnly::Part(k.clone())),
            }
        }
        // RFC 5545 §3.3.10: COUNT and UNTIL "MUST NOT occur in the same 'recur'", and a numbered
        // BYDAY belongs to a MONTHLY or YEARLY rule only.
        if facts.count.is_some() && facts.until.is_some() {
            return Err(TextOnly::Unreadable);
        }
        if matches!(freq, Freq::Daily | Freq::Weekly)
            && facts.by_day.iter().any(|d| d.nth.is_some())
        {
            return Err(TextOnly::Unreadable);
        }
        Ok(facts)
    }
}

/// An EXDATE or RDATE line: its head as it came, its dates read in the line's zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dates {
    /// The property name, canonical, and its parameters as they came.
    head: String,
    /// The line's TZID; `None` reads a wall-clock value in the series' own zone.
    pub zone: Option<Tz>,
    pub values: Vec<Stamp>,
}

impl Dates {
    fn parse(name: &str, params: &str, value: &str) -> Option<Dates> {
        let mut zone = None;
        for param in params.split(';').filter(|p| !p.is_empty()) {
            let (k, v) = param.split_once('=')?;
            match k.trim().to_ascii_uppercase().as_str() {
                "TZID" => zone = Some(v.trim().trim_matches('"').parse::<Tz>().ok()?),
                // A PERIOD (an RDATE with its own length) isn't an occurrence start.
                "VALUE" if v.trim().eq_ignore_ascii_case("PERIOD") => return None,
                _ => {}
            }
        }
        let values = value
            .split(',')
            .map(|v| Stamp::parse(&v.trim().to_ascii_uppercase()))
            .collect::<Option<Vec<_>>>()?;
        Some(Dates {
            head: format!("{name}{params}"),
            zone,
            values,
        })
    }

    /// The same line holding `values` instead.
    pub fn holding(&self, values: Vec<Stamp>) -> Dates {
        Dates {
            head: self.head.clone(),
            zone: self.zone,
            values,
        }
    }

    pub fn line(&self) -> String {
        let values: Vec<String> = self.values.iter().map(Stamp::text).collect();
        format!("{}:{}", self.head, values.join(","))
    }

    /// Where `value` (one of this line's) falls in the series.
    pub fn point(&self, value: &Stamp, start: &SeriesStart) -> Option<Point> {
        value.point(self.zone, start)
    }
}

/// One date value as written: a day, a UTC time, or a wall-clock time in some zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    Day(NaiveDate),
    Utc(NaiveDateTime),
    Wall(NaiveDateTime),
}

impl Stamp {
    fn parse(v: &str) -> Option<Stamp> {
        if v.len() == 8 {
            return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(Stamp::Day);
        }
        match v.strip_suffix('Z') {
            Some(t) => NaiveDateTime::parse_from_str(t, "%Y%m%dT%H%M%S")
                .ok()
                .map(Stamp::Utc),
            None => NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
                .ok()
                .map(Stamp::Wall),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Stamp::Day(d) => d.format("%Y%m%d").to_string(),
            Stamp::Utc(t) => t.format("%Y%m%dT%H%M%SZ").to_string(),
            Stamp::Wall(t) => t.format("%Y%m%dT%H%M%S").to_string(),
        }
    }

    /// Where this falls in a series starting at `start`, reading a wall-clock value in `zone` (or the
    /// series' own zone) as RFC 5545 does, a skipped hour included. An all-day series compares days.
    pub fn point(&self, zone: Option<Tz>, start: &SeriesStart) -> Option<Point> {
        let all_day = matches!(start, SeriesStart::AllDay(_));
        Some(match *self {
            Stamp::Day(d) => Point::Day(d),
            Stamp::Utc(t) if all_day => Point::Day(t.date()),
            Stamp::Utc(t) => Point::At(Utc.from_utc_datetime(&t)),
            Stamp::Wall(t) if all_day => Point::Day(t.date()),
            Stamp::Wall(t) => Point::At(rfc_instant(zone.unwrap_or(start.zone()), t)?),
        })
    }

    pub fn from_instant(t: DateTime<Utc>) -> Stamp {
        Stamp::Utc(t.naive_utc())
    }
}

/// A rule PM can change, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFacts {
    pub freq: Freq,
    pub interval: u16,
    pub count: Option<u32>,
    pub until: Option<Stamp>,
    pub by_day: Vec<ByDay>,
    pub by_month_day: Option<i8>,
    pub by_month: Vec<u8>,
    pub week_start: Option<Weekday>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// One BYDAY entry: a weekday, numbered (`2TU`, `-1FR`) or not (`MO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByDay {
    pub nth: Option<i8>,
    pub weekday: Weekday,
}

impl ByDay {
    fn parse(v: &str) -> Option<ByDay> {
        let v = v.trim();
        let split = v.len().checked_sub(2)?;
        let (n, day) = (v.get(..split)?, v.get(split..)?);
        let nth = match n {
            "" => None,
            n => Some(
                n.strip_prefix('+')
                    .unwrap_or(n)
                    .parse::<i8>()
                    .ok()
                    .filter(|n| *n != 0 && n.abs() <= 53)?,
            ),
        };
        Some(ByDay {
            nth,
            weekday: weekday_of(day)?,
        })
    }

    pub fn text(&self) -> String {
        match self.nth {
            Some(n) => format!("{n}{}", weekday_code(self.weekday)),
            None => weekday_code(self.weekday).to_string(),
        }
    }
}

pub fn weekday_code(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

fn weekday_of(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// Why PM can only show a series' rule as text. Its times can't be changed, and it can't be split;
/// its title and other details still can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextOnly {
    NoRule,
    SeveralRules,
    ExRule,
    SubDaily,
    /// A rule part PM doesn't change (BYSETPOS, BYWEEKNO, a list of days of the month…).
    Part(String),
    Unreadable,
}

impl fmt::Display for TextOnly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TextOnly::NoRule => write!(f, "it repeats only on set dates"),
            TextOnly::SeveralRules => write!(f, "it combines more than one repeat rule"),
            TextOnly::ExRule => write!(f, "it skips dates by a rule (EXRULE)"),
            TextOnly::SubDaily => write!(f, "it repeats more often than once a day"),
            TextOnly::Part(part) => write!(f, "its rule uses {part}, which PM doesn't change"),
            TextOnly::Unreadable => write!(f, "PM couldn't read its repeat rule"),
        }
    }
}

/// The series' one repeat rule, read, when PM can change the series' times or split it; otherwise
/// why it can only show it. Every line has to be readable: a series is only changed whole.
pub fn series_rule(lines: &[Line]) -> Result<(&Rule, RuleFacts), TextOnly> {
    if lines.iter().any(|l| l.kind == Kind::ExRule) {
        return Err(TextOnly::ExRule);
    }
    if lines.iter().any(|l| l.kind == Kind::Unreadable) {
        return Err(TextOnly::Unreadable);
    }
    let mut rules = lines.iter().filter_map(|l| match &l.kind {
        Kind::Rule(rule) => Some(rule),
        _ => None,
    });
    let rule = rules.next().ok_or(TextOnly::NoRule)?;
    if rules.next().is_some() {
        return Err(TextOnly::SeveralRules);
    }
    Ok((rule, rule.facts()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.raw.as_str()).collect()
    }

    fn shape(lines: &[&str]) -> Result<RuleFacts, TextOnly> {
        series_rule(&parse(lines)).map(|(_, facts)| facts)
    }

    /// T1: parsing keeps every line's exact text, the order of a rule's parts and a WKST included.
    #[test]
    fn untouched_lines_come_back_byte_for_byte() {
        for lines in [
            vec!["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            vec!["RRULE:FREQ=WEEKLY;WKST=SU;BYDAY=MO,WE;UNTIL=20261231T235959Z"],
            vec![
                "EXDATE;TZID=Europe/London:20261012T090000",
                "RRULE:FREQ=DAILY",
            ],
            vec!["rrule:freq=daily"],
            vec!["X-UNKNOWN:whatever", "RRULE:FREQ=DAILY"],
        ] {
            assert_eq!(raw(&parse(&lines)), lines);
        }
    }

    #[test]
    fn an_exdate_is_read_in_its_own_zone() {
        let lines = parse(&[
            "EXDATE;TZID=Europe/London:20261012T090000",
            "RRULE:FREQ=DAILY",
        ]);
        let Kind::ExDate(dates) = &lines[0].kind else {
            panic!("{:?}", lines[0]);
        };
        assert_eq!(dates.zone, Some(chrono_tz::Europe::London));
        let nine = NaiveDate::from_ymd_opt(2026, 10, 12)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        assert_eq!(dates.values, vec![Stamp::Wall(nine)]);
        // 09:00 in London (BST) is 08:00 UTC, whichever zone the series repeats in.
        let start = SeriesStart::Timed {
            local: nine,
            zone: chrono_tz::America::New_York,
        };
        assert_eq!(
            dates.point(&dates.values[0], &start),
            Some(Point::At(
                Utc.with_ymd_and_hms(2026, 10, 12, 8, 0, 0).unwrap()
            ))
        );
    }

    #[test]
    fn names_parts_and_values_are_read_whatever_their_case() {
        let facts = shape(&["rrule:freq=weekly;byday=mo,we;interval=2"]).unwrap();
        assert_eq!(facts.freq, Freq::Weekly);
        assert_eq!(facts.interval, 2);
        assert_eq!(
            facts.by_day.iter().map(ByDay::text).collect::<Vec<_>>(),
            ["MO", "WE"]
        );
    }

    #[test]
    fn shapes_pm_only_shows_as_text_say_why() {
        assert_eq!(
            shape(&["EXRULE:FREQ=WEEKLY", "RRULE:FREQ=DAILY"]),
            Err(TextOnly::ExRule)
        );
        assert_eq!(
            shape(&["RRULE:FREQ=DAILY", "RRULE:FREQ=WEEKLY"]),
            Err(TextOnly::SeveralRules)
        );
        assert_eq!(shape(&["RRULE:FREQ=HOURLY"]), Err(TextOnly::SubDaily));
        assert_eq!(
            shape(&["RRULE:BYMINUTE=0;FREQ=MINUTELY"]),
            Err(TextOnly::SubDaily)
        );
        assert_eq!(shape(&["RDATE:20261012T090000Z"]), Err(TextOnly::NoRule));
        assert_eq!(
            shape(&["RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2"]),
            Err(TextOnly::Part("BYSETPOS".into()))
        );
        assert_eq!(
            shape(&["RRULE:FREQ=MONTHLY;BYMONTHDAY=1,15"]),
            Err(TextOnly::Part("BYMONTHDAY".into()))
        );
        for unreadable in [
            "RRULE:FREQ=DAILY;COUNT=5;UNTIL=20261231T235959Z",
            "RRULE:FREQ=WEEKLY;BYDAY=2TU",
            "RRULE:FREQ=DAILY;INTERVAL=0",
            "RRULE:FREQ=DAILY;FREQ=WEEKLY",
            "RRULE:INTERVAL=2",
            "RRULE:FREQ=FORTNIGHTLY",
            "RRULE:FREQ=MONTHLY;BYMONTHDAY=32",
            "RRULE:FREQ=DAILY;UNTIL=tomorrow",
        ] {
            assert_eq!(
                shape(&[unreadable]),
                Err(TextOnly::Unreadable),
                "{unreadable}"
            );
        }
        // A line with more parts than any rule has is unread, without looking at them all.
        let parts: Vec<String> = (0..200_000).map(|i| format!("X{i}=1")).collect();
        let hostile = format!("RRULE:FREQ=DAILY;{}", parts.join(";"));
        assert_eq!(parse(&[&hostile])[0].kind, Kind::Unreadable);
        // Every other line has to be readable too: a series is only ever changed whole.
        for other in [
            "EXDATE;TZID=Mars/Olympus:20261012T090000",
            "RDATE;VALUE=PERIOD:20261012T090000Z/PT1H",
            "X-UNKNOWN:whatever",
        ] {
            assert_eq!(
                shape(&["RRULE:FREQ=DAILY", other]),
                Err(TextOnly::Unreadable),
                "{other}"
            );
        }
    }

    #[test]
    fn a_changed_part_keeps_its_place_and_a_new_one_goes_last() {
        let lines = parse(&["RRULE:FREQ=WEEKLY;WKST=SU;UNTIL=20261231T235959Z;BYDAY=MO"]);
        let Kind::Rule(rule) = &lines[0].kind else {
            panic!()
        };
        assert_eq!(
            rule.with("UNTIL", "20261012T075959Z").line(),
            "RRULE:FREQ=WEEKLY;WKST=SU;UNTIL=20261012T075959Z;BYDAY=MO"
        );
        assert_eq!(
            rule.without("UNTIL").with("COUNT", "3").line(),
            "RRULE:FREQ=WEEKLY;WKST=SU;BYDAY=MO;COUNT=3"
        );
        // A lower-case rule is written back canonical once PM changes it.
        let lower = parse(&["rrule:freq=daily;count=10"]);
        let Kind::Rule(rule) = &lower[0].kind else {
            panic!()
        };
        assert_eq!(rule.without("COUNT").line(), "RRULE:FREQ=DAILY");
    }

    #[test]
    fn numbered_weekdays_read_with_their_sign() {
        let facts = shape(&["RRULE:FREQ=MONTHLY;BYDAY=-1FR"]).unwrap();
        assert_eq!(
            facts.by_day,
            vec![ByDay {
                nth: Some(-1),
                weekday: Weekday::Fri
            }]
        );
        let plus = shape(&["RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=+2TU"]).unwrap();
        assert_eq!(plus.by_day[0].text(), "2TU");
        assert_eq!(plus.by_month, vec![10]);
    }
}
