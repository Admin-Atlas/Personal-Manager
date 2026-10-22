// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Replying to an invitation: the body that changes only the user's own reply, whether Google holds
//! it, and a note as Google keeps it. Nothing here fetches or sends (C14).
//!
//! The reply lives in the user's own entry on the event's guest list, the one Google marks `self`:
//! the calendar this copy is on, which for a calendar shared with you isn't you. A guest's reply goes
//! as that entry alone under `attendeesOmitted`, which Google reads as "only update the
//! participant's response" (events reference). The organiser's own reply goes as the whole list
//! Google sent with only their entry changed, since a one-entry list read as the whole list would
//! take every other guest off.
//!
//! CodeQL guardrail: Google's list is never edited in place (the whole-list form is built with
//! `map` over clones, the user's entry changed on a clone with PM's own keys), and no message names
//! an address: [`RsvpError`]'s words name none.

use std::fmt;

use serde_json::{Map, Value};

use super::attendees;
use super::dto::{ReplyForEdit, Rsvp};

/// Longest note PM sends, in characters, counted after tidying (Google states no limit; INFERRED,
/// probe L-I1-3). The reply control counts the same way.
pub const NOTE_MAX: usize = 1000;

/// Why a reply can't be built. Shown as is; never names an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsvpError {
    /// The copy's calendar isn't on its guest list.
    NotInvited,
    NoteTooLong,
}

impl fmt::Display for RsvpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            // A group invitation lists the group, not this calendar (C14 critique C2-M8).
            RsvpError::NotInvited => {
                "PM can only reply where this calendar is on the guest list itself (not through a \
                 group)."
            }
            RsvpError::NoteTooLong => {
                "A note can be up to 1,000 characters, so your reply wasn't sent."
            }
        })
    }
}

/// An address as PM compares it: trimmed, whatever its case.
fn same_address(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// An entry's address, when it has one.
fn address_of(entry: &Map<String, Value>) -> Option<&str> {
    entry
        .get("email")
        .and_then(Value::as_str)
        .filter(|a| !a.trim().is_empty())
}

/// The calendar's own entry on `event`'s guest list: an entry with Google's `self: true` and an
/// address. `calendar` is Google's id of the calendar the copy is on (a main calendar's id is its
/// account's address). When two entries are marked (an invitation sent to two addresses that both
/// reach this calendar, as a plus-address alias might; INFERRED, L-I0), the one whose address is
/// `calendar`, ignoring case, else the first.
pub fn self_entry<'a>(event: &'a Value, calendar: &str) -> Option<(usize, &'a Map<String, Value>)> {
    let marked: Vec<(usize, &Map<String, Value>)> = event
        .get("attendees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, e)| Some((i, e.as_object()?)))
        .filter(|(_, e)| e.get("self").and_then(Value::as_bool) == Some(true))
        .filter(|(_, e)| address_of(e).is_some())
        .collect();
    marked
        .iter()
        .find(|(_, e)| address_of(e).is_some_and(|a| same_address(a, calendar)))
        .or_else(|| marked.first())
        .copied()
}

/// Whether the calendar this copy is on has a reply to give: any entry marked `self` with an
/// address, the organiser's own included.
pub fn invited(event: &Value) -> bool {
    self_entry(event, "").is_some()
}

/// A note as Google keeps it: `\r\n` and `\r` as `\n`, other control characters except `\n` and
/// `\t` dropped, the ends trimmed. `Err(NoteTooLong)` past [`NOTE_MAX`] characters (counted after
/// tidying).
pub fn tidy_note(text: &str) -> Result<String, RsvpError> {
    let lines = text.replace("\r\n", "\n").replace('\r', "\n");
    let kept: String = lines
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let tidy = kept.trim();
    if tidy.chars().count() > NOTE_MAX {
        return Err(RsvpError::NoteTooLong);
    }
    Ok(tidy.to_string())
}

/// Google's `organizer.self`: the calendar this copy is on organises the event, so a reply from it
/// is the organiser's own.
pub fn organised_here(event: &Value) -> bool {
    event
        .get("organizer")
        .and_then(|o| o.get("self"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// The PATCH body that sets the user's reply to `response`, and their note to `note` when it's given
/// (`Some("")` removes it; `None` leaves it as Google holds it), on `event`, Google's copy now.
///
/// - A guest: `{"attendeesOmitted": true, "attendees": [entry]}`, which Google reads as "only update
///   the participant's response" (events reference). `entry` is the user's own entry exactly as
///   Google sent it, every key kept (name, optional, extra guests, `id`, `self`), with
///   `responseStatus` and, when given, `comment` set.
/// - The organiser replying to their own event, when Google sent the whole list (no
///   `attendeesOmitted`): `{"attendees": [every entry as Google sent it, their own changed]}`. Never a
///   one-entry list: if Google read it as the whole list, every guest would be taken off. With
///   `attendeesOmitted` on the organiser's copy, the guest form above.
///
/// `Err(NotInvited)` when the copy has no own entry.
pub fn rsvp_body(
    event: &Value,
    calendar: &str,
    response: Rsvp,
    note: Option<&str>,
) -> Result<Map<String, Value>, RsvpError> {
    let (own, entry) = self_entry(event, calendar).ok_or(RsvpError::NotInvited)?;
    let mut mine = entry.clone();
    mine.insert(
        "responseStatus".into(),
        Value::String(response.as_google().into()),
    );
    if let Some(note) = note {
        mine.insert("comment".into(), Value::String(note.into()));
    }
    let mut body = Map::new();
    let whole = organised_here(event) && !attendees::omitted(event);
    match event.get("attendees").and_then(Value::as_array) {
        Some(list) if whole => {
            let all: Vec<Value> = list
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    if i == own {
                        Value::Object(mine.clone())
                    } else {
                        e.clone()
                    }
                })
                .collect();
            body.insert("attendees".into(), Value::Array(all));
        }
        _ => {
            body.insert("attendeesOmitted".into(), Value::Bool(true));
            body.insert("attendees".into(), Value::Array(vec![Value::Object(mine)]));
        }
    }
    Ok(body)
}

/// The user's own reply on `event`, as Google says it, or `None` without an own entry or a reply.
pub fn own_response<'a>(event: &'a Value, calendar: &str) -> Option<&'a str> {
    self_entry(event, calendar)?
        .1
        .get("responseStatus")
        .and_then(Value::as_str)
}

/// Whether `event` already holds this reply, and this note when one is given (an absent note and ""
/// are the same). Only the user's own entry is read: someone else's reply or a guest added meanwhile
/// doesn't matter to it.
pub fn rsvp_applied(event: &Value, calendar: &str, response: Rsvp, note: Option<&str>) -> bool {
    let Some((_, entry)) = self_entry(event, calendar) else {
        return false;
    };
    let replied = entry.get("responseStatus").and_then(Value::as_str) == Some(response.as_google());
    let noted =
        note.is_none_or(|note| entry.get("comment").and_then(Value::as_str).unwrap_or("") == note);
    replied && noted
}

/// The user's own reply as the editor shows it, or `None` without an own entry.
pub fn reply_for_edit(event: &Value, calendar: &str) -> Option<ReplyForEdit> {
    let (_, entry) = self_entry(event, calendar)?;
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
    Some(ReplyForEdit {
        response: text("responseStatus"),
        note: text("comment").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The user, awaiting a reply.
    fn me() -> Value {
        json!({ "email": "me@x.org", "self": true, "responseStatus": "needsAction" })
    }

    /// The organiser, on the list.
    fn organiser() -> Value {
        json!({ "email": "o@x.org", "organizer": true, "responseStatus": "accepted" })
    }

    /// Another guest, with a note and someone with them.
    fn other() -> Value {
        json!({
            "email": "a@x.org", "responseStatus": "accepted", "comment": "late",
            "additionalGuests": 1
        })
    }

    /// An invitation from o@x.org whose guest list is `list`.
    fn invitation(list: Vec<Value>) -> Value {
        json!({ "id": "e1", "organizer": { "email": "o@x.org" }, "attendees": list })
    }

    fn with(mut entry: Value, key: &str, value: Value) -> Value {
        entry[key] = value;
        entry
    }

    /// The guest form with `entry` alone.
    fn guest_form(entry: Value) -> Map<String, Value> {
        json!({ "attendeesOmitted": true, "attendees": [entry] })
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn rsvp_body_table() {
        let me_x = || with(me(), "comment", json!("x"));
        let alias = || json!({ "email": "Me+pm@X.org", "self": true });
        let mine = || {
            json!({
                "email": "me@x.org", "self": true, "responseStatus": "needsAction",
                "displayName": "Me", "optional": true, "additionalGuests": 2, "id": "att-1"
            })
        };
        let organised = |list: Vec<Value>| json!({ "id": "e1", "organizer": { "email": "me@x.org", "self": true }, "attendees": list });
        let me_organising = || with(me(), "organizer", json!(true));
        // The row's name, the copy, its calendar, the reply and note, and the body it gives.
        type Row<'a> = (
            &'a str,
            Value,
            &'a str,
            Rsvp,
            Option<&'a str>,
            Result<Map<String, Value>, RsvpError>,
        );
        let rows: Vec<Row> = vec![
            (
                "B1",
                invitation(vec![organiser(), me(), other()]),
                "me@x.org",
                Rsvp::Yes,
                None,
                Ok(guest_form(with(me(), "responseStatus", json!("accepted")))),
            ),
            (
                "B2",
                invitation(vec![organiser(), me(), other()]),
                "me@x.org",
                Rsvp::Maybe,
                Some("running late"),
                Ok(guest_form(with(
                    with(me(), "responseStatus", json!("tentative")),
                    "comment",
                    json!("running late"),
                ))),
            ),
            (
                "B3",
                invitation(vec![organiser(), me_x(), other()]),
                "me@x.org",
                Rsvp::No,
                Some(""),
                Ok(guest_form(with(
                    with(me(), "responseStatus", json!("declined")),
                    "comment",
                    json!(""),
                ))),
            ),
            (
                "B4",
                invitation(vec![organiser(), me_x(), other()]),
                "me@x.org",
                Rsvp::Yes,
                None,
                Ok(guest_form(with(
                    me_x(),
                    "responseStatus",
                    json!("accepted"),
                ))),
            ),
            (
                "B5",
                invitation(vec![organiser(), mine()]),
                "me@x.org",
                Rsvp::Yes,
                None,
                Ok(guest_form(with(
                    mine(),
                    "responseStatus",
                    json!("accepted"),
                ))),
            ),
            (
                "B6",
                invitation(vec![organiser(), alias(), other()]),
                "me@x.org",
                Rsvp::Yes,
                None,
                Ok(guest_form(with(
                    alias(),
                    "responseStatus",
                    json!("accepted"),
                ))),
            ),
            (
                "B8",
                invitation(vec![organiser(), other()]),
                "me@x.org",
                Rsvp::Yes,
                None,
                Err(RsvpError::NotInvited),
            ),
            (
                "B9",
                invitation(vec![organiser(), json!({ "self": true }), other()]),
                "me@x.org",
                Rsvp::Yes,
                None,
                Err(RsvpError::NotInvited),
            ),
            (
                "B10",
                with(invitation(vec![me()]), "attendeesOmitted", json!(true)),
                "me@x.org",
                Rsvp::No,
                None,
                Ok(guest_form(with(me(), "responseStatus", json!("declined")))),
            ),
            (
                "B11",
                organised(vec![me_organising(), other()]),
                "me@x.org",
                Rsvp::No,
                None,
                Ok(json!({ "attendees": [
                    with(me_organising(), "responseStatus", json!("declined")),
                    other()
                ] })
                .as_object()
                .unwrap()
                .clone()),
            ),
            (
                "B12",
                with(
                    organised(vec![me_organising()]),
                    "attendeesOmitted",
                    json!(true),
                ),
                "me@x.org",
                Rsvp::Maybe,
                None,
                Ok(guest_form(with(
                    me_organising(),
                    "responseStatus",
                    json!("tentative"),
                ))),
            ),
            (
                "B13",
                json!({ "id": "e1", "organizer": { "email": "o@x.org" } }),
                "me@x.org",
                Rsvp::Yes,
                None,
                Err(RsvpError::NotInvited),
            ),
            (
                "B14",
                invitation(vec![organiser(), alias(), me(), other()]),
                "ME@x.org",
                Rsvp::Yes,
                None,
                Ok(guest_form(with(me(), "responseStatus", json!("accepted")))),
            ),
            (
                "B15",
                invitation(vec![organiser(), alias(), me(), other()]),
                "other@x.org",
                Rsvp::Yes,
                None,
                Ok(guest_form(with(
                    alias(),
                    "responseStatus",
                    json!("accepted"),
                ))),
            ),
        ];
        for (row, event, calendar, response, note, want) in rows {
            let got = rsvp_body(&event, calendar, response, note);
            assert!(got == want, "{row}: not the body the table gives");
        }
        // B6: the entry's address goes exactly as Google sent it.
        let b6 = rsvp_body(
            &invitation(vec![organiser(), alias(), other()]),
            "me@x.org",
            Rsvp::Yes,
            None,
        )
        .unwrap();
        assert_eq!(b6["attendees"][0]["email"], "Me+pm@X.org");
        // B1, B4: no note given, no `comment` key added.
        let b1 = rsvp_body(
            &invitation(vec![organiser(), me(), other()]),
            "me@x.org",
            Rsvp::Yes,
            None,
        )
        .unwrap();
        assert!(b1["attendees"][0].get("comment").is_none());
    }

    /// B7: whatever the reply, nobody else's entry is in a guest's body.
    #[test]
    fn a_guests_reply_carries_no_one_elses_entry() {
        let event = invitation(vec![organiser(), me(), other()]);
        for response in [Rsvp::Yes, Rsvp::No, Rsvp::Maybe] {
            for note in [None, Some(""), Some("n")] {
                let body = rsvp_body(&event, "me@x.org", response, note).unwrap();
                let list = body["attendees"].as_array().unwrap();
                assert_eq!(list.len(), 1);
                assert_eq!(list[0]["email"], "me@x.org");
                assert_eq!(body["attendeesOmitted"], true);
                assert_eq!(body.len(), 2, "nothing but the two keys");
            }
        }
    }

    #[test]
    fn a_reply_is_applied_when_the_own_entry_says_so() {
        let before = invitation(vec![organiser(), me(), other()]);
        let after = invitation(vec![
            organiser(),
            with(me(), "responseStatus", json!("accepted")),
            other(),
        ]);
        assert!(rsvp_applied(&after, "me@x.org", Rsvp::Yes, None));
        assert!(!rsvp_applied(&before, "me@x.org", Rsvp::Yes, None));
        assert!(!rsvp_applied(&after, "me@x.org", Rsvp::No, None));
        // No note asked about: whatever note Google holds.
        let noted = invitation(vec![with(
            with(me(), "responseStatus", json!("accepted")),
            "comment",
            json!("b"),
        )]);
        assert!(rsvp_applied(&noted, "me@x.org", Rsvp::Yes, None));
        assert!(!rsvp_applied(&noted, "me@x.org", Rsvp::Yes, Some("a")));
        assert!(rsvp_applied(&noted, "me@x.org", Rsvp::Yes, Some("b")));
        // An empty note and none at all are the same.
        assert!(rsvp_applied(&after, "me@x.org", Rsvp::Yes, Some("")));
        // Someone else's reply changing meanwhile doesn't matter.
        let moved = invitation(vec![
            organiser(),
            with(me(), "responseStatus", json!("accepted")),
            with(other(), "responseStatus", json!("declined")),
        ]);
        assert!(rsvp_applied(&moved, "me@x.org", Rsvp::Yes, None));
        // No own entry: never applied.
        assert!(!rsvp_applied(
            &invitation(vec![
                organiser(),
                with(other(), "responseStatus", json!("accepted"))
            ]),
            "me@x.org",
            Rsvp::Yes,
            None
        ));
        assert_eq!(own_response(&after, "me@x.org"), Some("accepted"));
        assert_eq!(own_response(&invitation(vec![other()]), "me@x.org"), None);
    }

    #[test]
    fn a_note_is_tidied_as_google_keeps_it() {
        assert_eq!(tidy_note("a\r\nb\rc"), Ok("a\nb\nc".to_string()));
        // The bell dropped, the tab trimmed at the end; tabs and new lines inside kept.
        assert_eq!(tidy_note("\u{7}x\t"), Ok("x".to_string()));
        assert_eq!(tidy_note(" a\tb\n\nc "), Ok("a\tb\n\nc".to_string()));
        assert_eq!(tidy_note(""), Ok(String::new()));
        // Counted in characters after tidying: a thousand two-byte letters pass, one more doesn't.
        let most = "é".repeat(NOTE_MAX);
        assert_eq!(tidy_note(&most), Ok(most.clone()));
        assert_eq!(tidy_note(&format!("{most}é")), Err(RsvpError::NoteTooLong));
        // Characters, never bytes: a thousand and one plain letters are too many, a thousand
        // three-byte signs aren't.
        assert_eq!(
            tidy_note(&"a".repeat(NOTE_MAX + 1)),
            Err(RsvpError::NoteTooLong)
        );
        assert!(tidy_note(&"€".repeat(NOTE_MAX)).is_ok());
        // Space trimmed off the ends before counting.
        assert_eq!(tidy_note(&format!("  {most}\r\n")), Ok(most));
        assert_eq!(
            RsvpError::NoteTooLong.to_string(),
            "A note can be up to 1,000 characters, so your reply wasn't sent."
        );
    }

    #[test]
    fn the_editor_reads_the_own_reply() {
        assert!(
            reply_for_edit(&invitation(vec![organiser(), me()]), "me@x.org")
                == Some(ReplyForEdit {
                    response: Some("needsAction".into()),
                    note: String::new(),
                })
        );
        let noted = invitation(vec![with(me(), "comment", json!("late"))]);
        assert_eq!(
            reply_for_edit(&noted, "me@x.org").map(|r| r.note),
            Some("late".to_string())
        );
        // B14's copy: the calendar's own address over the alias.
        let both = invitation(vec![
            json!({ "email": "Me+pm@X.org", "self": true, "responseStatus": "declined" }),
            with(me(), "responseStatus", json!("accepted")),
        ]);
        assert_eq!(
            reply_for_edit(&both, "ME@x.org").and_then(|r| r.response),
            Some("accepted".to_string())
        );
        assert!(reply_for_edit(&invitation(vec![organiser(), other()]), "me@x.org").is_none());
    }

    /// Any entry marked `self` with an address is a reply to give, the organiser's own included.
    #[test]
    fn a_copy_is_invited_when_its_calendar_is_on_the_list() {
        assert!(invited(&invitation(vec![organiser(), me()])));
        assert!(invited(&invitation(vec![with(
            me(),
            "organizer",
            json!(true)
        )])));
        assert!(!invited(&invitation(vec![organiser(), other()])));
        assert!(!invited(&invitation(vec![
            json!({ "self": true, "email": " " })
        ])));
        assert!(!invited(&json!({ "id": "e1" })));
    }
}
