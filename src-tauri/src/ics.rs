// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! iCalendar (.ics) feed parsing — the no-OAuth calendar path (Step 6). A Google
//! Calendar "secret address in iCal format" (or any ICS URL) needs no sign-in, so it
//! works even on accounts in Google's Advanced Protection Program, which blocks
//! unverified OAuth apps. We parse the feed into the same [`CalendarEvent`] shape the
//! OAuth path produces, so the agenda / name-match / chat reuse is identical.
//!
//! ICS is fiddly: lines are folded (RFC 5545 §3.1), times carry timezones (`TZID`,
//! `Z`, or floating), and events recur (`RRULE`). We unfold by hand, resolve times to
//! UTC with `chrono`/`chrono-tz`, and expand recurrences with the `rrule` crate,
//! bounded to the agenda window so a years-old weekly meeting doesn't blow up.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz as ChronoTz;

use crate::calendar::CalendarEvent;

/// Parse an ICS feed into events from yesterday through `window_days` ahead. `tz` is
/// the user's zone, used to anchor floating/all-day values (an ICS feed carries no
/// viewer zone). The window itself stays an absolute UTC instant range.
pub fn parse_feed(text: &str, feed_id: &str, window_days: i64, tz: ChronoTz) -> Vec<CalendarEvent> {
    let now = Utc::now();
    parse_feed_within(
        text,
        feed_id,
        now - Duration::days(1),
        now + Duration::days(window_days),
        tz,
    )
}

/// Defensive caps for a hostile or oversized feed: bound how many VEVENT blocks we
/// parse and how many expanded occurrences we keep. The 10 MiB body cap (see
/// `calendar::read_capped`) already bounds the input; these bound the work and the
/// memory the parse produces. Far above any real personal calendar.
const MAX_VEVENTS: usize = 50_000;
const MAX_EVENTS: usize = 100_000;

/// The window-pure core (so recurrence expansion is unit-testable without `now`).
pub fn parse_feed_within(
    text: &str,
    feed_id: &str,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
) -> Vec<CalendarEvent> {
    let lines = unfold(text);
    let mut out = Vec::new();
    for block in vevents(&lines).into_iter().take(MAX_VEVENTS) {
        if out.len() >= MAX_EVENTS {
            break;
        }
        out.extend(expand_vevent(&block, feed_id, win_start, win_end, tz));
    }
    out.truncate(MAX_EVENTS);
    out
}

/// Undo RFC 5545 line folding: a line starting with a space or tab continues the
/// previous one. Also strips trailing CR from CRLF feeds.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&line[1..]);
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}

/// Collect the property lines of each `VEVENT` block.
fn vevents(lines: &[String]) -> Vec<Vec<String>> {
    let mut blocks = Vec::new();
    let mut cur: Option<Vec<String>> = None;
    for l in lines {
        match l.as_str() {
            "BEGIN:VEVENT" => cur = Some(Vec::new()),
            "END:VEVENT" => {
                if let Some(b) = cur.take() {
                    blocks.push(b);
                }
            }
            _ => {
                if let Some(b) = cur.as_mut() {
                    b.push(l.clone());
                }
            }
        }
    }
    blocks
}

/// Turn one VEVENT into its in-window occurrences (one for a single event, many for a
/// recurring one).
fn expand_vevent(
    block: &[String],
    feed_id: &str,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
) -> Vec<CalendarEvent> {
    if find(block, "STATUS") == Some("CANCELLED") {
        return Vec::new();
    }
    let Some((start_params, start_val)) = find_prop(block, "DTSTART") else {
        return Vec::new();
    };
    let all_day = param(start_params, "VALUE") == Some("DATE")
        || (!start_val.contains('T') && start_val.trim().len() == 8);

    let summary = find(block, "SUMMARY")
        .map(unescape)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(no title)".to_string());
    let location = find(block, "LOCATION")
        .map(unescape)
        .filter(|s| !s.is_empty());
    let description = find(block, "DESCRIPTION")
        .map(unescape)
        .filter(|s| !s.is_empty());
    let uid = find(block, "UID").unwrap_or("").to_string();

    let Some(start_anchor) = parse_any(start_val, param(start_params, "TZID"), all_day, tz) else {
        return Vec::new();
    };
    let end_anchor =
        find_prop(block, "DTEND").and_then(|(p, v)| parse_any(v, param(p, "TZID"), all_day, tz));
    let dur = end_anchor.map(|e| e - start_anchor);

    let starts: Vec<DateTime<Utc>> = if block.iter().any(|l| l.starts_with("RRULE")) {
        expand_rrule(block, win_start, win_end)
    } else if start_anchor <= win_end && end_anchor.unwrap_or(start_anchor) >= win_start {
        vec![start_anchor]
    } else {
        Vec::new()
    };

    starts
        .into_iter()
        .map(|s| {
            make_event(
                feed_id,
                &uid,
                &summary,
                &location,
                &description,
                s,
                dur,
                all_day,
                tz,
            )
        })
        .collect()
}

/// Expand an `RRULE` to its UTC occurrence-starts within the window. Feeds the
/// original DTSTART/RRULE/EXDATE/RDATE lines straight to `rrule` so it resolves the
/// timezone itself; a parse failure degrades to "no occurrences" rather than erroring.
fn expand_rrule(
    block: &[String],
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    // Guard against a pathological recurrence: a sub-daily FREQ (SECONDLY/MINUTELY/
    // HOURLY) with a far-past DTSTART and no COUNT/UNTIL forces the iterator to walk
    // millions of pre-window occurrences before reaching the agenda window — a CPU
    // hang on sync from one crafted feed line (`.all(366)` caps results, not the
    // walk). Such rules are meaningless in a day-level agenda, so skip them.
    if has_sub_daily_freq(block) {
        return Vec::new();
    }

    let spec = block
        .iter()
        .filter(|l| {
            l.starts_with("DTSTART")
                || l.starts_with("RRULE")
                || l.starts_with("EXDATE")
                || l.starts_with("RDATE")
        })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    let Ok(set) = spec.parse::<rrule::RRuleSet>() else {
        return Vec::new();
    };
    let after = win_start.with_timezone(&rrule::Tz::UTC);
    let before = win_end.with_timezone(&rrule::Tz::UTC);
    set.after(after)
        .before(before)
        .all(366)
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect()
}

/// True if the block's `RRULE` uses a sub-daily frequency — see `expand_rrule`.
fn has_sub_daily_freq(block: &[String]) -> bool {
    block.iter().any(|l| {
        let u = l.to_ascii_uppercase();
        u.starts_with("RRULE")
            && ["FREQ=SECONDLY", "FREQ=MINUTELY", "FREQ=HOURLY"]
                .iter()
                .any(|f| u.contains(f))
    })
}

// Builds a CalendarEvent from its already-parsed parts; the many fields are the
// event's columns, not a sign the function should be split.
#[allow(clippy::too_many_arguments)]
fn make_event(
    feed_id: &str,
    uid: &str,
    summary: &str,
    location: &Option<String>,
    description: &Option<String>,
    start_utc: DateTime<Utc>,
    dur: Option<Duration>,
    all_day: bool,
    tz: ChronoTz,
) -> CalendarEvent {
    let (start, end) = if all_day {
        // The anchor is the user-zone midnight as a UTC instant; convert it *back* to
        // that zone before formatting the date, so an all-day event reads as the same
        // calendar day in every zone (UTC formatting would drift it east of UTC).
        let s = start_utc.with_timezone(&tz).format("%Y-%m-%d").to_string();
        let e = dur.map(|d| {
            (start_utc + d)
                .with_timezone(&tz)
                .format("%Y-%m-%d")
                .to_string()
        });
        (s, e)
    } else {
        let s = start_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let e = dur.map(|d| (start_utc + d).format("%Y-%m-%dT%H:%M:%SZ").to_string());
        (s, e)
    };
    CalendarEvent {
        id: format!("{feed_id}:{uid}:{start}"),
        calendar_id: feed_id.to_string(),
        summary: summary.to_string(),
        description: description.clone(),
        location: location.clone(),
        start,
        end,
        all_day,
        html_link: None,
        // The iCal UID is the durable cross-provider anchor; an empty UID stores as None.
        uid: (!uid.is_empty()).then(|| uid.to_string()),
    }
}

/// Resolve an ICS date/datetime value to a UTC instant. All-day values anchor to the
/// user's zone midnight; floating times (no TZID, no `Z`) resolve in the user's zone
/// too (an explicit `TZID`, or a trailing `Z`, always wins and keeps its exact
/// instant). See `make_event` for how all-day dates are formatted back from `tz`.
fn parse_any(
    value: &str,
    tzid: Option<&str>,
    all_day: bool,
    tz: ChronoTz,
) -> Option<DateTime<Utc>> {
    let v = value.trim();
    if all_day {
        let date = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        // Anchor at NOON, not midnight: a zone whose DST spring-forward lands exactly
        // at 00:00 (e.g. America/Havana, Africa/Cairo) has no local midnight that day,
        // so a midnight anchor is a `None` gap and the event silently vanishes. Noon is
        // never inside a 1-hour gap; `make_event` formats date-only, so the offset is
        // truncated and the stored civil date is unchanged in every zone.
        return tz
            .from_local_datetime(&date.and_hms_opt(12, 0, 0)?)
            .earliest()
            .map(|d| d.with_timezone(&Utc));
    }
    if let Some(stripped) = v.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        return Some(Utc.from_utc_datetime(&naive));
    }
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    match tzid.and_then(|t| t.parse::<ChronoTz>().ok()) {
        Some(explicit) => explicit
            .from_local_datetime(&naive)
            .earliest()
            .map(|d| d.with_timezone(&Utc)),
        // Floating time (no TZID, no Z): RFC 5545 says interpret it in the viewer's
        // zone — use the user's chosen IANA zone (not the machine's), so the event
        // lands on the same instant no matter which machine syncs the feed.
        None => tz
            .from_local_datetime(&naive)
            .earliest()
            .map(|d| d.with_timezone(&Utc)),
    }
}

/// The value of property `name`, ignoring its parameters.
fn find<'a>(block: &'a [String], name: &str) -> Option<&'a str> {
    find_prop(block, name).map(|(_, v)| v)
}

/// The (parameters, value) of property `name`: `NAME;p=v:value` → `("p=v", "value")`.
fn find_prop<'a>(block: &'a [String], name: &str) -> Option<(&'a str, &'a str)> {
    block.iter().find_map(|line| {
        let colon = line.find(':')?;
        let (head, value) = (&line[..colon], &line[colon + 1..]);
        let (prop, params) = match head.find(';') {
            Some(i) => (&head[..i], &head[i + 1..]),
            None => (head, ""),
        };
        (prop == name).then_some((params, value))
    })
}

/// A named parameter from a property's parameter string (`TZID=Europe/London`).
fn param<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    params.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.eq_ignore_ascii_case(key).then_some(v)
    })
}

/// Unescape ICS text (`\n`, `\,`, `\;`, `\\`).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        )
    }

    #[test]
    fn parses_a_timed_event_and_skips_cancelled() {
        let feed = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Standup\r\n\
                    DTSTART:20260615T090000Z\r\nDTEND:20260615T093000Z\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:b\r\nSTATUS:CANCELLED\r\nSUMMARY:Off\r\n\
                    DTSTART:20260616T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let (s, e) = window();
        let events = parse_feed_within(feed, "feed1", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Standup");
        assert_eq!(events[0].start, "2026-06-15T09:00:00Z");
        assert_eq!(events[0].end.as_deref(), Some("2026-06-15T09:30:00Z"));
        assert!(!events[0].all_day);
        assert_eq!(events[0].calendar_id, "feed1");
    }

    #[test]
    fn parses_all_day_and_unfolds_long_summaries() {
        let feed = "BEGIN:VEVENT\nUID:c\nSUMMARY:Quarterly planning\n offsite day\n\
                    DTSTART;VALUE=DATE:20260620\nDTEND;VALUE=DATE:20260621\nEND:VEVENT";
        let (s, e) = window();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 1);
        assert!(events[0].all_day);
        assert_eq!(events[0].start, "2026-06-20");
        // The folded continuation line joins onto the summary.
        assert_eq!(events[0].summary, "Quarterly planningoffsite day");
    }

    #[test]
    fn resolves_tzid_to_utc() {
        // 09:00 in Europe/London (BST, +01:00) is 08:00 UTC.
        let feed = "BEGIN:VEVENT\nUID:d\nSUMMARY:Call\n\
                    DTSTART;TZID=Europe/London:20260615T090000\nEND:VEVENT";
        let (s, e) = window();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert_eq!(events[0].start, "2026-06-15T08:00:00Z");
    }

    #[test]
    fn expands_a_weekly_recurrence_within_the_window() {
        // Weekly on Mondays from 2026-06-01; the window spans June (5 Mondays).
        let feed = "BEGIN:VEVENT\nUID:e\nSUMMARY:Weekly sync\n\
                    DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        // Mondays in [Jun 1, Jun 30): 1, 8, 15, 22, 29.
        assert_eq!(events.len(), 5);
        assert!(events.iter().all(|ev| ev.summary == "Weekly sync"));
        assert_eq!(events[0].start, "2026-06-01T10:00:00Z");
    }

    #[test]
    fn sub_daily_recurrence_far_in_the_past_is_skipped_without_hanging() {
        // FREQ=SECONDLY from 26 years before the window, unbounded: expanding it
        // naively walks ~8e8 occurrences (a sync hang). It must be skipped — and
        // this test completing at all is the regression guard (F6).
        let feed = "BEGIN:VEVENT\nUID:bomb\nSUMMARY:tick\n\
                    DTSTART:20000101T000000Z\nRRULE:FREQ=SECONDLY\nEND:VEVENT";
        let (s, e) = window();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert!(events.is_empty(), "sub-daily recurrence should be skipped");
    }

    #[test]
    fn floating_time_resolves_to_user_zone_not_machine() {
        // A floating DTSTART (no Z, no TZID) is the viewer's zone — the user's chosen
        // IANA zone, NOT the machine's. 09:00 floating in Asia/Tokyo (+09) = 00:00 UTC.
        // Guard against regressing to UTC or the machine's local zone.
        use chrono_tz::Asia::Tokyo;
        let got = parse_any("20260615T090000", None, false, Tokyo).unwrap();
        assert_eq!(got, Utc.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap());
    }

    #[test]
    fn all_day_keeps_its_calendar_date_across_zones() {
        // An all-day VALUE=DATE event is a civil date with no instant; it must read as
        // the same calendar day in every zone. `make_event` formats the user-zone
        // midnight anchor back in that zone, so an east-of-UTC zone doesn't drift it a
        // day (without that fix, Tokyo would render 2026-06-19).
        let feed = "BEGIN:VEVENT\nUID:c\nSUMMARY:Offsite\n\
                    DTSTART;VALUE=DATE:20260620\nDTEND;VALUE=DATE:20260621\nEND:VEVENT";
        let (s, e) = window();
        let east = parse_feed_within(feed, "f", s, e, chrono_tz::Asia::Tokyo);
        let west = parse_feed_within(feed, "f", s, e, chrono_tz::America::Los_Angeles);
        assert_eq!(east[0].start, "2026-06-20");
        assert_eq!(west[0].start, "2026-06-20");
        assert_eq!(east[0].end.as_deref(), Some("2026-06-21"));
    }

    #[test]
    fn all_day_survives_a_midnight_dst_gap() {
        // America/Havana springs forward at 00:00 on 2026-03-08, so local midnight
        // that day does not exist. An all-day event must still parse (anchored at
        // noon, never inside the gap) instead of silently vanishing.
        let feed = "BEGIN:VEVENT\nUID:h\nSUMMARY:Holiday\nDTSTART;VALUE=DATE:20260308\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 3, 31, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, chrono_tz::America::Havana);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, "2026-03-08");
    }
}
