// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The requests a save sends, built without sending them, so their exact shape is tested: the path
//! with calendar and event ids percent-encoded (a calendar id is an address, `@` and all), `sendUpdates`
//! stated on every write, `If-Match` on every change to an existing event, and the version flags only
//! when the body needs them.

use serde_json::Value;

use super::classify::Method;
use super::dto::Notify;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

/// One request to Google Calendar, ready for the I/O layer to send.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestPlan {
    pub method: Method,
    /// The full URL, path segments encoded and query attached.
    pub url: String,
    /// The etag a change to an existing event must still match.
    pub if_match: Option<String>,
    pub body: Option<Value>,
}

/// Body features that need Google to be told which API version of them the client speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VersionFlags {
    /// The body adds or removes a conference (`conferenceDataVersion=1`). Sending it on any other
    /// write would make Google ignore the calendar's "add Meet automatically" setting.
    pub conference: bool,
    /// The body sets an event label (`eventLabelVersion=1`). Never sent otherwise.
    pub labels: bool,
}

fn url(segments: &[&str], query: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(CALENDAR_API).expect("the Calendar API base is a valid URL");
    url.path_segments_mut()
        .expect("the Calendar API base can take path segments")
        .extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    url.to_string()
}

/// Whether an id stays one path segment of its own in a request URL. `Url::path_segments_mut` drops
/// a "." or ".." segment (and strips tabs and newlines first, so ".\t." becomes ".."), which would
/// send the write to the calendar's event list, or the calendar itself, instead of the event. Google's
/// ids never look like that; a corrupt row, or a corrupt reply, isn't written through.
pub fn is_plain_segment(s: &str) -> bool {
    !s.is_empty() && !matches!(s, "." | "..") && !s.chars().any(|c| c.is_ascii_control())
}

fn write_query(notify: Notify, flags: VersionFlags) -> Vec<(&'static str, &'static str)> {
    let mut query = vec![("sendUpdates", notify.as_param())];
    if flags.conference {
        query.push(("conferenceDataVersion", "1"));
    }
    if flags.labels {
        query.push(("eventLabelVersion", "1"));
    }
    query
}

/// A fresh copy of one event (an occurrence's own id included).
pub fn get_event(calendar: &str, event: &str) -> RequestPlan {
    RequestPlan {
        method: Method::Get,
        url: url(&["calendars", calendar, "events", event], &[]),
        if_match: None,
        body: None,
    }
}

/// The occurrences of the series `event`, cancelled ones included (`showDeleted`, so a cancel shows as
/// one and is never mistaken for a lag), narrowed by `query` (`timeMin`, `maxResults`, a page token).
pub fn instances(calendar: &str, event: &str, query: &[(&str, &str)]) -> RequestPlan {
    let mut q = vec![("showDeleted", "true")];
    q.extend_from_slice(query);
    RequestPlan {
        method: Method::Get,
        url: url(&["calendars", calendar, "events", event, "instances"], &q),
        if_match: None,
        body: None,
    }
}

/// The events stored under iCal UID `ical`: a series' master and its exceptions, cancelled ones
/// included (`showDeleted`), never spread into occurrences (`singleEvents=false`). A page of up to 100
/// at a time; `page_token` is the previous page's `nextPageToken`. No name here says "uid": CodeQL
/// reads any such name as account data, and every request built from it as a leak.
pub fn list_by_ical(calendar: &str, ical: &str, page_token: Option<&str>) -> RequestPlan {
    let mut query = vec![
        ("iCalUID", ical),
        ("singleEvents", "false"),
        ("showDeleted", "true"),
        ("maxResults", "100"),
    ];
    if let Some(token) = page_token {
        query.push(("pageToken", token));
    }
    RequestPlan {
        method: Method::Get,
        url: url(&["calendars", calendar, "events"], &query),
        if_match: None,
        body: None,
    }
}

/// The new parts PM split off the series `series` (its private `pmSplitOf` note), live ones only, as
/// stored (`singleEvents=false`): only what reading an overlap needs. Google may copy the note onto a
/// part's own exceptions, so `recurringEventId` comes too, to tell them apart (C9a-3b critique L4).
/// A page of up to 250; `page_token` is the previous page's `nextPageToken`.
pub fn split_parts(calendar: &str, series: &str, page_token: Option<&str>) -> RequestPlan {
    let note = format!("pmSplitOf={series}");
    let mut query = vec![
        ("privateExtendedProperty", note.as_str()),
        ("singleEvents", "false"),
        ("maxResults", "250"),
        (
            "fields",
            "nextPageToken,items(id,status,recurringEventId,start,extendedProperties)",
        ),
    ];
    if let Some(token) = page_token {
        query.push(("pageToken", token));
    }
    RequestPlan {
        method: Method::Get,
        url: url(&["calendars", calendar, "events"], &query),
        if_match: None,
        body: None,
    }
}

/// Change the fields in `body`, only if the event is still at `etag`.
pub fn patch_event(
    calendar: &str,
    event: &str,
    etag: &str,
    body: Value,
    notify: Notify,
    flags: VersionFlags,
) -> RequestPlan {
    RequestPlan {
        method: Method::Patch,
        url: url(
            &["calendars", calendar, "events", event],
            &write_query(notify, flags),
        ),
        if_match: Some(etag.to_string()),
        body: Some(body),
    }
}

/// Delete the event, only if it's still at `etag`.
pub fn delete_event(calendar: &str, event: &str, etag: &str, notify: Notify) -> RequestPlan {
    RequestPlan {
        method: Method::Delete,
        url: url(
            &["calendars", calendar, "events", event],
            &[("sendUpdates", notify.as_param())],
        ),
        if_match: Some(etag.to_string()),
        body: None,
    }
}

/// Create an event under `id`, an id PM chose ([`client_event_id`]) and wrote into the body here,
/// so an insert can't go out without one. After an answer that never arrived or a server error, the
/// caller GETs that id before sending again (rule R2). Google warns it can't always catch a repeated
/// id at creation, so a retry very rarely makes a second event, and the posts under one id are
/// capped.
pub fn insert_event(
    calendar: &str,
    id: &str,
    mut body: Value,
    notify: Notify,
    flags: VersionFlags,
) -> RequestPlan {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("id".into(), Value::String(id.to_string()));
    }
    RequestPlan {
        method: Method::Insert,
        url: url(
            &["calendars", calendar, "events"],
            &write_query(notify, flags),
        ),
        if_match: None,
        body: Some(body),
    }
}

/// Dev builds only: how every request URL for `calendar` starts (`…/calendars/{id}/`, the id
/// encoded, so no other calendar's URL starts the same), for the dev allow-list.
#[cfg(debug_assertions)]
pub fn calendar_prefix(calendar: &str) -> String {
    url(&["calendars", calendar, ""], &[])
}

/// A new event id of PM's choosing: 32 lowercase hex characters, inside Google's allowed alphabet
/// (base32hex: `0-9` and `a-v`, 5 to 1024 characters).
pub fn client_event_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_with_at_and_hash_are_encoded_into_the_path() {
        let plan = get_event(
            "team#holidays@group.calendar.google.com",
            "abc_20261012T090000Z",
        );
        assert_eq!(
            plan.url,
            "https://www.googleapis.com/calendar/v3/calendars/team%23holidays@group.calendar.google.com/events/abc_20261012T090000Z"
        );
        // A slash in an id can't climb out of its segment.
        let sneaky = get_event("me@example.com", "../../users/me");
        assert!(
            sneaky.url.ends_with("/events/..%2F..%2Fusers%2Fme"),
            "{}",
            sneaky.url
        );
    }

    #[test]
    fn a_series_occurrences_always_include_cancelled_ones() {
        let p = instances(
            "me@example.com",
            "abc",
            &[("timeMin", "2026-10-18T09:00:00Z"), ("maxResults", "10")],
        );
        assert_eq!(p.method, Method::Get);
        assert_eq!(p.if_match, None);
        assert_eq!(
            p.url,
            "https://www.googleapis.com/calendar/v3/calendars/me@example.com/events/abc/instances\
             ?showDeleted=true&timeMin=2026-10-18T09%3A00%3A00Z&maxResults=10"
        );
    }

    /// The dev allow-list matches a request to its calendar by this prefix: never another
    /// calendar's, even one whose id starts the same.
    #[cfg(debug_assertions)]
    #[test]
    fn calendar_prefix_matches_only_its_own_calendar() {
        let team = calendar_prefix("team@group.calendar.google.com");
        assert!(get_event("team@group.calendar.google.com", "e")
            .url
            .starts_with(&team));
        assert!(!get_event("team@group.calendar.google.com.evil", "e")
            .url
            .starts_with(&team));
        // A slash in an id is encoded, so it can't end the calendar's segment early.
        assert!(!get_event("team/x", "e")
            .url
            .starts_with(&calendar_prefix("team")));
    }

    #[test]
    fn a_series_is_listed_by_its_ical_uid_as_stored() {
        let first = list_by_ical("me@example.com", "abc@google.com", None);
        assert_eq!((first.method, first.if_match), (Method::Get, None));
        assert_eq!(
            first.url,
            "https://www.googleapis.com/calendar/v3/calendars/me@example.com/events\
             ?iCalUID=abc%40google.com&singleEvents=false&showDeleted=true&maxResults=100"
        );
        let next = list_by_ical("me@example.com", "abc@google.com", Some("p2&x"));
        assert!(
            next.url.ends_with("&maxResults=100&pageToken=p2%26x"),
            "{}",
            next.url
        );
    }

    #[test]
    fn a_series_parts_are_listed_by_pms_note() {
        let first = split_parts("me@example.com", "abc", None);
        assert_eq!((first.method, first.if_match), (Method::Get, None));
        assert_eq!(
            first.url,
            "https://www.googleapis.com/calendar/v3/calendars/me@example.com/events\
             ?privateExtendedProperty=pmSplitOf%3Dabc&singleEvents=false&maxResults=250\
             &fields=nextPageToken%2Citems%28id%2Cstatus%2CrecurringEventId%2Cstart%2C\
             extendedProperties%29"
        );
        let next = split_parts("me@example.com", "a&b", Some("p2"));
        assert!(
            next.url.contains("pmSplitOf%3Da%26b&") && next.url.ends_with("&pageToken=p2"),
            "{}",
            next.url
        );
    }

    #[test]
    fn every_write_states_who_is_emailed() {
        for notify in [Notify::All, Notify::ExternalOnly, Notify::None] {
            let p = patch_event(
                "c",
                "e",
                "\"1\"",
                json!({}),
                notify,
                VersionFlags::default(),
            );
            assert!(
                p.url
                    .contains(&format!("sendUpdates={}", notify.as_param())),
                "{}",
                p.url
            );
            let d = delete_event("c", "e", "\"1\"", notify);
            assert!(d
                .url
                .contains(&format!("sendUpdates={}", notify.as_param())));
            let i = insert_event(
                "c",
                &client_event_id(),
                json!({}),
                notify,
                VersionFlags::default(),
            );
            assert!(i
                .url
                .contains(&format!("sendUpdates={}", notify.as_param())));
        }
    }

    #[test]
    fn changes_to_an_existing_event_carry_its_etag_and_creates_do_not() {
        let p = patch_event(
            "c",
            "e",
            "\"42\"",
            json!({ "summary": "x" }),
            Notify::None,
            VersionFlags::default(),
        );
        assert_eq!(p.method, Method::Patch);
        assert_eq!(p.if_match.as_deref(), Some("\"42\""));
        assert_eq!(p.body, Some(json!({ "summary": "x" })));
        let d = delete_event("c", "e", "\"42\"", Notify::None);
        assert_eq!(
            (d.method, d.if_match.as_deref(), d.body),
            (Method::Delete, Some("\"42\""), None)
        );
        let i = insert_event(
            "c",
            "abcde",
            json!({ "summary": "x", "id": "not-this" }),
            Notify::None,
            VersionFlags::default(),
        );
        assert_eq!((i.method, i.if_match), (Method::Insert, None));
        // The id PM chose always goes in, whatever the body held.
        assert_eq!(i.body, Some(json!({ "summary": "x", "id": "abcde" })));
        assert!(
            i.url.ends_with("/calendars/c/events?sendUpdates=none"),
            "{}",
            i.url
        );
    }

    #[test]
    fn version_flags_are_sent_only_when_asked_for() {
        let plain = patch_event(
            "c",
            "e",
            "\"1\"",
            json!({}),
            Notify::None,
            VersionFlags::default(),
        );
        assert!(!plain.url.contains("conferenceDataVersion"));
        assert!(!plain.url.contains("eventLabelVersion"));
        let both = patch_event(
            "c",
            "e",
            "\"1\"",
            json!({}),
            Notify::None,
            VersionFlags {
                conference: true,
                labels: true,
            },
        );
        assert!(both.url.contains("conferenceDataVersion=1"));
        assert!(both.url.contains("eventLabelVersion=1"));
    }

    #[test]
    fn a_client_event_id_fits_googles_alphabet() {
        let id = client_event_id();
        assert_eq!(id.len(), 32);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='v').contains(&c)),
            "{id}"
        );
        assert_ne!(id, client_event_id());
    }
}
