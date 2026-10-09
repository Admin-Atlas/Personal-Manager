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
/// caller GETs that id before sending again (rule R2; Google warns it can't always catch a repeated
/// id at creation), so a retry can't make a second event.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C10, creating events")
)]
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

/// A new event id of PM's choosing: 32 lowercase hex characters, inside Google's allowed alphabet
/// (base32hex: `0-9` and `a-v`, 5 to 1024 characters).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller lands in C10, creating events")
)]
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
