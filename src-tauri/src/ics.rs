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

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz as ChronoTz;

use crate::calendar::CalendarEvent;

/// Parse an ICS feed into events from yesterday through `window_days` ahead.
pub fn parse_feed(text: &str, feed_id: &str, window_days: i64) -> Vec<CalendarEvent> {
    let now = Utc::now();
    parse_feed_within(text, feed_id, now - Duration::days(1), now + Duration::days(window_days))
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
) -> Vec<CalendarEvent> {
    let lines = unfold(text);
    let mut out = Vec::new();
    for block in vevents(&lines).into_iter().take(MAX_VEVENTS) {
        if out.len() >= MAX_EVENTS {
            break;
        }
        out.extend(expand_vevent(&block, feed_id, win_start, win_end));
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
    let location = find(block, "LOCATION").map(unescape).filter(|s| !s.is_empty());
    let description = find(block, "DESCRIPTION").map(unescape).filter(|s| !s.is_empty());
    let uid = find(block, "UID").unwrap_or("").to_string();

    let Some(start_anchor) = parse_any(start_val, param(start_params, "TZID"), all_day) else {
        return Vec::new();
    };
    let end_anchor = find_prop(block, "DTEND")
        .and_then(|(p, v)| parse_any(v, param(p, "TZID"), all_day));
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
        .map(|s| make_event(feed_id, &uid, &summary, &location, &description, s, dur, all_day))
        .collect()
}

/// Expand an `RRULE` to its UTC occurrence-starts within the window. Feeds the
/// original DTSTART/RRULE/EXDATE/RDATE lines straight to `rrule` so it resolves the
/// timezone itself; a parse failure degrades to "no occurrences" rather than erroring.
fn expand_rrule(block: &[String], win_start: DateTime<Utc>, win_end: DateTime<Utc>) -> Vec<DateTime<Utc>> {
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
            && ["FREQ=SECONDLY", "FREQ=MINUTELY", "FREQ=HOURLY"].iter().any(|f| u.contains(f))
    })
}

fn make_event(
    feed_id: &str,
    uid: &str,
    summary: &str,
    location: &Option<String>,
    description: &Option<String>,
    start_utc: DateTime<Utc>,
    dur: Option<Duration>,
    all_day: bool,
) -> CalendarEvent {
    let (start, end) = if all_day {
        let s = start_utc.format("%Y-%m-%d").to_string();
        let e = dur.map(|d| (start_utc + d).format("%Y-%m-%d").to_string());
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
    }
}

/// Resolve an ICS date/datetime value to a UTC instant. All-day values are anchored
/// to UTC midnight (formatted back to a date later).
fn parse_any(value: &str, tzid: Option<&str>, all_day: bool) -> Option<DateTime<Utc>> {
    let v = value.trim();
    if all_day {
        let date = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
    }
    if let Some(stripped) = v.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        return Some(Utc.from_utc_datetime(&naive));
    }
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    match tzid.and_then(|t| t.parse::<ChronoTz>().ok()) {
        Some(tz) => tz.from_local_datetime(&naive).earliest().map(|d| d.with_timezone(&Utc)),
        // Floating time (no TZID, no Z): RFC 5545 says interpret it in the viewer's
        // local zone, so resolve against the system timezone — assuming UTC would
        // shift the event by the user's offset (Outlook and exported feeds emit these).
        None => Local.from_local_datetime(&naive).earliest().map(|d| d.with_timezone(&Utc)),
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
        let events = parse_feed_within(feed, "feed1", s, e);
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
        let events = parse_feed_within(feed, "f", s, e);
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
        let events = parse_feed_within(feed, "f", s, e);
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
        let events = parse_feed_within(feed, "f", s, e);
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
        let events = parse_feed_within(feed, "f", s, e);
        assert!(events.is_empty(), "sub-daily recurrence should be skipped");
    }

    #[test]
    fn floating_time_resolves_to_local_not_utc() {
        // A floating DTSTART (no Z, no TZID) is the viewer's local time, not UTC
        // (F5). Guard against regressing to a hard-coded UTC interpretation.
        let naive = NaiveDateTime::parse_from_str("20260615T090000", "%Y%m%dT%H%M%S").unwrap();
        let got = parse_any("20260615T090000", None, false).unwrap();
        let expected = Local.from_local_datetime(&naive).earliest().unwrap().with_timezone(&Utc);
        assert_eq!(got, expected);
    }
}
