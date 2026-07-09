// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Read-only Outlook / Microsoft 365 calendar over Microsoft Graph (board card 6A) — the OAuth
//! sibling of the Google bits in [`crate::calendar`], and the Microsoft counterpart that flows into
//! the same multi-provider mirror (account → calendar → event). Auth is reused wholesale from
//! [`crate::microsoft`] (the OneDrive provider): the same public-client PKCE consent, the same
//! per-account token blob in the keychain, `Calendars.Read` only — PM never writes (spec non-goal #4).
//!
//! Two Graph specifics this module owns:
//! 1. **`calendarView`** (not `/events`) — given a time window it PRE-EXPANDS recurring series into
//!    individual instances, exactly like Google's `singleEvents=true`, so read-only rendering never
//!    touches RRULE.
//! 2. **UTC by default** — without a `Prefer: outlook.timezone` header Graph returns every
//!    `start`/`end` in UTC, so we parse the naive `dateTime` as a UTC instant and normalise it to the
//!    same `…Z` / `YYYY-MM-DD` shape the rest of the calendar code uses.
//!
//! Everything Graph sends is untrusted DATA, never instructions (rule #6).

use chrono::NaiveDateTime;
use serde_json::Value;

use crate::calendar::{CalendarEvent, RawCalendarInput};
use crate::error::{Error, Result};
use crate::{microsoft, secrets};

/// How many events to request per page, and the page-follow guard (a backstop, not a coverage cap —
/// even the wide ~13-month mirror band keeps any real calendar far under 250 × 100 events).
const PAGE_SIZE: usize = 250;
const MAX_PAGES: usize = 100;
/// The field projection for one event — only what the mirror needs.
const SELECT_EVENT: &str =
    "id,subject,bodyPreview,location,start,end,isAllDay,isCancelled,webLink,iCalUId";

// --- identity / namespacing ----------------------------------------------------------------------

/// The keychain token key for one Outlook calendar account (`<prefix><email>`).
pub fn account_token_key(email: &str) -> String {
    secrets::token_key_for("microsoft", "calendar", email)
        .expect("microsoft/calendar is a token-bearing pair")
}

/// The `connector_sources.id` (and `calendars.source_id`) for one Outlook account.
pub fn account_id(email: &str) -> String {
    format!("outlook:{email}")
}

// --- network (async, DB-free; callers hold no lock across these — rule #4) -----------------------

/// The account a fresh token grants (email + display name), via Graph `/me` — to learn which account
/// to save the token under, right after consent.
pub async fn me_account(token: &microsoft::Token) -> Result<(String, String)> {
    microsoft::me(token).await
}

/// Fetch one account's calendars, authorised with its `token_key`. Returns the provider-neutral
/// registration input the shared [`crate::calendar::register_calendars`] consumes.
pub async fn list_calendars(token_key: &str) -> Result<Vec<RawCalendarInput>> {
    let url = format!(
        "{}/me/calendars?$select=id,name,color,isDefaultCalendar&$top=200",
        microsoft::GRAPH_API
    );
    // Graph's `@odata.nextLink` IS the page cursor (a full URL); the first page uses the initial
    // URL. The page guard is a pure runaway backstop, so the truncated flag is discarded.
    let (all, _truncated) = crate::connector_sync::paginate(MAX_PAGES, |cursor| {
        let url = url.as_str();
        async move {
            let u = cursor.unwrap_or_else(|| url.to_string());
            let v = microsoft::authorized_get(token_key, &u).await?;
            Ok(parse_calendar_list(&v))
        }
    })
    .await?;
    Ok(all)
}

/// Fetch one calendar's events within `[time_min, time_max]` (RFC3339), recurrences pre-expanded via
/// `calendarView`, ordered by start. `mirror_calendar_id` is the owning [`crate::calendar::Calendar::
/// id`] the events are stored under; `remote_id` is Graph's own calendar id for the path.
pub async fn fetch_events(
    token_key: &str,
    mirror_calendar_id: &str,
    remote_id: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<CalendarEvent>> {
    // Build the path via `path_segments_mut` (not string interpolation): a Graph calendar id is an
    // opaque, often base64url token that can contain `/`, which spliced into the path raw would
    // become extra segments and 404 the request — silently dropping a non-default calendar. Mirrors
    // the Google sibling in `crate::calendar::fetch_events`.
    let mut url =
        reqwest::Url::parse(microsoft::GRAPH_API).map_err(|e| Error::Other(e.to_string()))?;
    url.path_segments_mut()
        .map_err(|_| Error::Other("invalid Graph API base".into()))?
        .extend(["me", "calendars", remote_id, "calendarView"]);
    url.query_pairs_mut()
        .append_pair("startDateTime", time_min)
        .append_pair("endDateTime", time_max)
        .append_pair("$select", SELECT_EVENT)
        .append_pair("$orderby", "start/dateTime")
        .append_pair("$top", &PAGE_SIZE.to_string());

    // `@odata.nextLink` is the cursor here too; the guard stays a silent backstop (flag discarded),
    // matching the prior behaviour.
    let initial = url.to_string();
    let (out, _truncated) = crate::connector_sync::paginate(MAX_PAGES, |cursor| {
        let initial = initial.as_str();
        async move {
            let u = cursor.unwrap_or_else(|| initial.to_string());
            let v = microsoft::authorized_get(token_key, &u).await?;
            Ok(parse_events(mirror_calendar_id, &v))
        }
    })
    .await?;
    Ok(out)
}

// --- pure parsing (unit-tested) ------------------------------------------------------------------

/// Parse a `/me/calendars` page → its calendars + the next-page link. Graph's `color` is a named enum
/// (`auto`, `lightBlue`, …); `auto` means "no specific colour", so it maps to `None`.
fn parse_calendar_list(value: &Value) -> (Vec<RawCalendarInput>, Option<String>) {
    let items = value
        .get("value")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let remote_id = c.get("id").and_then(Value::as_str)?.to_string();
                    let name = c
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("(calendar)")
                        .to_string();
                    let color = c
                        .get("color")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty() && *s != "auto")
                        .map(str::to_string);
                    let is_primary = c
                        .get("isDefaultCalendar")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some(RawCalendarInput {
                        remote_id,
                        name,
                        color,
                        is_primary,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("@odata.nextLink")
        .and_then(Value::as_str)
        .map(String::from);
    (items, next)
}

/// Parse a `calendarView` page → its events (skipping cancelled) + the next-page link.
fn parse_events(mirror_calendar_id: &str, value: &Value) -> (Vec<CalendarEvent>, Option<String>) {
    let events = value
        .get("value")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| parse_event(mirror_calendar_id, e))
                .collect()
        })
        .unwrap_or_default();
    let next = value
        .get("@odata.nextLink")
        .and_then(Value::as_str)
        .map(String::from);
    (events, next)
}

fn parse_event(mirror_calendar_id: &str, e: &Value) -> Option<CalendarEvent> {
    if e.get("isCancelled").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let all_day = e.get("isAllDay").and_then(Value::as_bool).unwrap_or(false);
    let start = e
        .get("start")
        .and_then(|s| s.get("dateTime"))
        .and_then(Value::as_str)
        .and_then(|dt| graph_datetime_to_iso(dt, all_day))?;
    let end = e
        .get("end")
        .and_then(|s| s.get("dateTime"))
        .and_then(Value::as_str)
        .and_then(|dt| graph_datetime_to_iso(dt, all_day));
    // The iCalUId is the durable cross-provider anchor and lives in `uid`. The mirror id, though,
    // must key on Graph's PER-INSTANCE `id`: `calendarView` pre-expands a recurring series into
    // instances that all SHARE one iCalUId, so using the UID as the suffix would make every
    // occurrence collide on `<calendar>:<uid>` and `INSERT OR REPLACE` collapse the series to a
    // single mirror row. Fall back to the UID, then the start, only when Graph omits the id.
    let uid = e
        .get("iCalUId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let id_suffix = e
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| uid.clone())
        .unwrap_or_else(|| start.clone());
    Some(CalendarEvent {
        id: format!("{mirror_calendar_id}:{id_suffix}"),
        calendar_id: mirror_calendar_id.to_string(),
        summary: e
            .get("subject")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("(no title)")
            .to_string(),
        description: e
            .get("bodyPreview")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        location: e
            .get("location")
            .and_then(|l| l.get("displayName"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        start,
        end,
        all_day,
        html_link: e.get("webLink").and_then(Value::as_str).map(str::to_string),
        uid,
    })
}

/// Normalise a Graph `dateTime` (UTC, e.g. `2026-06-27T09:00:00.0000000`) to the mirror's shape: a
/// plain `YYYY-MM-DD` for an all-day event, else `YYYY-MM-DDTHH:MM:SSZ`. Tolerates a missing or
/// variable fractional part and an already-`Z`-suffixed value.
fn graph_datetime_to_iso(dt: &str, all_day: bool) -> Option<String> {
    let trimmed = dt.trim();
    if all_day {
        return trimmed.get(0..10).map(str::to_string);
    }
    // Drop any fractional seconds and a trailing Z, then parse the naive seconds-precision instant.
    let base = trimmed.split('.').next().unwrap_or(trimmed);
    let base = base.trim_end_matches('Z');
    let naive = NaiveDateTime::parse_from_str(base, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(naive.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_identity_is_namespaced_per_email() {
        assert_eq!(account_id("a@b.com"), "outlook:a@b.com");
        assert_eq!(
            account_token_key("a@b.com"),
            "microsoft_oauth_token_calendar::a@b.com"
        );
    }

    #[test]
    fn graph_datetime_normalises_timed_and_all_day() {
        assert_eq!(
            graph_datetime_to_iso("2026-06-27T09:00:00.0000000", false).as_deref(),
            Some("2026-06-27T09:00:00Z")
        );
        // Already-Z and no-fraction shapes both work.
        assert_eq!(
            graph_datetime_to_iso("2026-06-27T09:00:00Z", false).as_deref(),
            Some("2026-06-27T09:00:00Z")
        );
        // All-day keeps just the civil date.
        assert_eq!(
            graph_datetime_to_iso("2026-06-27T00:00:00.0000000", true).as_deref(),
            Some("2026-06-27")
        );
        assert_eq!(graph_datetime_to_iso("not-a-date", false), None);
    }

    #[test]
    fn parse_calendar_list_reads_name_color_primary_and_next() {
        let v = serde_json::json!({
            "value": [
                {"id": "AAA", "name": "Calendar", "color": "auto", "isDefaultCalendar": true},
                {"id": "BBB", "name": "Work", "color": "lightGreen", "isDefaultCalendar": false}
            ],
            "@odata.nextLink": "https://graph/next"
        });
        let (cals, next) = parse_calendar_list(&v);
        assert_eq!(next.as_deref(), Some("https://graph/next"));
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].remote_id, "AAA");
        assert!(cals[0].is_primary);
        assert_eq!(cals[0].color, None); // "auto" → no specific colour
        assert_eq!(cals[1].name, "Work");
        assert_eq!(cals[1].color.as_deref(), Some("lightGreen"));
    }

    #[test]
    fn parse_events_maps_timed_all_day_uid_and_skips_cancelled() {
        let v = serde_json::json!({
            "value": [
                {
                    "subject": "Standup", "iCalUId": "UID-1",
                    "start": {"dateTime": "2026-06-27T09:00:00.0000000", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-06-27T09:30:00.0000000", "timeZone": "UTC"},
                    "isAllDay": false, "isCancelled": false,
                    "location": {"displayName": "Room 1"}, "webLink": "https://outlook/1"
                },
                {
                    "subject": "Offsite", "iCalUId": "UID-2",
                    "start": {"dateTime": "2026-06-28T00:00:00.0000000", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-06-29T00:00:00.0000000", "timeZone": "UTC"},
                    "isAllDay": true
                },
                {
                    "subject": "Dropped", "isCancelled": true,
                    "start": {"dateTime": "2026-06-27T10:00:00.0000000", "timeZone": "UTC"},
                    "isAllDay": false
                }
            ]
        });
        let (events, next) = parse_events("outlook:a@b.com:AAA", &v);
        assert!(next.is_none());
        assert_eq!(events.len(), 2, "cancelled event is skipped");

        let timed = &events[0];
        assert_eq!(timed.id, "outlook:a@b.com:AAA:UID-1");
        assert_eq!(timed.calendar_id, "outlook:a@b.com:AAA");
        assert_eq!(timed.summary, "Standup");
        assert_eq!(timed.start, "2026-06-27T09:00:00Z");
        assert_eq!(timed.end.as_deref(), Some("2026-06-27T09:30:00Z"));
        assert!(!timed.all_day);
        assert_eq!(timed.location.as_deref(), Some("Room 1"));
        assert_eq!(timed.uid.as_deref(), Some("UID-1"));

        let allday = &events[1];
        assert!(allday.all_day);
        assert_eq!(allday.start, "2026-06-28");
        assert_eq!(allday.end.as_deref(), Some("2026-06-29"));
    }

    #[test]
    fn recurring_instances_sharing_one_ical_uid_get_distinct_mirror_ids() {
        // calendarView pre-expands a series: every instance shares one iCalUId but has a unique
        // per-instance `id`. Keying the mirror id on the instance id keeps them distinct — keying
        // on the UID would collapse the whole series to one row via INSERT OR REPLACE.
        let v = serde_json::json!({
            "value": [
                {
                    "id": "INST-A", "subject": "Weekly", "iCalUId": "SERIES-UID",
                    "start": {"dateTime": "2026-06-01T09:00:00.0000000", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-06-01T09:30:00.0000000", "timeZone": "UTC"},
                    "isAllDay": false
                },
                {
                    "id": "INST-B", "subject": "Weekly", "iCalUId": "SERIES-UID",
                    "start": {"dateTime": "2026-06-08T09:00:00.0000000", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-06-08T09:30:00.0000000", "timeZone": "UTC"},
                    "isAllDay": false
                }
            ]
        });
        let (events, _) = parse_events("outlook:a@b.com:AAA", &v);
        assert_eq!(events.len(), 2);
        assert_ne!(events[0].id, events[1].id, "instances must not collide");
        assert_eq!(events[0].id, "outlook:a@b.com:AAA:INST-A");
        assert_eq!(events[1].id, "outlook:a@b.com:AAA:INST-B");
        // Both keep the shared UID for cross-provider dedup.
        assert_eq!(events[0].uid.as_deref(), Some("SERIES-UID"));
        assert_eq!(events[1].uid.as_deref(), Some("SERIES-UID"));
    }
}
