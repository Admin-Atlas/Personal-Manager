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
    DateTime, Days, FixedOffset, LocalResult, NaiveDate, NaiveTime, SecondsFormat, TimeZone,
    Timelike,
};
use chrono_tz::Tz;
use serde_json::{json, Value};

use super::dto::TimeDraft;

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
            TimeError::EndBeforeStart => write!(f, "The event ends before it starts."),
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

/// An IANA zone name as a zone.
pub fn parse_zone(name: &str) -> Result<Tz, TimeError> {
    name.trim()
        .parse::<Tz>()
        .map_err(|_| TimeError::UnknownZone(name.trim().to_string()))
}

fn parse_date(s: &str) -> Result<NaiveDate, TimeError> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| TimeError::BadDate(s.into()))
}

fn parse_time(s: &str) -> Result<NaiveTime, TimeError> {
    let s = s.trim();
    NaiveTime::parse_from_str(s, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S"))
        .map_err(|_| TimeError::BadTime(s.into()))
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
/// minute) in the same zone `tz`.
fn held_wall_time(
    node: &Value,
    tz: Tz,
    date: NaiveDate,
    time: NaiveTime,
) -> Option<DateTime<FixedOffset>> {
    let at = DateTime::parse_from_rfc3339(node.get("dateTime")?.as_str()?.trim()).ok()?;
    if parse_zone(node.get("timeZone")?.as_str()?).ok()? != tz {
        return None;
    }
    let local = at.with_timezone(&tz);
    (local.date_naive() == date && local.hour() == time.hour() && local.minute() == time.minute())
        .then_some(at)
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
}
