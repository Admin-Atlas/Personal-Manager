// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An event's video call (C18): what kind it is and how far Google has got with it, the body that
//! asks Google for a new Meet call, and when two calls are the same. Only Meet is ever asked for;
//! another service's call is read, never changed. Never reads a call's notes (they may be HTML) or
//! its phone entries (they carry PINs). Nothing here fetches or sends.

use serde_json::{json, Map, Value};

use super::dto::{CallForEdit, CallKind, CallState, EventPatchDraft, MeetChange};
use super::patch::PatchError;

/// Google's name for a Meet call's kind (`conferenceSolutionKey.type`).
const MEET: &str = "hangoutsMeet";

/// The longest service name the editor is given, in characters.
const NAME_MAX: usize = 100;

/// Where a Meet link starts, for a mirror row that keeps only the link.
const MEET_LINK: &str = "https://meet.google.com/";

/// The video call Google's copy `event` has, or `None` for none.
pub fn call_of(event: &Value) -> Option<CallForEdit> {
    let link = event
        .get("hangoutLink")
        .and_then(Value::as_str)
        .filter(|l| !l.is_empty());
    let Some(data) = event.get("conferenceData").filter(|d| !d.is_null()) else {
        // A bare link with no call details: a kind PM can show but not change.
        return link.map(|l| CallForEdit {
            kind: CallKind::Other,
            name: String::new(),
            join: https(l),
            state: CallState::Ready,
        });
    };
    let name: String = data
        .pointer("/conferenceSolution/name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect();
    // Only a video entry point is ever read: the others carry phone numbers and PINs.
    let video = data
        .get("entryPoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| e.get("entryPointType").and_then(Value::as_str) == Some("video"))
        .find_map(|e| e.get("uri").and_then(Value::as_str).and_then(https));
    Some(CallForEdit {
        kind: kind_of(data),
        name,
        join: video.or_else(|| link.and_then(https)),
        state: state_of(data),
    })
}

/// A link to put in front of the user: an `https://` one only.
fn https(link: &str) -> Option<String> {
    link.starts_with("https://").then(|| link.to_string())
}

/// The kind of call a `conferenceData` value is: its solution's, else its request's. Google says a
/// client should still show a kind it doesn't know, and not change it.
fn kind_of(data: &Value) -> CallKind {
    let kind = data
        .pointer("/conferenceSolution/key/type")
        .or_else(|| data.pointer("/createRequest/conferenceSolutionKey/type"))
        .and_then(Value::as_str);
    match kind {
        Some(MEET) => CallKind::Meet,
        Some("addOn") => CallKind::AddOn,
        _ => CallKind::Other,
    }
}

/// How far Google has got with a `conferenceData` value's request; no request is a call made.
fn state_of(data: &Value) -> CallState {
    match data
        .pointer("/createRequest/status/statusCode")
        .and_then(Value::as_str)
    {
        Some("pending") => CallState::Pending,
        Some("failure") => CallState::Failed,
        _ => CallState::Ready,
    }
}

/// A Meet call made, or one on its way: not a request that failed.
fn live_meet(data: &Value) -> bool {
    kind_of(data) == CallKind::Meet && state_of(data) != CallState::Failed
}

/// Whether `event` has a Meet call or one on its way (pending). A failed request is no call.
pub fn has_meet(event: &Value) -> bool {
    call_of(event).is_some_and(|c| c.kind == CallKind::Meet && c.state != CallState::Failed)
}

/// Whether a Meet call may be asked for on `event`: no call at all, or a Meet request that failed.
pub fn room_for_meet(event: &Value) -> bool {
    match call_of(event) {
        None => true,
        Some(c) => c.kind == CallKind::Meet && c.state == CallState::Failed,
    }
}

/// A mirror row's link read as a call kind, for the list (the mirror keeps only a link): a
/// `https://meet.google.com/` link is Meet, any other link `Other`.
pub fn kind_of_link(link: &str) -> CallKind {
    if link.starts_with(MEET_LINK) {
        CallKind::Meet
    } else {
        CallKind::Other
    }
}

/// The `conferenceData` that asks Google for a new Meet call under `request` (sent at version 1).
/// The kind is named, though Google's own sample leaves it out: never Google's guess.
pub fn meet_request_body(request: &str) -> Value {
    json!({ "createRequest": { "requestId": request, "conferenceSolutionKey": { "type": MEET } } })
}

/// What `draft` sends for the call against `fresh`, Google's copy now: `Some(body)` to ask for a new
/// Meet call, `Some(Value::Null)` to remove the call, `None` to send nothing. A call Google already
/// has (or already doesn't) sends nothing; another service's call sends nothing (the gate refuses it
/// first). `MeetUnready` when a call would be asked for and `draft.meet_request` is `None`, which a
/// draft the backend put its session's id in never is.
pub fn meet_value(fresh: &Value, draft: &EventPatchDraft) -> Result<Option<Value>, PatchError> {
    match draft.meet {
        None => Ok(None),
        Some(MeetChange::Add) if room_for_meet(fresh) => {
            let request = draft
                .meet_request
                .as_deref()
                .ok_or(PatchError::MeetUnready)?;
            Ok(Some(meet_request_body(request)))
        }
        Some(MeetChange::Remove) if has_meet(fresh) => Ok(Some(Value::Null)),
        Some(_) => Ok(None),
    }
}

/// A request PM built for a new call: a `createRequest` Google hasn't answered (no status) and no
/// call yet.
fn asked(data: &Value) -> bool {
    data.get("createRequest").is_some()
        && data.pointer("/createRequest/status").is_none()
        && data.get("conferenceId").is_none()
}

/// Whether two `conferenceData` values say the same call: none and none (absent and `null` alike);
/// a request PM built for a new Meet call against Google's answer to it ([`answers`]); the same
/// conference (`conferenceId`); the same request (`createRequest.requestId`).
pub fn same_call(a: Option<&Value>, b: Option<&Value>) -> bool {
    let (a, b) = (a.filter(|v| !v.is_null()), b.filter(|v| !v.is_null()));
    let (a, b) = match (a, b) {
        (None, None) => return true,
        (Some(a), Some(b)) => (a, b),
        _ => return false,
    };
    if asked(a) {
        return answers(a, b);
    }
    if asked(b) {
        return answers(b, a);
    }
    if let (Some(x), Some(y)) = (text(a, "/conferenceId"), text(b, "/conferenceId")) {
        return x == y;
    }
    match (request_of(a), request_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Whether Google's `held` answers PM's request `asked`: any Meet call that isn't failed, or this
/// very request however it went. A live Meet call is sound because PM asks for a call only when the
/// event has none ([`meet_value`]): one there now is the one asked for, or one someone else added,
/// and either way the event holds what the user wanted. Its own request failed is still Google's
/// answer to that write, so the write landed, and the call's failure is said apart; sent again
/// under the same id, it would be ignored.
fn answers(asked: &Value, held: &Value) -> bool {
    live_meet(held) || request_of(asked).is_some_and(|id| request_of(held) == Some(id))
}

/// A string at `at` in `v`.
fn text(v: &Value, at: &str) -> Option<String> {
    v.pointer(at).and_then(Value::as_str).map(str::to_string)
}

/// The id of a call's request (`createRequest.requestId`).
fn request_of(data: &Value) -> Option<String> {
    text(data, "/createRequest/requestId")
}

/// Whether a body asks Google for a new call.
pub fn asks_for_meet(body: &Map<String, Value>) -> bool {
    body.get("conferenceData")
        .is_some_and(|c| c.get("createRequest").is_some())
}

/// PM's own `body` without the call it asks for: what Google's answer to a save is judged by, the
/// call's state being the wait's to say ([`asks_for_meet`]). A call's removal stays in.
pub fn without_meet_request(body: &Map<String, Value>) -> Map<String, Value> {
    body.iter()
        .filter(|(key, value)| {
            !(key.as_str() == "conferenceData" && value.get("createRequest").is_some())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Meet call as Google holds it: ready, with a phone entry point, Google's "more numbers" page
    /// (an https link, never the one to join by), and the video one.
    fn meet_ready() -> Value {
        json!({
            "conferenceId": "abc-defg-hij",
            "conferenceSolution": { "key": { "type": "hangoutsMeet" }, "name": "Google Meet" },
            "entryPoints": [
                { "entryPointType": "phone", "uri": "tel:+44-20-0000-0000", "pin": "123456" },
                { "entryPointType": "more", "uri": "https://tel.meet/abc-defg-hij?pin=123456" },
                { "entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij" }
            ]
        })
    }

    /// A Meet request Google is still working on (or has failed), under `id`.
    fn meet_asked(id: &str, status: &str) -> Value {
        json!({
            "createRequest": {
                "requestId": id,
                "conferenceSolutionKey": { "type": "hangoutsMeet" },
                "status": { "statusCode": status }
            }
        })
    }

    fn zoom() -> Value {
        json!({
            "conferenceId": "987",
            "conferenceSolution": { "key": { "type": "addOn" }, "name": "Zoom Meeting" },
            "entryPoints": [{ "entryPointType": "video", "uri": "https://zoom.us/j/987" }]
        })
    }

    fn event_with(data: Value) -> Value {
        json!({ "id": "e", "conferenceData": data })
    }

    #[test]
    fn call_of_reads_every_kind_and_state() {
        assert!(call_of(&json!({ "id": "e" })).is_none(), "no call");
        assert!(
            call_of(&json!({ "id": "e", "conferenceData": null })).is_none(),
            "a null call is none"
        );
        let link_only =
            call_of(&json!({ "hangoutLink": "https://hangouts.google.com/x" })).unwrap();
        assert!(
            link_only
                == CallForEdit {
                    kind: CallKind::Other,
                    name: String::new(),
                    join: Some("https://hangouts.google.com/x".into()),
                    state: CallState::Ready,
                },
            "a bare link is a call PM can't change"
        );
        let meet = call_of(&event_with(meet_ready())).unwrap();
        assert!(
            meet == CallForEdit {
                kind: CallKind::Meet,
                name: "Google Meet".into(),
                join: Some("https://meet.google.com/abc-defg-hij".into()),
                state: CallState::Ready,
            },
            "the video entry point, never the phone one"
        );
        let addon = call_of(&event_with(zoom())).unwrap();
        assert!(addon.kind == CallKind::AddOn && addon.name == "Zoom Meeting");
        let old = event_with(json!({
            "conferenceId": "h", "conferenceSolution": { "key": { "type": "eventHangout" } }
        }));
        assert_eq!(call_of(&old).unwrap().kind, CallKind::Other);
        let empty = event_with(json!({
            "conferenceId": "h", "conferenceSolution": { "key": { "type": "" } }
        }));
        assert_eq!(call_of(&empty).unwrap().kind, CallKind::Other);
        let pending = call_of(&event_with(meet_asked("r1", "pending"))).unwrap();
        assert!(
            pending.kind == CallKind::Meet
                && pending.state == CallState::Pending
                && pending.join.is_none(),
            "a request still being made: Meet, pending, no link yet"
        );
        let failed = call_of(&event_with(meet_asked("r1", "failure"))).unwrap();
        assert_eq!(failed.state, CallState::Failed);
        let plain_http = event_with(json!({
            "conferenceSolution": { "key": { "type": "hangoutsMeet" } },
            "entryPoints": [{ "entryPointType": "video", "uri": "http://x" }]
        }));
        assert_eq!(
            call_of(&plain_http).unwrap().join,
            None,
            "never an http link"
        );
        // With no video entry point, Google's `hangoutLink` when it's https.
        let mut linked = event_with(json!({
            "conferenceSolution": { "key": { "type": "hangoutsMeet" } }
        }));
        linked["hangoutLink"] = json!("https://meet.google.com/x");
        assert_eq!(
            call_of(&linked).unwrap().join.as_deref(),
            Some("https://meet.google.com/x")
        );
        let long = event_with(json!({
            "conferenceSolution": { "key": { "type": "addOn" }, "name": "z".repeat(300) }
        }));
        assert_eq!(call_of(&long).unwrap().name.chars().count(), 100);
        let lines = event_with(json!({
            "conferenceSolution": { "key": { "type": "addOn" }, "name": "Zoom\nMeeting" }
        }));
        assert_eq!(call_of(&lines).unwrap().name, "ZoomMeeting");
        // A request's own kind when there's no solution yet.
        let zoom_asked = event_with(json!({
            "createRequest": { "requestId": "r", "conferenceSolutionKey": { "type": "addOn" } }
        }));
        assert_eq!(call_of(&zoom_asked).unwrap().kind, CallKind::AddOn);
    }

    #[test]
    fn a_call_never_reads_pins_or_notes() {
        let event = event_with(json!({
            "conferenceId": "abc",
            "conferenceSolution": { "key": { "type": "hangoutsMeet" }, "name": "Google Meet" },
            "notes": "<b>the call's notes</b>",
            "entryPoints": [
                { "entryPointType": "phone", "uri": "tel:+1-555", "pin": "424242",
                  "passcode": "777777", "password": "868686", "accessCode": "31337",
                  "meetingCode": "99887766" },
                { "entryPointType": "video", "uri": "https://meet.google.com/abc", "pin": "555111" }
            ]
        }));
        let json = serde_json::to_string(&call_of(&event).unwrap()).unwrap();
        for held in [
            "424242", "777777", "868686", "31337", "99887766", "555111", "notes", "tel:",
        ] {
            assert!(
                !json.contains(held),
                "a PIN or the notes reached the editor"
            );
        }
    }

    #[test]
    fn a_link_is_meet_only_on_meets_own_address() {
        assert_eq!(
            kind_of_link("https://meet.google.com/abc-defg-hij"),
            CallKind::Meet
        );
        assert_eq!(kind_of_link("https://zoom.us/j/1"), CallKind::Other);
        assert_eq!(
            kind_of_link("https://meet.google.com.example.org/x"),
            CallKind::Other
        );
        assert_eq!(kind_of_link("http://meet.google.com/x"), CallKind::Other);
    }

    fn asking(meet: MeetChange, request: Option<&str>) -> EventPatchDraft {
        EventPatchDraft {
            meet: Some(meet),
            meet_request: request.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn meet_value_table() {
        let none = json!({ "id": "e" });
        let ready = event_with(meet_ready());
        let pending = event_with(meet_asked("r0", "pending"));
        let failed = event_with(meet_asked("r0", "failure"));
        let addon = event_with(zoom());
        let other = json!({ "id": "e", "hangoutLink": "https://hangouts.google.com/x" });
        let add = asking(MeetChange::Add, Some("r1"));
        let remove = asking(MeetChange::Remove, Some("r1"));
        let untouched = EventPatchDraft::default();
        let body = Some(meet_request_body("r1"));
        let rows: Vec<(&str, &Value, &EventPatchDraft, Option<Value>)> = vec![
            ("untouched", &ready, &untouched, None),
            ("add, none", &none, &add, body.clone()),
            ("add, failed", &failed, &add, body.clone()),
            ("add, ready", &ready, &add, None),
            ("add, pending", &pending, &add, None),
            ("add, add-on", &addon, &add, None),
            ("add, other", &other, &add, None),
            ("remove, ready", &ready, &remove, Some(Value::Null)),
            ("remove, pending", &pending, &remove, Some(Value::Null)),
            ("remove, none", &none, &remove, None),
            ("remove, failed", &failed, &remove, None),
            ("remove, add-on", &addon, &remove, None),
            ("remove, other", &other, &remove, None),
        ];
        for (row, fresh, draft, want) in rows {
            assert!(meet_value(fresh, draft) == Ok(want), "{row}");
        }
        assert!(
            meet_value(&none, &asking(MeetChange::Add, None)) == Err(PatchError::MeetUnready),
            "a call asked for with no request id"
        );
        // Nothing to ask for, so no id is needed.
        assert!(meet_value(&ready, &asking(MeetChange::Add, None)) == Ok(None));
        assert_eq!(
            meet_request_body("r1"),
            json!({ "createRequest": { "requestId": "r1",
                    "conferenceSolutionKey": { "type": "hangoutsMeet" } } })
        );
    }

    #[test]
    fn same_call_table() {
        let request = meet_request_body("mine");
        let ready = meet_ready();
        let pending_other = meet_asked("theirs", "pending");
        let failed_mine = meet_asked("mine", "failure");
        let rows: Vec<(&str, Option<Value>, Option<Value>, bool)> = vec![
            ("none and none", None, None, true),
            ("absent and null", None, Some(Value::Null), true),
            (
                "same conference",
                Some(ready.clone()),
                Some(ready.clone()),
                true,
            ),
            (
                "another conference",
                Some(ready.clone()),
                Some(json!({ "conferenceId": "x" })),
                false,
            ),
            (
                "PM's request, a ready Meet",
                Some(request.clone()),
                Some(ready.clone()),
                true,
            ),
            (
                "PM's request, a pending Meet under another id",
                Some(request.clone()),
                Some(pending_other.clone()),
                true,
            ),
            (
                "a ready Meet, PM's request",
                Some(ready.clone()),
                Some(request.clone()),
                true,
            ),
            // Google's answer to this very write: it landed, and the failure is said apart.
            (
                "PM's request, its own that failed",
                Some(request.clone()),
                Some(failed_mine.clone()),
                true,
            ),
            (
                "PM's request, another's that failed",
                Some(request.clone()),
                Some(meet_asked("theirs", "failure")),
                false,
            ),
            (
                "PM's request, an add-on",
                Some(request.clone()),
                Some(zoom()),
                false,
            ),
            ("PM's request, none", Some(request.clone()), None, false),
            (
                "two pending, one id",
                Some(meet_asked("a", "pending")),
                Some(meet_asked("a", "pending")),
                true,
            ),
            (
                "two pending, two ids",
                Some(meet_asked("a", "pending")),
                Some(meet_asked("b", "pending")),
                false,
            ),
            // Google filled in the call it was making.
            (
                "pending, then ready",
                Some(meet_asked("a", "pending")),
                Some(
                    json!({ "conferenceId": "abc", "createRequest": { "requestId": "a",
                              "status": { "statusCode": "success" } } }),
                ),
                true,
            ),
            ("a call, none", Some(ready.clone()), None, false),
        ];
        for (row, a, b, want) in rows {
            assert_eq!(same_call(a.as_ref(), b.as_ref()), want, "{row}");
        }
    }

    #[test]
    fn a_body_asking_for_a_call_is_judged_without_it() {
        let mut body = Map::new();
        body.insert("summary".into(), json!("Retro"));
        body.insert("conferenceData".into(), meet_request_body("r"));
        assert!(asks_for_meet(&body));
        let judged = without_meet_request(&body);
        assert!(!judged.contains_key("conferenceData") && judged.contains_key("summary"));
        // A removal is a change like any other, so it stays.
        let mut removing = Map::new();
        removing.insert("conferenceData".into(), Value::Null);
        assert!(!asks_for_meet(&removing));
        assert_eq!(without_meet_request(&removing), removing);
    }

    #[test]
    fn has_meet_and_room_for_meet_read_a_failed_request_as_none() {
        let none = json!({ "id": "e" });
        let ready = event_with(meet_ready());
        let pending = event_with(meet_asked("r", "pending"));
        let failed = event_with(meet_asked("r", "failure"));
        let addon = event_with(zoom());
        for (row, event, has, room) in [
            ("none", &none, false, true),
            ("ready", &ready, true, false),
            ("pending", &pending, true, false),
            ("failed", &failed, false, true),
            ("add-on", &addon, false, false),
        ] {
            assert_eq!(has_meet(event), has, "{row}: has");
            assert_eq!(room_for_meet(event), room, "{row}: room");
        }
    }
}
