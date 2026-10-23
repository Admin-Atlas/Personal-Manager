// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Times as people set them, turned into what Google stores. Everything takes a named zone and
//! nothing falls back to UTC (rule R6): both of PM's zone resolvers default to UTC today, which is
//! harmless for reading and wrong for writing, so a save without a real zone is refused.
//!
//! Two rules decide the awkward hours. A wall-clock time the clocks skip (the spring-forward gap) is
//! refused with a reason, not nudged. A time that happens twice (the autumn fall-back) takes the
//! first, which is what Google Calendar's own editor does.

use std::fmt;

use chrono::{
    DateTime, Days, FixedOffset, LocalResult, NaiveDate, NaiveTime, SecondsFormat, TimeDelta,
    TimeZone, Timelike,
};
use chrono_tz::Tz;
use serde_json::{json, Value};

use super::dto::{MoveTo, TimeDraft};
use crate::calendar_recur::wire;
use crate::calendar_recur::SeriesStart;

/// Why a time can't be saved as given. Shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeError {
    /// PM couldn't tell which time zone this computer is in.
    NoZone,
    UnknownZone(String),
    BadDate(String),
    BadTime(String),
    /// The clocks skip that time in that zone.
    Gap {
        zone: String,
        wall: String,
    },
    EndBeforeStart,
    /// A drop lands on the second time an hour happens in the event's zone (the clocks go back), and
    /// a wall time can only name the first (R6), so PM can't set it (C16a).
    RepeatedHour {
        zone: String,
        wall: String,
    },
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeError::NoZone => write!(
                f,
                "PM couldn't tell which time zone this computer is in, so it can't save a time. \
                 Set the time zone in your system settings and try again."
            ),
            TimeError::UnknownZone(z) => write!(f, "\"{z}\" isn't a time zone PM knows."),
            TimeError::BadDate(d) => write!(f, "\"{d}\" isn't a date (expected YYYY-MM-DD)."),
            TimeError::BadTime(t) => write!(f, "\"{t}\" isn't a time (expected HH:MM)."),
            TimeError::Gap { zone, wall } => write!(
                f,
                "{wall} doesn't happen in {zone}: the clocks skip it that night. Pick a time \
                 outside that hour."
            ),
            TimeError::EndBeforeStart => write!(f, "The event has to end after it starts."),
            // No "set it in Edit": the editor also sets only the first (C16 critique C2-M10).
            TimeError::RepeatedHour { zone, wall } => write!(
                f,
                "{wall} happens twice in {zone} that night (the clocks go back), and PM can only \
                 set the first."
            ),
        }
    }
}

/// The device zone the webview reports, as a zone. Missing or empty is an error, never UTC.
pub fn parse_device_zone(zone: Option<&str>) -> Result<Tz, TimeError> {
    match zone.map(str::trim).filter(|z| !z.is_empty()) {
        Some(z) => parse_zone(z),
        None => Err(TimeError::NoZone),
    }
}

/// An IANA zone name as a zone. The three parsers here are the repeat menu's own ([`wire`]), so
/// a save and the menu's words can't read the same text two ways (C11 critique 1 L6).
pub fn parse_zone(name: &str) -> Result<Tz, TimeError> {
    wire::parse_zone(name).ok_or_else(|| TimeError::UnknownZone(name.trim().to_string()))
}

fn parse_date(s: &str) -> Result<NaiveDate, TimeError> {
    wire::parse_date(s).ok_or_else(|| TimeError::BadDate(s.into()))
}

fn parse_time(s: &str) -> Result<NaiveTime, TimeError> {
    wire::parse_time(s).ok_or_else(|| TimeError::BadTime(s.trim().into()))
}

/// Where a draft's event starts, as a series would, for a repeat rule to start from (C11): read as
/// [`wire::series_start`] reads the repeat menu's start, with the same parsers in the same order,
/// so the menu's words and the save start a rule on the same day. A time the clocks skip isn't
/// refused here: the rule may move the start, and the day it lands on is checked there.
pub fn series_start(draft: &TimeDraft) -> Result<SeriesStart, TimeError> {
    match draft {
        TimeDraft::Timed {
            start_date,
            start_time,
            start_zone,
            ..
        } => {
            let (date, time) = (parse_date(start_date)?, parse_time(start_time)?);
            Ok(SeriesStart::Timed {
                local: date.and_time(time),
                zone: parse_zone(start_zone)?,
            })
        }
        TimeDraft::AllDay { first_day, .. } => Ok(SeriesStart::AllDay(parse_date(first_day)?)),
    }
}

/// The draft moved `days` days later, start and end alike, wall times and zones kept: where a repeat
/// rule moved its start, the end goes with it and the event keeps its length.
pub fn shifted(draft: &TimeDraft, days: i64) -> Result<TimeDraft, TimeError> {
    let by = |date: &str| -> Result<String, TimeError> {
        TimeDelta::try_days(days)
            .and_then(|delta| parse_date(date).ok()?.checked_add_signed(delta))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .ok_or_else(|| TimeError::BadDate(date.into()))
    };
    Ok(match draft {
        TimeDraft::Timed {
            start_date,
            start_time,
            start_zone,
            end_date,
            end_time,
            end_zone,
        } => TimeDraft::Timed {
            start_date: by(start_date)?,
            start_time: start_time.clone(),
            start_zone: start_zone.clone(),
            end_date: by(end_date)?,
            end_time: end_time.clone(),
            end_zone: end_zone.clone(),
        },
        TimeDraft::AllDay {
            first_day,
            last_day,
        } => TimeDraft::AllDay {
            first_day: by(first_day)?,
            last_day: by(last_day)?,
        },
    })
}

/// The instant a wall-clock time names in `zone`. The skipped hour is an error; a repeated one takes
/// its first occurrence.
pub fn resolve_wall_time(
    zone: Tz,
    date: NaiveDate,
    time: NaiveTime,
) -> Result<DateTime<Tz>, TimeError> {
    match zone.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(t) => Ok(t),
        LocalResult::Ambiguous(a, b) => Ok(a.min(b)),
        LocalResult::None => Err(TimeError::Gap {
            zone: zone.name().to_string(),
            wall: format!("{} {}", date.format("%d-%m-%Y"), time.format("%H:%M")),
        }),
    }
}

/// Google's all-day `end.date` (the day after the last one covered) from the last day covered.
pub fn exclusive_end(last_day: NaiveDate) -> NaiveDate {
    last_day
        .checked_add_days(Days::new(1))
        .unwrap_or(NaiveDate::MAX)
}

/// The last day an all-day event covers, from Google's exclusive `end.date`. A malformed end that
/// isn't after the start reads as a one-day event.
pub fn last_day_covered(start: NaiveDate, exclusive_end: NaiveDate) -> NaiveDate {
    exclusive_end
        .checked_sub_days(Days::new(1))
        .filter(|d| *d >= start)
        .unwrap_or(start)
}

/// A start or end node as the editor's date, `HH:MM` and zone: the node's own zone, else
/// `fallback`.
fn wall_parts(node: &Value, fallback: &str) -> Option<(String, String, String)> {
    let at = DateTime::parse_from_rfc3339(node.get("dateTime")?.as_str()?.trim()).ok()?;
    let zone_name = node
        .get("timeZone")
        .and_then(Value::as_str)
        .unwrap_or(fallback);
    let zone = parse_zone(zone_name)
        .or_else(|_| parse_zone(fallback))
        .ok()?;
    let local = at.with_timezone(&zone);
    Some((
        local.format("%Y-%m-%d").to_string(),
        local.format("%H:%M").to_string(),
        zone.name().to_string(),
    ))
}

/// An event's times as the editor shows them: an all-day one's first and last days covered, a
/// timed one's wall times, each half in its node's own zone, else `fallback_zone` (the calendar's,
/// else this computer's; never a guessed UTC, R6). The editor opens on this, and a one-off given a
/// repeat starts from it when the user left its time alone (C11 §10), so both read it alike.
/// `None` when its start or end can't be read.
pub fn time_draft_of(event: &Value, fallback_zone: &str) -> Option<TimeDraft> {
    let start = event.get("start")?;
    let end = event.get("end")?;
    match start.get("date").and_then(Value::as_str) {
        Some(first) => {
            let first_day = NaiveDate::parse_from_str(first, "%Y-%m-%d").ok()?;
            let end_day = end
                .get("date")
                .and_then(Value::as_str)
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .unwrap_or(first_day);
            Some(TimeDraft::AllDay {
                first_day: first.to_string(),
                last_day: last_day_covered(first_day, end_day)
                    .format("%Y-%m-%d")
                    .to_string(),
            })
        }
        None => {
            let (start_date, start_time, start_zone) = wall_parts(start, fallback_zone)?;
            let (end_date, end_time, end_zone) = wall_parts(end, &start_zone)?;
            Some(TimeDraft::Timed {
                start_date,
                start_time,
                start_zone,
                end_date,
                end_time,
                end_zone,
            })
        }
    }
}

/// Start and end as Google's event resource holds them.
#[derive(Debug, Clone, PartialEq)]
pub struct GoogleWhen {
    pub start: Value,
    pub end: Value,
    pub all_day: bool,
}

/// A time draft as Google's `start` and `end`, for a new event. A timed event keeps its own
/// zone(s): `dateTime` with that zone's offset, plus `timeZone`, so Google shows it as the user set it
/// and a repeating event follows that zone's daylight saving.
pub fn resolve(draft: &TimeDraft) -> Result<GoogleWhen, TimeError> {
    resolve_against(draft, None, None)
}

/// As [`resolve`], against the event's current `start` and `end` as Google holds them. A half whose
/// wall date, time and zone are what Google already holds keeps Google's exact value, so changing one
/// half never moves the other: the editor's `HH:MM` can't say which of a repeated hour's two 01:30s
/// it means, or carry seconds, and re-resolving an untouched half would silently pick the first
/// 01:30. Only a half the user actually changed takes the "first occurrence" rule.
pub fn resolve_against(
    draft: &TimeDraft,
    current_start: Option<&Value>,
    current_end: Option<&Value>,
) -> Result<GoogleWhen, TimeError> {
    match draft {
        TimeDraft::Timed {
            start_date,
            start_time,
            start_zone,
            end_date,
            end_time,
            end_zone,
        } => {
            let (start, start_at) = half(start_date, start_time, start_zone, current_start)?;
            let (end, end_at) = half(end_date, end_time, end_zone, current_end)?;
            if end_at <= start_at {
                return Err(TimeError::EndBeforeStart);
            }
            Ok(GoogleWhen {
                start,
                end,
                all_day: false,
            })
        }
        TimeDraft::AllDay {
            first_day,
            last_day,
        } => {
            let (first, last) = (parse_date(first_day)?, parse_date(last_day)?);
            if last < first {
                return Err(TimeError::EndBeforeStart);
            }
            Ok(GoogleWhen {
                start: json!({ "date": first.format("%Y-%m-%d").to_string() }),
                end: json!({ "date": exclusive_end(last).format("%Y-%m-%d").to_string() }),
                all_day: true,
            })
        }
    }
}

fn timed(t: DateTime<Tz>) -> Value {
    json!({
        "dateTime": t.to_rfc3339_opts(SecondsFormat::Secs, false),
        "timeZone": t.timezone().name(),
    })
}

/// One half of a timed draft, with its instant: Google's own node when it already names this wall
/// time, else the wall time resolved.
fn half(
    date: &str,
    time: &str,
    zone: &str,
    current: Option<&Value>,
) -> Result<(Value, DateTime<FixedOffset>), TimeError> {
    let tz = parse_zone(zone)?;
    let (date, time) = (parse_date(date)?, parse_time(time)?);
    if let Some(at) = current.and_then(|node| held_wall_time(node, tz, date, time)) {
        return Ok((current.cloned().unwrap_or(Value::Null), at));
    }
    let at = resolve_wall_time(tz, date, time)?;
    Ok((timed(at), at.fixed_offset()))
}

/// The instant a Google start/end `node` holds, when it names wall time `date` `time` (to the
/// minute) in the same zone `tz`. A node with no `timeZone` (an event made through the API, say) is
/// read in `tz`, the zone the editor showed it in: the same wall time there is the same instant.
fn held_wall_time(
    node: &Value,
    tz: Tz,
    date: NaiveDate,
    time: NaiveTime,
) -> Option<DateTime<FixedOffset>> {
    let at = DateTime::parse_from_rfc3339(node.get("dateTime")?.as_str()?.trim()).ok()?;
    if let Some(zone) = node.get("timeZone").and_then(Value::as_str) {
        if parse_zone(zone).ok()? != tz {
            return None;
        }
    }
    let local = at.with_timezone(&tz);
    (local.date_naive() == date && local.hour() == time.hour() && local.minute() == time.minute())
        .then_some(at)
}

/// The editor's time draft for a drop (C16a). `to` is wall time in `device`, this computer's zone:
/// each half the drop moved is resolved there (a time the clocks skip refused, a repeated one its
/// first) and said again in the zone Google keeps for that half, else the other half's, else
/// `device`, so a drag never changes which zone an event is in. A half the drop left where Google
/// has it is said as Google holds it, so [`resolve_against`] keeps Google's own node, seconds and
/// the second pass of a repeated hour included. An all-day drop needs no zone; a timed one without
/// it is refused (R6).
pub fn retime_draft(
    to: &MoveTo,
    device: Option<Tz>,
    fresh: &Value,
) -> Result<TimeDraft, TimeError> {
    let (start_date, start_time, end_date, end_time) = match to {
        MoveTo::AllDay {
            first_day,
            last_day,
        } => {
            return Ok(TimeDraft::AllDay {
                first_day: first_day.clone(),
                last_day: last_day.clone(),
            })
        }
        MoveTo::Timed {
            start_date,
            start_time,
            end_date,
            end_time,
        } => (start_date, start_time, end_date, end_time),
    };
    let device = device.ok_or(TimeError::NoZone)?;
    let (start, end) = (fresh.get("start"), fresh.get("end"));
    let start_zone = zone_of(start).or_else(|| zone_of(end)).unwrap_or(device);
    let end_zone = zone_of(end).or_else(|| zone_of(start)).unwrap_or(device);
    let (start_date, start_time, start_zone) =
        retimed_half(start_date, start_time, start, start_zone, device)?;
    let (end_date, end_time, end_zone) = retimed_half(end_date, end_time, end, end_zone, device)?;
    Ok(TimeDraft::Timed {
        start_date,
        start_time,
        start_zone,
        end_date,
        end_time,
        end_zone,
    })
}

/// The zone a timed start or end `node` names, when PM knows it. An all-day node has none.
fn zone_of(node: Option<&Value>) -> Option<Tz> {
    let node = node.filter(|n| n.get("dateTime").is_some_and(Value::is_string))?;
    parse_zone(node.get("timeZone")?.as_str()?).ok()
}

/// One half of a drop, `date` `time` on this computer (`device`), as the editor's date, `HH:MM` and
/// zone: Google's own instant in its own zone when `node` names that very wall time here (kept), else
/// the drop resolved on this computer and said in `zone`. A wall time that happens twice in `zone`
/// can only name the first: a drop on the second is refused rather than moved an hour.
fn retimed_half(
    date: &str,
    time: &str,
    node: Option<&Value>,
    zone: Tz,
    device: Tz,
) -> Result<(String, String, String), TimeError> {
    let (date, time) = (parse_date(date)?, parse_time(time)?);
    let held = node
        .and_then(|n| n.get("dateTime"))
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok());
    if let Some(at) = held {
        let here = at.with_timezone(&device);
        if here.date_naive() == date && here.hour() == time.hour() && here.minute() == time.minute()
        {
            let own = node
                .and_then(|n| n.get("timeZone"))
                .and_then(Value::as_str)
                .and_then(|z| parse_zone(z).ok())
                .unwrap_or(device);
            return Ok(wall_in(at.with_timezone(&own)));
        }
    }
    let at = resolve_wall_time(device, date, time)?;
    let local = at.with_timezone(&zone);
    let (day, clock) = (local.date_naive(), local.time());
    if resolve_wall_time(zone, day, clock)? != at {
        return Err(TimeError::RepeatedHour {
            zone: zone.name().to_string(),
            wall: format!("{} {}", day.format("%d-%m-%Y"), clock.format("%H:%M")),
        });
    }
    Ok(wall_in(local))
}

/// An instant as the editor's date, `HH:MM` and zone.
fn wall_in(t: DateTime<Tz>) -> (String, String, String) {
    (
        t.format("%Y-%m-%d").to_string(),
        t.format("%H:%M").to_string(),
        t.timezone().name().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed_draft(sd: &str, st: &str, sz: &str, ed: &str, et: &str, ez: &str) -> TimeDraft {
        TimeDraft::Timed {
            start_date: sd.into(),
            start_time: st.into(),
            start_zone: sz.into(),
            end_date: ed.into(),
            end_time: et.into(),
            end_zone: ez.into(),
        }
    }

    #[test]
    fn a_missing_device_zone_is_refused_never_utc() {
        assert_eq!(parse_device_zone(None), Err(TimeError::NoZone));
        assert_eq!(parse_device_zone(Some("  ")), Err(TimeError::NoZone));
        assert_eq!(
            parse_device_zone(Some("Mars/Olympus")),
            Err(TimeError::UnknownZone("Mars/Olympus".into()))
        );
        assert_eq!(
            parse_device_zone(Some("Europe/London")).unwrap(),
            chrono_tz::Europe::London
        );
    }

    /// London falls back on 25-10-2026: 01:30 happens twice, and the first (still BST) is taken.
    #[test]
    fn a_repeated_hour_takes_its_first_occurrence() {
        let t = resolve_wall_time(
            chrono_tz::Europe::London,
            NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(),
            NaiveTime::from_hms_opt(1, 30, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(t.to_rfc3339(), "2026-10-25T01:30:00+01:00");
        // New York falls back on 01-11-2026: the first 01:30 is still EDT.
        let ny = resolve_wall_time(
            chrono_tz::America::New_York,
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            NaiveTime::from_hms_opt(1, 30, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(ny.to_rfc3339(), "2026-11-01T01:30:00-04:00");
    }

    /// London springs forward on 29-03-2026 and New York on 08-03-2026: the skipped times are refused.
    #[test]
    fn a_skipped_hour_is_refused_with_a_reason() {
        let gap = resolve_wall_time(
            chrono_tz::Europe::London,
            NaiveDate::from_ymd_opt(2026, 3, 29).unwrap(),
            NaiveTime::from_hms_opt(1, 30, 0).unwrap(),
        );
        assert_eq!(
            gap,
            Err(TimeError::Gap {
                zone: "Europe/London".into(),
                wall: "29-03-2026 01:30".into()
            })
        );
        assert!(gap.unwrap_err().to_string().contains("the clocks skip it"));
        assert!(matches!(
            resolve_wall_time(
                chrono_tz::America::New_York,
                NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
                NaiveTime::from_hms_opt(2, 30, 0).unwrap(),
            ),
            Err(TimeError::Gap { .. })
        ));
    }

    #[test]
    fn a_timed_event_keeps_its_own_zones() {
        // A flight: leaves London at 09:00, lands in New York at 12:00 local.
        let when = resolve(&timed_draft(
            "2026-07-01",
            "09:00",
            "Europe/London",
            "2026-07-01",
            "12:00",
            "America/New_York",
        ))
        .unwrap();
        assert!(!when.all_day);
        assert_eq!(
            when.start,
            json!({ "dateTime": "2026-07-01T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            when.end,
            json!({ "dateTime": "2026-07-01T12:00:00-04:00", "timeZone": "America/New_York" })
        );
    }

    #[test]
    fn an_end_at_or_before_the_start_is_refused() {
        let same = timed_draft(
            "2026-07-01",
            "09:00",
            "Europe/London",
            "2026-07-01",
            "09:00",
            "Europe/London",
        );
        assert_eq!(resolve(&same), Err(TimeError::EndBeforeStart));
        // 09:00 in London (BST) is 04:00 in New York (EDT): ending at 03:59 there is before the start.
        let earlier = timed_draft(
            "2026-07-01",
            "09:00",
            "Europe/London",
            "2026-07-01",
            "03:59",
            "America/New_York",
        );
        assert_eq!(resolve(&earlier), Err(TimeError::EndBeforeStart));
        let days = TimeDraft::AllDay {
            first_day: "2026-07-02".into(),
            last_day: "2026-07-01".into(),
        };
        assert_eq!(resolve(&days), Err(TimeError::EndBeforeStart));
    }

    /// Google's all-day end is the day after the last day covered; leap days included.
    #[test]
    fn all_day_ends_round_trip_through_googles_exclusive_end() {
        let when = resolve(&TimeDraft::AllDay {
            first_day: "2028-02-29".into(),
            last_day: "2028-02-29".into(),
        })
        .unwrap();
        assert!(when.all_day);
        assert_eq!(when.start, json!({ "date": "2028-02-29" }));
        assert_eq!(when.end, json!({ "date": "2028-03-01" }));

        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        for (first, last) in [
            ("2028-02-28", "2028-02-29"),
            ("2026-12-31", "2027-01-01"),
            ("2026-10-10", "2026-10-12"),
        ] {
            let end = exclusive_end(d(last));
            assert_eq!(last_day_covered(d(first), end), d(last), "{first}..{last}");
        }
        // A malformed end that isn't after the start reads as one day.
        assert_eq!(
            last_day_covered(d("2026-10-10"), d("2026-10-10")),
            d("2026-10-10")
        );
    }

    #[test]
    fn unparseable_fields_say_which() {
        let bad_date = timed_draft(
            "10/12/2026",
            "09:00",
            "Europe/London",
            "2026-10-12",
            "10:00",
            "Europe/London",
        );
        assert_eq!(
            resolve(&bad_date),
            Err(TimeError::BadDate("10/12/2026".into()))
        );
        let bad_time = timed_draft(
            "2026-10-12",
            "9am",
            "Europe/London",
            "2026-10-12",
            "10:00",
            "Europe/London",
        );
        assert_eq!(resolve(&bad_time), Err(TimeError::BadTime("9am".into())));
    }

    // --- the half the user didn't touch keeps Google's exact value (London falls back 25-10-2026) ---

    fn node(date_time: &str) -> Value {
        json!({ "dateTime": date_time, "timeZone": "Europe/London" })
    }

    /// The start is the SECOND 01:30 (GMT); only the end moves. The start must stay put.
    #[test]
    fn an_end_only_change_keeps_a_start_in_the_second_repeated_hour() {
        let start = node("2026-10-25T01:30:00Z");
        let end = node("2026-10-25T02:30:00Z");
        let draft = timed_draft(
            "2026-10-25",
            "01:30",
            "Europe/London",
            "2026-10-25",
            "03:00",
            "Europe/London",
        );
        let when = resolve_against(&draft, Some(&start), Some(&end)).unwrap();
        assert_eq!(
            when.start, start,
            "the untouched start is Google's own value"
        );
        assert_eq!(when.end["dateTime"], "2026-10-25T03:00:00+00:00");
        // Without the current value the editor's 01:30 would mean the first one, an hour earlier.
        assert_eq!(
            resolve(&draft).unwrap().start["dateTime"],
            "2026-10-25T01:30:00+01:00"
        );
    }

    /// 23:00 BST to the second 01:30 (GMT); only the start moves. The end must stay put.
    #[test]
    fn a_start_only_change_keeps_an_end_in_the_second_repeated_hour() {
        let end = node("2026-10-25T01:30:00Z");
        let draft = timed_draft(
            "2026-10-24",
            "22:30",
            "Europe/London",
            "2026-10-25",
            "01:30",
            "Europe/London",
        );
        let when =
            resolve_against(&draft, Some(&node("2026-10-24T22:00:00Z")), Some(&end)).unwrap();
        assert_eq!(when.end, end);
        assert_eq!(when.start["dateTime"], "2026-10-24T22:30:00+01:00");
    }

    /// A node Google holds without a zone (made through the API) is shown in the calendar's zone, and
    /// an untouched one is kept exactly, not re-read as the first of a repeated hour.
    #[test]
    fn an_untouched_half_without_a_zone_is_kept_exactly() {
        let end = json!({ "dateTime": "2026-10-25T01:30:00Z" }); // 01:30 GMT, the second one
        let draft = timed_draft(
            "2026-10-24",
            "22:30",
            "Europe/London",
            "2026-10-25",
            "01:30",
            "Europe/London",
        );
        let start = json!({ "dateTime": "2026-10-24T21:00:00Z" });
        let when = resolve_against(&draft, Some(&start), Some(&end)).unwrap();
        assert_eq!(when.end, end);
        assert_eq!(when.start["dateTime"], "2026-10-24T22:30:00+01:00");
    }

    /// 01:45 BST to 01:15 GMT is a valid half-hour event; read back as wall times both would be BST
    /// and the end would come before the start.
    #[test]
    fn an_event_across_the_fold_is_not_refused() {
        let (start, end) = (node("2026-10-25T00:45:00Z"), node("2026-10-25T01:15:00Z"));
        let draft = timed_draft(
            "2026-10-25",
            "01:45",
            "Europe/London",
            "2026-10-25",
            "01:15",
            "Europe/London",
        );
        let when = resolve_against(&draft, Some(&start), Some(&end)).unwrap();
        assert_eq!((when.start, when.end), (start, end));
        assert_eq!(resolve(&draft), Err(TimeError::EndBeforeStart));
    }

    /// Seconds Google holds survive an edit to the other half, and a zone change is a real change.
    #[test]
    fn seconds_survive_and_a_new_zone_is_a_change() {
        let start = node("2026-07-01T08:00:30Z"); // 09:00:30 BST
        let draft = timed_draft(
            "2026-07-01",
            "09:00",
            "Europe/London",
            "2026-07-01",
            "10:00",
            "Europe/London",
        );
        let when = resolve_against(&draft, Some(&start), None).unwrap();
        assert_eq!(when.start, start);
        let paris = timed_draft(
            "2026-07-01",
            "09:00",
            "Europe/Paris",
            "2026-07-01",
            "10:00",
            "Europe/Paris",
        );
        let moved = resolve_against(&paris, Some(&start), None).unwrap();
        assert_eq!(
            moved.start,
            json!({ "dateTime": "2026-07-01T09:00:00+02:00", "timeZone": "Europe/Paris" })
        );
    }

    // --- a repeating event's start (C11) ---

    /// C11 critique 1 L6: the save reads a start exactly as the repeat menu does, the text it
    /// accepts and the part it refuses first alike.
    #[test]
    fn the_save_reads_a_start_as_the_repeat_menu_does() {
        use crate::calendar_recur::wire::{RepeatStart, StartError};
        for (date, time, zone) in [
            ("2026-10-13", "09:00", "Europe/London"),
            (" 2026-10-13 ", " 09:00:30 ", " America/New_York "),
            ("2026-03-29", "01:30", "Europe/London"),
            ("13-10-2026", "09:00", "Europe/London"),
            ("2026-10-13", "9am", "Europe/London"),
            ("2026-10-13", "09:00", "Mars/Olympus"),
            ("soon", "late", "nowhere"),
            ("2026-10-13", "25:00", "nowhere"),
        ] {
            let draft = timed_draft(date, time, zone, date, time, zone);
            let menu = wire::series_start(&RepeatStart::Timed {
                date: date.into(),
                time: time.into(),
                zone: zone.into(),
                end: None,
            });
            let save = series_start(&draft);
            match (menu, save) {
                (Ok(a), Ok(b)) => assert_eq!(a, b),
                (Err(StartError::Date), Err(TimeError::BadDate(_)))
                | (Err(StartError::Time), Err(TimeError::BadTime(_)))
                | (Err(StartError::Zone), Err(TimeError::UnknownZone(_))) => {}
                _ => panic!("the save and the menu read {date} {time} {zone} differently"),
            }
        }
        let all_day = TimeDraft::AllDay {
            first_day: "2026-10-31".into(),
            last_day: "2026-11-01".into(),
        };
        assert_eq!(
            series_start(&all_day),
            Ok(SeriesStart::AllDay(
                NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
            ))
        );
    }

    /// A start the rule moved takes its end with it: the same wall times and zones, whole days on.
    #[test]
    fn a_draft_moves_whole_days_keeping_its_wall_times_and_zones() {
        let flight = timed_draft(
            "2026-10-13",
            "22:00",
            "Europe/London",
            "2026-10-14",
            "06:30",
            "America/New_York",
        );
        assert_eq!(
            shifted(&flight, 1),
            Ok(timed_draft(
                "2026-10-14",
                "22:00",
                "Europe/London",
                "2026-10-15",
                "06:30",
                "America/New_York",
            ))
        );
        assert_eq!(shifted(&flight, 0), Ok(flight));
        let days = TimeDraft::AllDay {
            first_day: "2026-10-31".into(),
            last_day: "2026-11-01".into(),
        };
        assert_eq!(
            shifted(&days, 2),
            Ok(TimeDraft::AllDay {
                first_day: "2026-11-02".into(),
                last_day: "2026-11-03".into(),
            })
        );
        let unread = TimeDraft::AllDay {
            first_day: "2026-10-31".into(),
            last_day: "someday".into(),
        };
        assert_eq!(
            shifted(&unread, 1),
            Err(TimeError::BadDate("someday".into()))
        );
    }

    // --- a drop's time (C16a): this computer's wall time, said in the event's own zone ---

    fn dropped(sd: &str, st: &str, ed: &str, et: &str) -> MoveTo {
        MoveTo::Timed {
            start_date: sd.into(),
            start_time: st.into(),
            end_date: ed.into(),
            end_time: et.into(),
        }
    }

    fn london_event(start: &str, end: &str) -> Value {
        json!({ "start": node(start), "end": node(end) })
    }

    /// A London event dragged on a New York laptop stays in London's zone: the drop is New York's
    /// wall time, said again in London's, so `resolve_against` sends London nodes.
    #[test]
    fn a_drop_keeps_the_events_own_zone() {
        let fresh = london_event("2026-11-03T09:00:00Z", "2026-11-03T10:00:00Z");
        let to = dropped("2026-11-03", "05:00", "2026-11-03", "06:00");
        let draft = retime_draft(&to, Some(chrono_tz::America::New_York), &fresh).unwrap();
        assert_eq!(
            draft,
            timed_draft(
                "2026-11-03",
                "10:00",
                "Europe/London",
                "2026-11-03",
                "11:00",
                "Europe/London",
            )
        );
        let when = resolve_against(&draft, fresh.get("start"), fresh.get("end")).unwrap();
        assert_eq!(when.start, node("2026-11-03T10:00:00+00:00"));
        assert_eq!(when.end, node("2026-11-03T11:00:00+00:00"));
    }

    /// A resize leaves the start where Google has it, seconds and the second 01:30 included: the
    /// start is said as Google holds it, and `resolve_against` keeps Google's node byte for byte.
    #[test]
    fn a_resize_keeps_the_untouched_half_byte_for_byte() {
        // 01:30:15 GMT on 25-10-2026: the second 01:30 in London.
        let start = node("2026-10-25T01:30:15Z");
        let fresh = json!({ "start": start, "end": node("2026-10-25T02:30:00Z") });
        let to = dropped("2026-10-25", "01:30", "2026-10-25", "03:00");
        let draft = retime_draft(&to, Some(chrono_tz::Europe::London), &fresh).unwrap();
        let when = resolve_against(&draft, fresh.get("start"), fresh.get("end")).unwrap();
        assert_eq!(when.start, start, "Google's own start");
        assert_eq!(
            when.end,
            node("2026-10-25T03:00:00+00:00"),
            "only the end moved"
        );
        // A node with no zone of its own is kept too, read on this computer.
        let bare = json!({ "dateTime": "2026-07-01T08:00:30Z" });
        let fresh = json!({ "start": bare, "end": { "dateTime": "2026-07-01T09:00:00Z" } });
        let to = dropped("2026-07-01", "09:00", "2026-07-01", "11:00");
        let draft = retime_draft(&to, Some(chrono_tz::Europe::London), &fresh).unwrap();
        let when = resolve_against(&draft, fresh.get("start"), fresh.get("end")).unwrap();
        assert_eq!(when.start, bare);
        // A London event on a New York laptop whose start is the second 01:30 there (01-11-2026,
        // 06:30 GMT): read again as New York's 01:30 it would be the first, an hour earlier. Kept.
        let start = node("2026-11-01T06:30:00Z");
        let fresh = json!({ "start": start, "end": node("2026-11-01T07:30:00Z") });
        let to = dropped("2026-11-01", "01:30", "2026-11-01", "03:00");
        let draft = retime_draft(&to, Some(chrono_tz::America::New_York), &fresh).unwrap();
        let when = resolve_against(&draft, fresh.get("start"), fresh.get("end")).unwrap();
        assert_eq!(when.start, start, "Google's own start, not an hour earlier");
        assert_eq!(when.end, node("2026-11-01T08:00:00+00:00"));
    }

    /// A drop in the hour this computer's clocks skip is refused, never nudged.
    #[test]
    fn a_drop_in_the_device_zones_skipped_hour_is_refused() {
        let fresh = london_event("2027-03-27T09:00:00Z", "2027-03-27T10:00:00Z");
        let to = dropped("2027-03-28", "01:30", "2027-03-28", "02:30");
        assert!(matches!(
            retime_draft(&to, Some(chrono_tz::Europe::London), &fresh),
            Err(TimeError::Gap { .. })
        ));
    }

    /// 21:30 in New York on 24-10-2026 is 01:30 GMT on the 25th: the second 01:30 in London, which
    /// a wall time there can't name. Refused with its words, never moved an hour; the first is fine.
    #[test]
    fn a_drop_on_the_events_second_pass_is_refused() {
        let fresh = london_event("2026-10-24T18:00:00Z", "2026-10-24T19:00:00Z");
        let second = dropped("2026-10-24", "21:30", "2026-10-24", "22:30");
        let refused = retime_draft(&second, Some(chrono_tz::America::New_York), &fresh);
        assert_eq!(
            refused,
            Err(TimeError::RepeatedHour {
                zone: "Europe/London".into(),
                wall: "25-10-2026 01:30".into(),
            })
        );
        let words = refused.unwrap_err().to_string();
        assert!(words.contains("happens twice in Europe/London"), "{words}");
        assert!(!words.contains("Edit"), "the editor can't set it either");
        // 20:30 in New York is the first 01:30 (BST): fine.
        let first = dropped("2026-10-24", "20:30", "2026-10-24", "20:45");
        let draft = retime_draft(&first, Some(chrono_tz::America::New_York), &fresh).unwrap();
        assert!(matches!(draft, TimeDraft::Timed { ref start_time, .. } if start_time == "01:30"));
    }

    /// An all-day event dropped in the hours takes this computer's zone, having none of its own;
    /// one dropped on days needs no zone at all.
    #[test]
    fn all_day_to_timed_uses_the_device_zone_and_all_day_needs_none() {
        let fresh = json!({ "start": { "date": "2026-10-12" }, "end": { "date": "2026-10-13" } });
        let to = dropped("2026-10-12", "14:00", "2026-10-12", "15:00");
        let draft = retime_draft(&to, Some(chrono_tz::America::New_York), &fresh).unwrap();
        assert_eq!(
            draft,
            timed_draft(
                "2026-10-12",
                "14:00",
                "America/New_York",
                "2026-10-12",
                "15:00",
                "America/New_York",
            )
        );
        let days = MoveTo::AllDay {
            first_day: "2026-10-14".into(),
            last_day: "2026-10-15".into(),
        };
        assert_eq!(
            retime_draft(&days, None, &fresh),
            Ok(TimeDraft::AllDay {
                first_day: "2026-10-14".into(),
                last_day: "2026-10-15".into(),
            })
        );
        // A half with no zone of its own takes the other half's.
        let half = json!({
            "start": { "dateTime": "2026-10-12T09:00:00Z" },
            "end": node("2026-10-12T10:00:00Z"),
        });
        let to = dropped("2026-10-12", "06:00", "2026-10-12", "07:00");
        let draft = retime_draft(&to, Some(chrono_tz::America::New_York), &half).unwrap();
        assert!(
            matches!(&draft, TimeDraft::Timed { start_zone, end_zone, .. }
                if start_zone == "Europe/London" && end_zone == "Europe/London")
        );
    }

    /// R6: a timed drop with no zone for this computer is refused, never read as UTC.
    #[test]
    fn a_timed_drop_without_a_zone_is_refused() {
        let fresh = london_event("2026-11-03T09:00:00Z", "2026-11-03T10:00:00Z");
        let to = dropped("2026-11-03", "05:00", "2026-11-03", "06:00");
        assert_eq!(retime_draft(&to, None, &fresh), Err(TimeError::NoZone));
    }
}
