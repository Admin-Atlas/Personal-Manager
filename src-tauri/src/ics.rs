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

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz as ChronoTz;

use crate::calendar::{Attendee, CalendarEvent};

/// Defensive caps for a hostile or oversized feed: bound how many VEVENT blocks we
/// parse and how many expanded occurrences we keep. The 10 MiB body cap (see
/// `calendar::read_capped`) already bounds the input; these bound the work and the
/// memory the parse produces. Far above any real personal calendar.
const MAX_VEVENTS: usize = 50_000;
const MAX_EVENTS: usize = 100_000;

/// The window-pure core keeping only the events — a thin wrapper over
/// [`parse_feed_within_reporting`], exercised by the unit tests (recurrence expansion is testable
/// without `now`). Production code goes through the reporting form: anything that WRITES the mirror
/// needs the completeness verdict, because a silently clamped parse that reaps rows breaks I-09.3.
#[cfg(test)]
fn parse_feed_within(
    text: &str,
    feed_id: &str,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
) -> Vec<CalendarEvent> {
    parse_feed_within_reporting(text, feed_id, win_start, win_end, tz).0
}

/// [`parse_feed_within`], also reporting whether the parse saw the WHOLE feed. `false` means events
/// were dropped without anything failing: the body was cut mid-`VEVENT` (a truncated download still
/// reads as a clean 200), there were more blocks than [`MAX_VEVENTS`], or the expansion hit
/// [`MAX_EVENTS`]. The mirror write then withholds its delete half — "we never saw it" is not "the
/// user deleted it" (I-09.3).
pub fn parse_feed_within_reporting(
    text: &str,
    feed_id: &str,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
) -> (Vec<CalendarEvent>, bool) {
    parse_within_capped(
        text,
        feed_id,
        win_start,
        win_end,
        tz,
        MAX_VEVENTS,
        MAX_EVENTS,
    )
}

/// [`parse_feed_within_reporting`] with the defensive caps injected, so both clamp paths are
/// exercisable without generating a 100k-event feed.
fn parse_within_capped(
    text: &str,
    feed_id: &str,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
    max_vevents: usize,
    max_events: usize,
) -> (Vec<CalendarEvent>, bool) {
    let lines = unfold(text);
    let (parsed, terminated) = vevents(&lines);
    let blocks_seen = parsed.len();
    let blocks: Vec<Vec<String>> = parsed.into_iter().take(max_vevents).collect();

    // A VEVENT carrying RECURRENCE-ID is a per-instance override (moved or cancelled)
    // of the series with the same UID (RFC 5545 §3.8.4.4). Collect those overridden
    // instants keyed by UID so the master's RRULE expansion skips them — otherwise a
    // cancelled instance still shows and a moved one renders twice (its new slot plus
    // the master's original slot). The override VEVENT itself still renders normally
    // (or, if STATUS:CANCELLED, drops out in `expand_vevent`).
    let mut overrides: HashMap<String, HashSet<i64>> = HashMap::new();
    for block in &blocks {
        let Some((params, value)) = find_prop(block, "RECURRENCE-ID") else {
            continue;
        };
        let uid = find(block, "UID").unwrap_or("").to_string();
        let all_day = param(params, "VALUE") == Some("DATE")
            || (!value.contains('T') && value.trim().len() == 8);
        if let Some(inst) = parse_any(value, param(params, "TZID"), all_day, tz) {
            overrides.entry(uid).or_default().insert(inst.timestamp());
        }
    }

    let mut out = Vec::new();
    for block in &blocks {
        if out.len() >= max_events {
            break;
        }
        out.extend(expand_vevent(
            block, feed_id, win_start, win_end, tz, &overrides,
        ));
    }
    out.truncate(max_events);
    // A feed landing exactly ON the occurrence cap counts as clamped: the loop can't tell "the last
    // block fit" from "the next block was never tried". Over-reporting incompleteness only costs a
    // withheld delete for one sync; under-reporting reaps rows that are still live upstream.
    let complete = terminated && blocks_seen <= max_vevents && out.len() < max_events;
    (out, complete)
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

/// Collect the property lines of each `VEVENT` block, and report whether every block was
/// TERMINATED. A block still open at the end of the body is dropped by the `cur.take()` below, so
/// an unterminated tail is the one signal that a cut-off feed leaves behind — the events it lost
/// can't even be counted.
fn vevents(lines: &[String]) -> (Vec<Vec<String>>, bool) {
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
    (blocks, cur.is_none())
}

/// Turn one VEVENT into its in-window occurrences (one for a single event, many for a
/// recurring one).
fn expand_vevent(
    block: &[String],
    feed_id: &str,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
    overrides: &HashMap<String, HashSet<i64>>,
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
    // Duration comes from DTEND if present, else the mutually-exclusive DURATION
    // property (RFC 5545 §3.8.2 — common in CalDAV/recurring exports). Without the
    // DURATION fallback such events would render as zero-length points and, worse, a
    // long DURATION-only event predating the window would be wrongly excluded below.
    let end_anchor =
        find_prop(block, "DTEND").and_then(|(p, v)| parse_any(v, param(p, "TZID"), all_day, tz));
    let dur = end_anchor
        .map(|e| e - start_anchor)
        .or_else(|| find(block, "DURATION").and_then(parse_duration));
    let effective_end = end_anchor
        .or_else(|| dur.and_then(|d| start_anchor.checked_add_signed(d)))
        .unwrap_or(start_anchor);

    let mut starts: Vec<DateTime<Utc>> = if block.iter().any(|l| is_prop(l, "RRULE")) {
        expand_rrule(block, win_start, win_end, tz, all_day)
    } else if start_anchor <= win_end && effective_end >= win_start {
        vec![start_anchor]
    } else {
        Vec::new()
    };

    // Drop occurrences the series' own RECURRENCE-ID overrides have moved/cancelled — but only
    // from the series MASTER. A block that itself carries RECURRENCE-ID *is* an override: it
    // defines exactly one instance and its own RECURRENCE-ID is in the skip set, so applying the
    // skip here deleted every override that did NOT move in time — the ordinary "change just this
    // one" edit (new title, room or end time). The master's occurrence at that instant is still
    // removed below, so the override replaces it rather than doubling it.
    if let Some(skip) = overrides
        .get(&uid)
        .filter(|_| find_prop(block, "RECURRENCE-ID").is_none())
    {
        starts.retain(|s| !skip.contains(&s.timestamp()));
    }

    // v40 detail fields for the event popup — parsed once from the VEVENT block (constant across a
    // recurring series' occurrences), then stamped on each event below.
    let show_as = Some(
        if find(block, "TRANSP") == Some("TRANSPARENT") {
            "free"
        } else {
            "busy"
        }
        .to_string(),
    );
    let organizer = find_prop(block, "ORGANIZER").map(|(params, val)| {
        param(params, "CN").map(unescape).unwrap_or_else(|| {
            val.trim_start_matches("mailto:")
                .trim_start_matches("MAILTO:")
                .to_string()
        })
    });
    let attendees = ics_attendees(block);
    let recurring = block.iter().any(|l| is_prop(l, "RRULE"));
    let recurrence_summary = find(block, "RRULE").map(str::to_string);
    let status = find(block, "STATUS").map(str::to_string);
    let visibility = find(block, "CLASS").map(str::to_string);
    let created = find(block, "CREATED").map(str::to_string);
    let updated = find(block, "LAST-MODIFIED")
        .or_else(|| find(block, "DTSTAMP"))
        .map(str::to_string);

    starts
        .into_iter()
        .map(|s| {
            let mut ev = make_event(
                feed_id,
                &uid,
                &summary,
                &location,
                &description,
                s,
                dur,
                all_day,
                tz,
            );
            ev.show_as = show_as.clone();
            ev.organizer = organizer.clone();
            ev.attendees = attendees.clone();
            ev.recurring = recurring;
            ev.recurrence_summary = recurrence_summary.clone();
            ev.status = status.clone();
            ev.visibility = visibility.clone();
            ev.created = created.clone();
            ev.updated = updated.clone();
            ev
        })
        .collect()
}

/// Expand an `RRULE` to its UTC occurrence-starts within the window. Feeds the
/// DTSTART/RRULE/EXDATE/RDATE lines to `rrule` so it resolves the timezone and DST
/// itself; a floating DTSTART is first pinned to the user's zone so it doesn't fall
/// back to the machine's. A parse failure retries with the DTSTART pinned to its
/// resolved UTC instant (so a DST-ambiguous DTSTART doesn't drop the whole series),
/// and only then degrades to "no occurrences".
///
/// An `all_day` series is re-anchored on the way out, so its occurrences land in the same
/// value space as every other all-day start (see the mapping at the tail).
fn expand_rrule(
    block: &[String],
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
    tz: ChronoTz,
    all_day: bool,
) -> Vec<DateTime<Utc>> {
    // Only expand a day-or-coarser FREQ. Sub-daily frequencies (SECONDLY/MINUTELY/
    // HOURLY) — or an unrecognisable rule — force the iterator to walk a huge number
    // of pre-window occurrences before reaching the agenda window (a CPU hang on sync
    // from one crafted feed line; `.all(..)` caps results, not the walk), and are
    // meaningless in a day-level agenda. Allowlist, not denylist, so an unknown or
    // future sub-daily pattern is refused rather than risked.
    if !rrule_freq_is_expandable(block) {
        return Vec::new();
    }

    let spec = build_rrule_spec(block, tz);
    let set = match spec.parse::<rrule::RRuleSet>() {
        Ok(set) => set,
        Err(_) => match rrule_spec_utc_fallback(block, tz).and_then(|s| s.parse().ok()) {
            Some(set) => set,
            None => return Vec::new(),
        },
    };

    let after = win_start.with_timezone(&rrule::Tz::UTC);
    let before = win_end.with_timezone(&rrule::Tz::UTC);
    // Cap returned occurrences by the window span with generous headroom for a daily
    // (or multi-time daily) series, bounded well under MAX_EVENTS. The old fixed 366
    // silently dropped ~2 months of any daily series in the −1..+13-month mirror.
    let window_days = (win_end - win_start).num_days().max(1);
    let limit = window_days.saturating_mul(24).clamp(366, u16::MAX as i64) as u16;
    set.after(after)
        .before(before)
        .all(limit)
        .dates
        .into_iter()
        // An all-day occurrence comes back as a midnight in whatever zone `rrule` resolved the
        // `VALUE=DATE` DTSTART in — the machine's local zone, since a DATE carries no TZID — so
        // its UTC instant is NOT the noon-in-`tz` anchor `parse_any` builds for the same civil
        // date, and the two can even name different days. Read the civil date out of `rrule`'s
        // own frame (never out of the UTC instant, which is a day out east or west of that zone)
        // and re-anchor it: the date then renders identically in every zone, and a
        // RECURRENCE-ID's instant compares equal to the master occurrence it replaces.
        .filter_map(|d| {
            if all_day {
                all_day_anchor(d.date_naive(), tz)
            } else {
                Some(d.with_timezone(&Utc))
            }
        })
        .collect()
}

/// True only if the block's `RRULE` uses a day-or-coarser FREQ (DAILY/WEEKLY/MONTHLY/
/// YEARLY). See `expand_rrule` for why this is an allowlist.
fn rrule_freq_is_expandable(block: &[String]) -> bool {
    block.iter().any(|l| {
        let u = l.to_ascii_uppercase();
        u.starts_with("RRULE")
            && ["FREQ=DAILY", "FREQ=WEEKLY", "FREQ=MONTHLY", "FREQ=YEARLY"]
                .iter()
                .any(|f| u.contains(f))
    })
}

/// The DTSTART/RRULE/EXDATE/RDATE lines joined for `rrule`, pinning a *floating*
/// DTSTART (timed, no `Z`, no `TZID`) to the user's zone so `rrule` resolves it like
/// `parse_any` does — not in the machine's local zone.
fn build_rrule_spec(block: &[String], tz: ChronoTz) -> String {
    block
        .iter()
        .map(|l| canonical_prop_line(l))
        .filter(|l| {
            is_prop(l, "DTSTART")
                || is_prop(l, "RRULE")
                || is_prop(l, "EXDATE")
                || is_prop(l, "RDATE")
        })
        .map(|l| pin_floating_dtstart(&l, tz))
        .collect::<Vec<_>>()
        .join("\n")
}

/// If `line` is a floating (timed, no `Z`, no `TZID`) DTSTART, add `;TZID=<user zone>`;
/// otherwise return it unchanged.
fn pin_floating_dtstart(line: &str, tz: ChronoTz) -> String {
    if !is_prop(line, "DTSTART") {
        return line.to_string();
    }
    let Some(colon) = line.find(':') else {
        return line.to_string();
    };
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let is_timed = value.contains('T');
    let has_z = value.trim_end().ends_with('Z');
    let has_tzid = head.to_ascii_uppercase().contains("TZID=");
    if is_timed && !has_z && !has_tzid {
        format!("{head};TZID={}:{value}", tz.name())
    } else {
        line.to_string()
    }
}

/// A fallback spec whose DTSTART is pinned to its resolved UTC instant, for the rare
/// DTSTART that lands in a DST gap and makes the primary parse fail. Loses cross-DST
/// wall-clock stability but keeps the series instead of dropping it entirely.
fn rrule_spec_utc_fallback(block: &[String], tz: ChronoTz) -> Option<String> {
    let (params, value) = find_prop(block, "DTSTART")?;
    let all_day =
        param(params, "VALUE") == Some("DATE") || (!value.contains('T') && value.trim().len() == 8);
    let anchor = parse_any(value, param(params, "TZID"), all_day, tz)?;
    let dtstart = if all_day {
        format!(
            "DTSTART;VALUE=DATE:{}",
            anchor.with_timezone(&tz).format("%Y%m%d")
        )
    } else {
        format!("DTSTART:{}", anchor.format("%Y%m%dT%H%M%SZ"))
    };
    let rest = block
        .iter()
        .map(|l| canonical_prop_line(l))
        .filter(|l| is_prop(l, "RRULE") || is_prop(l, "EXDATE") || is_prop(l, "RDATE"));
    Some(
        std::iter::once(dtstart)
            .chain(rest)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Parse an RFC 5545 DURATION value (`P1DT2H`, `-PT30M`, `P2W`) to a `chrono::Duration`.
///
/// Every unit is folded in with checked arithmetic and the total is bounded, so an extreme or
/// malformed value is treated as malformed (`None`) rather than pushing the accumulation — or the
/// later instant math — out of range.
fn parse_duration(value: &str) -> Option<Duration> {
    // No real calendar event runs a century; a larger (or overflowing) value is treated as malformed.
    const MAX_DURATION_SECS: i64 = 100 * 366 * 86_400;

    let v = value.trim();
    let (sign, rest) = match v.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut secs: i64 = 0;
    let mut num = String::new();
    let mut in_time = false;
    let mut saw_any = false;
    // Fold `<num><unit>` into `secs` without ever overflowing i64.
    let acc = |secs: i64, num: &str, unit_secs: i64| -> Option<i64> {
        let n = num.parse::<i64>().ok()?;
        secs.checked_add(n.checked_mul(unit_secs)?)
    };
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            'W' => {
                secs = acc(secs, &num, 7 * 86_400)?;
                num.clear();
                saw_any = true;
            }
            'D' => {
                secs = acc(secs, &num, 86_400)?;
                num.clear();
                saw_any = true;
            }
            'H' if in_time => {
                secs = acc(secs, &num, 3_600)?;
                num.clear();
                saw_any = true;
            }
            'M' if in_time => {
                secs = acc(secs, &num, 60)?;
                num.clear();
                saw_any = true;
            }
            'S' if in_time => {
                secs = acc(secs, &num, 1)?;
                num.clear();
                saw_any = true;
            }
            _ => return None,
        }
    }
    // Trailing digits with no unit, or a bare `P`, are malformed.
    if !num.is_empty() || !saw_any {
        return None;
    }
    if secs > MAX_DURATION_SECS {
        return None;
    }
    Duration::try_seconds(sign * secs)
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
        let e = dur.and_then(|d| {
            start_utc
                .checked_add_signed(d)
                .map(|end| end.with_timezone(&tz).format("%Y-%m-%d").to_string())
        });
        (s, e)
    } else {
        let s = start_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let e = dur.and_then(|d| {
            start_utc
                .checked_add_signed(d)
                .map(|end| end.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        });
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
        // The v40 detail fields (show_as / organizer / recurrence / …) are the same for every
        // occurrence of a recurring VEVENT, so `expand_vevent` computes them once and sets them on
        // each returned event rather than threading them through this per-occurrence builder.
        ..Default::default()
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
        return all_day_anchor(NaiveDate::parse_from_str(v, "%Y%m%d").ok()?, tz);
    }
    if let Some(stripped) = v.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        return Some(Utc.from_utc_datetime(&naive));
    }
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    match tzid.and_then(|t| t.parse::<ChronoTz>().ok()) {
        Some(explicit) => resolve_local(explicit, naive),
        // Floating time (no TZID, no Z): RFC 5545 says interpret it in the viewer's
        // zone — use the user's chosen IANA zone (not the machine's), so the event
        // lands on the same instant no matter which machine syncs the feed.
        None => resolve_local(tz, naive),
    }
}

/// The instant an all-day civil date anchors to: NOON in the user's zone, not midnight. A zone
/// whose DST spring-forward lands exactly at 00:00 (e.g. America/Havana, Africa/Cairo) has no
/// local midnight that day, so a midnight anchor is a `None` gap and the event silently vanishes.
/// Noon is never inside a 1-hour gap; `make_event` formats date-only, so the offset is truncated
/// and the stored civil date is unchanged in every zone.
///
/// Every all-day start goes through here — a lone `VALUE=DATE`, an expanded RRULE occurrence and a
/// `RECURRENCE-ID` alike — so they share one value space and an override's instant compares equal
/// to the master occurrence it replaces.
fn all_day_anchor(date: NaiveDate, tz: ChronoTz) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&date.and_hms_opt(12, 0, 0)?)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
}

/// Resolve a naive local datetime in `zone` to a UTC instant, tolerating a DST
/// spring-forward gap: a local time inside the skipped hour has no instant
/// (`LocalResult::None`), so nudge forward past the gap rather than dropping the event
/// (a 1-hour shift covers essentially every zone). A fall-back (ambiguous) time takes
/// the earliest of the two candidates.
fn resolve_local(zone: ChronoTz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    if let Some(dt) = zone.from_local_datetime(&naive).earliest() {
        return Some(dt.with_timezone(&Utc));
    }
    zone.from_local_datetime(&(naive + Duration::hours(1)))
        .earliest()
        .map(|d| d.with_timezone(&Utc))
}

/// The value of property `name`, ignoring its parameters.
fn find<'a>(block: &'a [String], name: &str) -> Option<&'a str> {
    find_prop(block, name).map(|(_, v)| v)
}

/// Every (parameters, value) pair for property `name`, in feed order.
///
/// [`find_prop`] stops at the first match, which is right for the single-valued properties but wrong
/// for `ATTENDEE`: RFC 5545 allows one line per attendee, so taking only the first would report a
/// twelve-person meeting as having one guest.
fn find_props<'a>(block: &'a [String], name: &str) -> Vec<(&'a str, &'a str)> {
    block
        .iter()
        .filter_map(|line| {
            let colon = line.find(':')?;
            let (head, value) = (&line[..colon], &line[colon + 1..]);
            let (prop, params) = match head.find(';') {
                Some(i) => (&head[..i], &head[i + 1..]),
                None => (head, ""),
            };
            prop.eq_ignore_ascii_case(name).then_some((params, value))
        })
        .collect()
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
        // RFC 5545 property names are case-INSENSITIVE (3.1: "property names ... are case
        // insensitive"). Matching them exactly meant a feed emitting `dtstart:` had no DTSTART at
        // all as far as this parser was concerned, so every one of its events was silently
        // skipped — the feed simply appeared empty. `param` already compares this way.
        prop.eq_ignore_ascii_case(name).then_some((params, value))
    })
}

/// Does `line` carry the named property? Case-insensitive per RFC 5545, and the name must end at a
/// `;` (parameters) or `:` (value), so `RDATE` can never match an unrelated `RDATEX`.
fn is_prop(line: &str, name: &str) -> bool {
    match line.as_bytes().get(name.len()) {
        Some(b';') | Some(b':') => line
            .get(..name.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(name)),
        _ => false,
    }
}

/// `line` with its property NAME upper-cased; parameters and value untouched (a `TZID=Europe/London`
/// value IS case-sensitive, so only the name may be folded).
///
/// Load-bearing, not cosmetic: the `rrule` crate parses the spec string we hand it and does NOT
/// share RFC 5545's case-insensitivity. Finding a lowercase feed's `rrule:` without canonicalising
/// it would only trade a silently-dropped event for a rejected spec — so recognition and
/// normalisation have to travel together.
fn canonical_prop_line(line: &str) -> String {
    match line.find([';', ':']) {
        Some(i) => format!("{}{}", line[..i].to_ascii_uppercase(), &line[i..]),
        None => line.to_string(),
    }
}

/// A named parameter from a property's parameter string (`TZID=Europe/London`).
fn param<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    params.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.eq_ignore_ascii_case(key).then_some(v)
    })
}

/// ICS `ATTENDEE` lines → the shared [`Attendee`] shape (empty when the VEVENT lists none).
///
/// One line per attendee, e.g.
/// `ATTENDEE;CN=Ada Lovelace;ROLE=OPT-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:ada@example.com`.
///
/// Every field is optional because feeds vary wildly in what they emit — a bare
/// `ATTENDEE:mailto:x@y` is valid and common, so an entry with only an email still round-trips.
/// `PARTSTAT` is normalised to the same vocabulary Google and Graph are mapped onto, so the popup
/// renders one set of terms whatever the source.
fn ics_attendees(block: &[String]) -> Vec<Attendee> {
    find_props(block, "ATTENDEE")
        .into_iter()
        .map(|(params, value)| {
            let email = value
                .trim()
                .strip_prefix("mailto:")
                .or_else(|| value.trim().strip_prefix("MAILTO:"))
                .unwrap_or(value.trim());
            Attendee {
                name: param(params, "CN").map(unescape).filter(|s| !s.is_empty()),
                email: (!email.is_empty()).then(|| email.to_string()),
                response: param(params, "PARTSTAT").map(partstat),
                // RFC 5545 §3.2.16: OPT-PARTICIPANT is the optional role; everything else is
                // required or non-participating, neither of which is "optional" in the UI's sense.
                optional: param(params, "ROLE")
                    .is_some_and(|r| r.eq_ignore_ascii_case("OPT-PARTICIPANT")),
                // An ICS feed carries no notion of "this is the connected account", and CHAIR is a
                // role rather than a claim of organisership — ORGANIZER is its own property and is
                // parsed separately. Leaving both false is honest; guessing would mislabel guests.
                organizer: false,
                is_self: false,
            }
        })
        .collect()
}

/// ICS `PARTSTAT` → the response vocabulary Google/Graph are normalised onto, so one set of terms
/// reaches the UI. An unrecognised value is passed through lowercased rather than dropped: a feed
/// using an `X-` extension still says something, and inventing "needsAction" for it would not.
fn partstat(v: &str) -> String {
    match v.to_ascii_uppercase().as_str() {
        "ACCEPTED" => "accepted".to_string(),
        "DECLINED" => "declined".to_string(),
        "TENTATIVE" => "tentative".to_string(),
        "NEEDS-ACTION" => "needsAction".to_string(),
        other => other.to_ascii_lowercase(),
    }
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
    fn property_names_are_case_insensitive() {
        // RFC 5545 §3.1: property names are case-insensitive. Matching them exactly meant a feed
        // emitting lowercase names had no DTSTART as far as this parser was concerned, so every
        // event was skipped and the feed simply looked EMPTY — no error, no clue.
        let feed = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nuid:a\r\nsummary:Standup\r\n\
                    dtstart:20260615T090000Z\r\ndtend:20260615T093000Z\r\nEND:VEVENT\r\n\
                    END:VCALENDAR";
        let (s, e) = window();
        let events = parse_feed_within(feed, "feed1", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 1, "a lowercase feed must not read as empty");
        assert_eq!(events[0].summary, "Standup");
        assert_eq!(events[0].start, "2026-06-15T09:00:00Z");
        assert_eq!(events[0].end.as_deref(), Some("2026-06-15T09:30:00Z"));
    }

    #[test]
    fn a_lowercase_rrule_still_expands() {
        // Recognising a lowercase `rrule:` is only half of it: the `rrule` crate parses the spec we
        // hand it and does NOT share RFC 5545's case-insensitivity, so finding the property without
        // canonicalising its name would just trade a dropped event for a rejected spec — one
        // instance instead of the series, which is the quieter of the two failures.
        let feed = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nuid:r\r\nsummary:Weekly\r\n\
                    dtstart:20260601T090000Z\r\nrrule:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\n\
                    END:VCALENDAR";
        let (s, e) = window();
        let events = parse_feed_within(feed, "feed1", s, e, ChronoTz::UTC);
        assert_eq!(
            events.len(),
            3,
            "the series must expand, not collapse to one"
        );
        assert_eq!(events[0].start, "2026-06-01T09:00:00Z");
        assert_eq!(events[2].start, "2026-06-15T09:00:00Z");
    }

    #[test]
    fn a_property_name_is_matched_whole() {
        // `is_prop` must not treat a longer name as its prefix, or an unrelated X-DTSTARTISH
        // property could be parsed as the event's start.
        assert!(is_prop("DTSTART:20260601", "DTSTART"));
        assert!(is_prop(
            "dtstart;TZID=Europe/London:20260601T090000",
            "DTSTART"
        ));
        assert!(!is_prop("DTSTARTX:20260601", "DTSTART"));
        assert!(!is_prop("DTSTART", "DTSTART"), "a bare name has no value");
        // The name folds; a TZID VALUE must not (Europe/London is case-sensitive).
        assert_eq!(
            canonical_prop_line("dtstart;TZID=Europe/London:20260601T090000"),
            "DTSTART;TZID=Europe/London:20260601T090000"
        );
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
    fn an_overflowing_duration_is_dropped_not_panicked() {
        // A DURATION whose magnitude overflows the unit accumulation (here i64::MAX days) must be
        // treated as malformed, never crash the whole calendar sync. The event still parses; with no
        // usable duration it renders as a zero-length point (no end).
        let feed = "BEGIN:VEVENT\nUID:e\nSUMMARY:Evil\n\
                    DTSTART:20260615T090000Z\nDURATION:P9223372036854775807D\nEND:VEVENT";
        let (s, e) = window();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, "2026-06-15T09:00:00Z");
        assert_eq!(events[0].end, None);
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

    #[test]
    fn daily_recurrence_is_not_truncated_across_a_14_month_mirror() {
        // Regression for the `.all(366)` cap: a daily series over a −1..+13-month
        // window (~425 days) must yield every in-window day, not stop at 366.
        let feed = "BEGIN:VEVENT\nUID:daily\nSUMMARY:Standup\n\
                    DTSTART:20260101T090000Z\nDTEND:20260101T091500Z\n\
                    RRULE:FREQ=DAILY\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2027, 2, 1, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        // Days in [2026-01-01, 2027-02-01): 365 (2026) + 31 (Jan 2027) = 396.
        assert_eq!(events.len(), 396);
        assert!(events.len() > 366, "must exceed the old fixed cap");
    }

    #[test]
    fn floating_recurrence_resolves_in_user_zone_not_machine() {
        // A floating (no Z, no TZID) recurring DTSTART must expand in the user's zone,
        // matching the non-recurring floating path. 09:00 floating in Asia/Tokyo (+09)
        // is 00:00 UTC, so each weekly occurrence is a UTC midnight.
        let feed = "BEGIN:VEVENT\nUID:fl\nSUMMARY:Sync\n\
                    DTSTART:20260601T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, chrono_tz::Asia::Tokyo);
        assert!(!events.is_empty());
        assert_eq!(events[0].start, "2026-06-01T00:00:00Z");
    }

    #[test]
    fn recurrence_id_cancellation_removes_only_that_instance() {
        // A weekly series with one instance cancelled via a RECURRENCE-ID override:
        // that Monday drops out, the rest survive.
        let feed = "BEGIN:VEVENT\nUID:s1\nSUMMARY:Weekly\n\
                    DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:s1\nRECURRENCE-ID:20260608T100000Z\n\
                    STATUS:CANCELLED\nDTSTART:20260608T100000Z\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        // Mondays Jun 1,8,15,22,29 minus the cancelled Jun 8 → 4.
        assert_eq!(events.len(), 4);
        assert!(!events.iter().any(|ev| ev.start == "2026-06-08T10:00:00Z"));
    }

    #[test]
    fn recurrence_id_reschedule_is_not_duplicated() {
        // A moved instance (override DTSTART differs from RECURRENCE-ID) renders once
        // at its new time, not twice.
        let feed = "BEGIN:VEVENT\nUID:s2\nSUMMARY:Weekly\n\
                    DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:s2\nRECURRENCE-ID:20260608T100000Z\n\
                    SUMMARY:Weekly (moved)\nDTSTART:20260608T140000Z\n\
                    DTEND:20260608T143000Z\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        // 5 Mondays, but Jun 8 10:00 replaced by Jun 8 14:00 → still 5 total.
        assert_eq!(events.len(), 5);
        assert!(!events.iter().any(|ev| ev.start == "2026-06-08T10:00:00Z"));
        assert!(events.iter().any(|ev| ev.start == "2026-06-08T14:00:00Z"));
    }

    // --- occurrence identity: RECURRENCE-ID overrides and the all-day value space (D8) ---------
    //
    // The skip set built in `parse_feed_within` prunes the series MASTER; applying it to the
    // override block deleted the override itself, and all-day starts lived in two different value
    // spaces so the skip never matched them at all.

    /// A 5-day all-day banner series, plus a window with headroom on both sides.
    ///
    /// The headroom is load-bearing: `rrule` resolves a `VALUE=DATE` DTSTART at midnight in the
    /// MACHINE's zone, so the first occurrence's instant sits up to 14 hours either side of UTC
    /// midnight and a window starting on the series' own first day would admit or drop it
    /// depending on where the test runs. What is under test is the civil date each admitted
    /// occurrence renders as, not which instants the window admits.
    const ALL_DAY_SERIES: &str = "BEGIN:VEVENT\nUID:s5\nSUMMARY:Daily banner\n\
                                  DTSTART;VALUE=DATE:20260601\nDTEND;VALUE=DATE:20260602\n\
                                  RRULE:FREQ=DAILY;COUNT=5\nEND:VEVENT";

    fn wide_window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            Utc.with_ymd_and_hms(2026, 5, 25, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        )
    }

    /// The three zones that expose an all-day drift: UTC and one on each side of it.
    fn zones() -> [ChronoTz; 3] {
        [
            ChronoTz::UTC,
            chrono_tz::Asia::Tokyo,
            chrono_tz::America::Los_Angeles,
        ]
    }

    #[test]
    fn recurrence_id_override_that_does_not_move_still_renders() {
        // The commonest override by far: "change just this one" edits the title or the room and
        // leaves the time alone, so the override's DTSTART equals its own RECURRENCE-ID. Applying
        // the series' skip set to the override block deleted it — and the master's occurrence at
        // that instant is (correctly) removed too, so the occurrence vanished from the calendar
        // altogether rather than merely losing its edit.
        let feed = "BEGIN:VEVENT\nUID:s3\nSUMMARY:Weekly\n\
                    DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:s3\nRECURRENCE-ID:20260608T100000Z\n\
                    SUMMARY:Weekly (new room)\nLOCATION:Room 5\n\
                    DTSTART:20260608T100000Z\nDTEND:20260608T110000Z\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        // 5 Mondays; Jun 8 is the override's, not the master's.
        assert_eq!(events.len(), 5);
        let edited: Vec<&CalendarEvent> = events
            .iter()
            .filter(|ev| ev.start == "2026-06-08T10:00:00Z")
            .collect();
        assert_eq!(
            edited.len(),
            1,
            "the edited occurrence renders exactly once"
        );
        assert_eq!(edited[0].summary, "Weekly (new room)");
        assert_eq!(edited[0].location.as_deref(), Some("Room 5"));
    }

    #[test]
    fn an_override_never_collides_with_the_master_row_id() {
        // `replace_events` skips an unchanged resync only while the stored row count equals
        // `events.len()`, and its INSERT OR REPLACE collapses two events sharing an id. Two rows
        // at one id would therefore make that gate unreachable and delete-and-reinsert the whole
        // calendar on every poll — the churn F-49 exists to prevent.
        let timed = "BEGIN:VEVENT\nUID:s3\nSUMMARY:Weekly\n\
                     DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                     RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT\n\
                     BEGIN:VEVENT\nUID:s3\nRECURRENCE-ID:20260608T100000Z\n\
                     SUMMARY:Weekly (new room)\nDTSTART:20260608T100000Z\nEND:VEVENT"
            .to_string();
        let all_day = format!(
            "{ALL_DAY_SERIES}\nBEGIN:VEVENT\nUID:s5\nRECURRENCE-ID;VALUE=DATE:20260603\n\
             SUMMARY:Banner (renamed)\nDTSTART;VALUE=DATE:20260603\n\
             DTEND;VALUE=DATE:20260604\nEND:VEVENT"
        );
        let (s, e) = wide_window();
        for tz in zones() {
            for feed in [&timed, &all_day] {
                let events = parse_feed_within(feed, "f", s, e, tz);
                let ids: HashSet<&String> = events.iter().map(|ev| &ev.id).collect();
                assert_eq!(ids.len(), events.len(), "duplicate row id in {tz}");
            }
        }
    }

    #[test]
    fn an_all_day_cancellation_override_removes_that_day() {
        // An all-day RECURRENCE-ID resolves through `parse_any`'s noon anchor while the master's
        // occurrences came back as raw midnights from `rrule`, so the skip set could never match
        // and a cancelled day still rendered. One value space is what makes the cancellation land.
        let feed = format!(
            "{ALL_DAY_SERIES}\nBEGIN:VEVENT\nUID:s5\nRECURRENCE-ID;VALUE=DATE:20260603\n\
             STATUS:CANCELLED\nDTSTART;VALUE=DATE:20260603\nEND:VEVENT"
        );
        let (s, e) = wide_window();
        for tz in zones() {
            let events = parse_feed_within(&feed, "f", s, e, tz);
            assert_eq!(events.len(), 4, "cancelled day still present in {tz}");
            assert!(!events.iter().any(|ev| ev.start == "2026-06-03"));
        }
    }

    #[test]
    fn an_all_day_override_replaces_only_that_day() {
        // The all-day twin of the headline: the override renders in place of the master's day,
        // once, carrying its own title — in every zone, since both sides of the comparison are
        // now civil dates anchored the same way.
        let feed = format!(
            "{ALL_DAY_SERIES}\nBEGIN:VEVENT\nUID:s5\nRECURRENCE-ID;VALUE=DATE:20260603\n\
             SUMMARY:Banner (renamed)\nDTSTART;VALUE=DATE:20260603\n\
             DTEND;VALUE=DATE:20260604\nEND:VEVENT"
        );
        let (s, e) = wide_window();
        for tz in zones() {
            let events = parse_feed_within(&feed, "f", s, e, tz);
            assert_eq!(events.len(), 5, "wrong occurrence count in {tz}");
            let on_day: Vec<&CalendarEvent> = events
                .iter()
                .filter(|ev| ev.start == "2026-06-03")
                .collect();
            assert_eq!(on_day.len(), 1, "the overridden day doubled in {tz}");
            assert_eq!(on_day[0].summary, "Banner (renamed)");
            assert!(events.iter().any(|ev| ev.start == "2026-06-02"));
        }
    }

    #[test]
    fn an_all_day_series_keeps_its_civil_date_across_zones() {
        // The recurring analogue of `all_day_keeps_its_calendar_date_across_zones`. An expanded
        // all-day occurrence is a civil date with no instant, so it must read as the day the feed
        // names in every zone; rendering `rrule`'s raw instant drifted the whole series a day.
        let (s, e) = wide_window();
        for tz in zones() {
            let events = parse_feed_within(ALL_DAY_SERIES, "f", s, e, tz);
            let dates: Vec<&str> = events.iter().map(|ev| ev.start.as_str()).collect();
            assert_eq!(
                dates,
                [
                    "2026-06-01",
                    "2026-06-02",
                    "2026-06-03",
                    "2026-06-04",
                    "2026-06-05"
                ],
                "all-day series drifted in {tz}"
            );
            assert!(events.iter().all(|ev| ev.all_day));
        }
    }

    #[test]
    fn chained_recurrence_id_overrides_each_keep_their_new_slot() {
        // Two overrides that swap Mondays: A takes B's slot and B takes A's. Both instants are in
        // the series' skip set, so the master vacates both and each override renders at its own
        // new start. This is the shape that discriminates the fix from "exclude only the block's
        // own RECURRENCE-ID from the skip set" — under that rule A's new start is still B's
        // RECURRENCE-ID, so A would be deleted anyway.
        let feed = "BEGIN:VEVENT\nUID:s4\nSUMMARY:Weekly\n\
                    DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:s4\nRECURRENCE-ID:20260608T100000Z\n\
                    SUMMARY:A moved to Jun 15\nDTSTART:20260615T100000Z\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:s4\nRECURRENCE-ID:20260615T100000Z\n\
                    SUMMARY:B moved to Jun 8\nDTSTART:20260608T100000Z\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 5);
        let at = |start: &str| {
            events
                .iter()
                .find(|ev| ev.start == start)
                .map(|ev| ev.summary.as_str())
        };
        assert_eq!(at("2026-06-15T10:00:00Z"), Some("A moved to Jun 15"));
        assert_eq!(at("2026-06-08T10:00:00Z"), Some("B moved to Jun 8"));
        let ids: HashSet<&String> = events.iter().map(|ev| &ev.id).collect();
        assert_eq!(ids.len(), events.len());
    }

    #[test]
    fn a_weekly_series_shares_one_uid_and_varies_only_the_start() {
        // The premise the mirror's occurrence key rests on (INVARIANTS I-04, and the (uid, start)
        // dedup in `calendar::agenda_query`): every expanded occurrence carries the SERIES uid, so
        // the uid alone names the series and only uid + instant names one occurrence. If
        // `make_event` ever minted a per-occurrence uid, that dedup would quietly become a no-op.
        let feed = "BEGIN:VEVENT\nUID:e\nSUMMARY:Weekly sync\n\
                    DTSTART:20260601T100000Z\nDTEND:20260601T103000Z\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT";
        let s = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let e = Utc.with_ymd_and_hms(2026, 6, 30, 0, 0, 0).unwrap();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 5);
        assert!(events.iter().all(|ev| ev.uid.as_deref() == Some("e")));
        let starts: HashSet<&str> = events.iter().map(|ev| ev.start.as_str()).collect();
        assert_eq!(starts.len(), 5, "occurrences must differ only by start");
    }

    #[test]
    fn a_feed_cut_mid_vevent_reports_the_parse_incomplete() {
        // A body that stops partway through a VEVENT (a truncated transfer that still reads as a
        // clean 200) loses that block silently — `vevents` drops the unclosed `cur`. The events it
        // DID parse are still good, so they're kept; what must not happen is the mirror treating
        // them as the whole picture and reaping the tail.
        let cut = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nSUMMARY:Standup\n\
                   DTSTART:20260615T090000Z\nEND:VEVENT\n\
                   BEGIN:VEVENT\nUID:b\nDTSTART:20260616T090000Z";
        let whole = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nSUMMARY:Standup\n\
                     DTSTART:20260615T090000Z\nEND:VEVENT\n\
                     BEGIN:VEVENT\nUID:b\nDTSTART:20260616T090000Z\nEND:VEVENT\n\
                     END:VCALENDAR";
        let (s, e) = window();
        let (events, complete) = parse_feed_within_reporting(cut, "f", s, e, ChronoTz::UTC);
        assert!(!complete, "an unterminated block is an incomplete parse");
        assert_eq!(events.len(), 1, "the terminated block still parses");
        let ids = |evs: &[CalendarEvent]| evs.iter().map(|ev| ev.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(&events),
            ids(&parse_feed_within(cut, "f", s, e, ChronoTz::UTC)),
            "the thin wrapper must parse identically"
        );

        let (events, complete) = parse_feed_within_reporting(whole, "f", s, e, ChronoTz::UTC);
        assert!(complete, "a properly terminated feed is complete");
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn hitting_the_expanded_event_cap_reports_incomplete() {
        // Both defensive caps drop events without failing, so both must report incompleteness — and
        // a feed comfortably inside them must not, or every sync would withhold every deletion
        // forever. Driven through the cap-injected core rather than a synthesised 100k-event feed.
        let feed = "BEGIN:VEVENT\nUID:a\nDTSTART:20260615T090000Z\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:b\nDTSTART:20260616T090000Z\nEND:VEVENT\n\
                    BEGIN:VEVENT\nUID:c\nDTSTART:20260617T090000Z\nEND:VEVENT";
        let (s, e) = window();
        let capped = |max_vevents, max_events| {
            let (events, complete) =
                parse_within_capped(feed, "f", s, e, ChronoTz::UTC, max_vevents, max_events);
            (events.len(), complete)
        };
        assert_eq!(capped(10, 10), (3, true), "well inside both caps");
        assert_eq!(capped(2, 10), (2, false), "a block past the VEVENT cap");
        assert_eq!(
            capped(10, 2),
            (2, false),
            "an occurrence past the event cap"
        );
        // Exactly on the occurrence cap reads as clamped: nothing distinguishes it from a feed that
        // had one more event to give.
        assert_eq!(capped(10, 3), (3, false), "landing exactly on the cap");
    }

    #[test]
    fn duration_property_supplies_the_end_time() {
        // DTSTART + DURATION (no DTEND) must yield a real end, not a zero-length point.
        let feed = "BEGIN:VEVENT\nUID:dur\nSUMMARY:Workshop\n\
                    DTSTART:20260615T090000Z\nDURATION:PT2H30M\nEND:VEVENT";
        let (s, e) = window();
        let events = parse_feed_within(feed, "f", s, e, ChronoTz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, "2026-06-15T09:00:00Z");
        assert_eq!(events[0].end.as_deref(), Some("2026-06-15T11:30:00Z"));
    }

    #[test]
    fn parses_duration_forms() {
        assert_eq!(parse_duration("PT1H"), Some(Duration::hours(1)));
        assert_eq!(parse_duration("P2W"), Some(Duration::weeks(2)));
        assert_eq!(
            parse_duration("P1DT2H30M"),
            Some(Duration::minutes(24 * 60 + 150))
        );
        assert_eq!(parse_duration("-PT30M"), Some(Duration::minutes(-30)));
        assert_eq!(parse_duration("P"), None);
        assert_eq!(parse_duration("PT1X"), None);
    }

    // --- ATTENDEE parsing (card 11 audit) -------------------------------------------------------
    //
    // Google and Graph have populated attendees since v40; ICS did not, so every Apple/ICS
    // subscription mirrored an empty guest list. These pin the gap closed.

    fn block(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    /// One ATTENDEE line per guest — taking only the first would report a meeting as having one.
    #[test]
    fn every_attendee_line_is_parsed() {
        let b = block(&[
            "ATTENDEE;CN=Ada Lovelace;PARTSTAT=ACCEPTED:mailto:ada@example.com",
            "ATTENDEE;CN=Alan Turing;ROLE=OPT-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:alan@example.com",
            "ATTENDEE:mailto:bare@example.com",
        ]);
        let got = ics_attendees(&b);
        assert_eq!(got.len(), 3);

        assert_eq!(got[0].name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(got[0].email.as_deref(), Some("ada@example.com"));
        assert_eq!(got[0].response.as_deref(), Some("accepted"));
        assert!(!got[0].optional);

        assert_eq!(got[1].response.as_deref(), Some("tentative"));
        assert!(got[1].optional, "OPT-PARTICIPANT is the optional role");

        // A bare `ATTENDEE:mailto:…` is valid and common — it must still round-trip.
        assert_eq!(got[2].email.as_deref(), Some("bare@example.com"));
        assert_eq!(got[2].name, None);
        assert_eq!(got[2].response, None);
    }

    /// PARTSTAT lands in the same vocabulary Google/Graph are mapped onto, and an unknown value is
    /// passed through rather than silently becoming "needsAction".
    #[test]
    fn partstat_normalises_to_the_shared_vocabulary() {
        assert_eq!(partstat("NEEDS-ACTION"), "needsAction");
        assert_eq!(partstat("declined"), "declined");
        assert_eq!(partstat("X-WEIRD"), "x-weird");
    }

    /// Property names are case-insensitive per RFC 5545, and a VEVENT with no guests is empty
    /// rather than a phantom entry.
    #[test]
    fn attendee_matching_is_case_insensitive_and_absence_is_empty() {
        let b = block(&["attendee;cn=Grace:MAILTO:grace@example.com"]);
        let got = ics_attendees(&b);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name.as_deref(), Some("Grace"));
        assert_eq!(got[0].email.as_deref(), Some("grace@example.com"));

        assert!(ics_attendees(&block(&["SUMMARY:Solo work"])).is_empty());
    }

    /// An ICS feed can't say which guest is the connected account, and CHAIR is a role rather than
    /// a claim of organisership — guessing either would mislabel real people in the popup.
    #[test]
    fn ics_never_claims_self_or_organizer() {
        let b = block(&["ATTENDEE;CN=Chair;ROLE=CHAIR:mailto:chair@example.com"]);
        let got = ics_attendees(&b);
        assert!(!got[0].is_self);
        assert!(!got[0].organizer);
        assert!(!got[0].optional, "CHAIR is required, not optional");
    }
}
