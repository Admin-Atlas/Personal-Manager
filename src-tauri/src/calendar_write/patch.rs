// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The body of a save, and the rules for an event that changed in Google while the editor was open.
//!
//! Every PATCH is built against a fresh `events.get` of the event (rule R1), never the mirror, which
//! clips titles and descriptions on purpose. It carries only the fields the user touched whose value
//! differs from what Google holds now; start and end always travel together. PM never uses
//! `events.update` (a PUT), because an update that omits a field removes it: a label, an attachment,
//! a field PM doesn't know about yet.
//!
//! A changed event (its etag moved) is handled before sending (rule R2):
//! - an update goes ahead only when none of the fields it sends changed in Google
//!   ([`update_check`]), and is otherwise a conflict, never an overwrite; a deleted event is gone;
//! - a delete sends no fields, so it has its own rule: it goes ahead only when what the user saw
//!   still matches the fresh copy ([`delete_still_matches`]). Re-deleting just because the etag moved
//!   would delete a version of the event nobody looked at.

use serde_json::{Map, Value};

use super::dto::{EventPatchDraft, SeenSummary};
use super::time::{self, TimeError};

/// The only fields a single-event save sends.
pub const PATCHABLE: &[&str] = &[
    "summary",
    "location",
    "description",
    "start",
    "end",
    "transparency",
    "visibility",
];

/// The PATCH body for `draft` against `fresh`, Google's current copy. Empty means Google already
/// holds exactly this, and nothing should be sent.
///
/// Switching between a timed and an all-day event sends the other key as `null` so Google drops it
/// rather than holding both (INFERRED: confirmed or refuted by live test L6).
pub fn build_patch(
    fresh: &Value,
    draft: &EventPatchDraft,
) -> Result<Map<String, Value>, TimeError> {
    let mut body = Map::new();
    let mut text = |key: &str, new: &Option<String>| {
        if let Some(new) = new {
            if text_of(fresh, key) != new.as_str() {
                body.insert(key.to_string(), Value::String(new.clone()));
            }
        }
    };
    text("summary", &draft.summary);
    text("location", &draft.location);
    text("description", &draft.description);
    if let Some(show_as) = draft.show_as {
        if transparency_of(fresh) != show_as.as_transparency() {
            body.insert(
                "transparency".into(),
                Value::String(show_as.as_transparency().into()),
            );
        }
    }
    if let Some(visibility) = draft.visibility {
        if visibility_of(fresh) != visibility.as_param() {
            body.insert(
                "visibility".into(),
                Value::String(visibility.as_param().into()),
            );
        }
    }
    if let Some(draft_time) = &draft.time {
        // Against Google's current start and end, so the half the user didn't change is sent exactly
        // as Google holds it (time::resolve_against).
        let when = time::resolve_against(draft_time, fresh.get("start"), fresh.get("end"))?;
        let unchanged = when_eq(fresh.get("start"), Some(&when.start))
            && when_eq(fresh.get("end"), Some(&when.end));
        if !unchanged {
            let was_all_day = fresh.get("start").is_some_and(|s| s.get("date").is_some());
            let (mut start, mut end) = (when.start, when.end);
            if was_all_day != when.all_day {
                let dropped: &[&str] = if when.all_day {
                    &["dateTime", "timeZone"]
                } else {
                    &["date"]
                };
                for node in [&mut start, &mut end] {
                    if let Some(obj) = node.as_object_mut() {
                        for key in dropped {
                            obj.insert((*key).to_string(), Value::Null);
                        }
                    }
                }
            }
            body.insert("start".into(), start);
            body.insert("end".into(), end);
        }
    }
    Ok(body)
}

/// Whether Google's copy is a deleted event (`get` still returns those, details and all, so they can
/// be restored). Nothing may be saved onto it, and it can't be deleted again.
pub fn is_gone(fresh: &Value) -> bool {
    fresh.get("status").and_then(Value::as_str) == Some("cancelled")
}

/// The answer to "may this update still go ahead?" after Google's copy turned out to be newer than
/// the one the editor opened on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// Nothing the save sends changed in Google: send it on the fresh etag.
    Proceed,
    /// Deleted in Google meanwhile.
    Gone,
    /// Google changed these fields, which the save also sends. Nothing is overwritten.
    Conflict(Vec<String>),
}

/// Decide an update against `base` (the copy the editor opened on) and `fresh` (Google's copy now).
/// Fields the save doesn't send never count: a location changed in Google doesn't stop a title edit,
/// and both changes are kept.
pub fn update_check(base: &Value, fresh: &Value, body: &Map<String, Value>) -> UpdateCheck {
    if is_gone(fresh) {
        return UpdateCheck::Gone;
    }
    let changed: Vec<String> = body
        .keys()
        .filter(|key| !field_eq(key, base.get(key.as_str()), fresh.get(key.as_str())))
        .cloned()
        .collect();
    if changed.is_empty() {
        UpdateCheck::Proceed
    } else {
        UpdateCheck::Conflict(changed)
    }
}

/// Whether `fresh` already holds everything `body` would set: after a save whose answer never
/// arrived (a dropped connection), this tells "it landed" from "send it again". A deleted copy never
/// counts as landed, whatever its details still say.
pub fn already_applied(fresh: &Value, body: &Map<String, Value>) -> bool {
    !is_gone(fresh)
        && body
            .iter()
            .all(|(key, value)| field_eq(key, Some(value), fresh.get(key.as_str())))
}

/// The answer to "may this delete still go ahead?" after Google's copy turned out to be newer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteCheck {
    /// What the user saw is still what's there (only the etag moved, say a reminder): delete it.
    Matches,
    /// Already gone (deleted, or cancelled from its series).
    Gone,
    /// It changed in ways the user didn't see: these fields. Nothing is deleted.
    Changed(Vec<String>),
}

/// Compare what the user saw (`seen`, from the mirror row they clicked) with Google's fresh copy, in
/// the mirror's own terms so like is compared with like.
pub fn delete_still_matches(seen: &SeenSummary, fresh: &Value) -> DeleteCheck {
    if is_gone(fresh) {
        return DeleteCheck::Gone;
    }
    // `parse_event` normalises start and end exactly as the mirror stores them; a calendar id is
    // needed for its row id but plays no part in the comparison.
    let Some(now) = crate::calendar::parse_event("", fresh) else {
        // Not a single event PM could mirror (no start, or a series master): don't delete blind.
        return DeleteCheck::Changed(vec!["start".into()]);
    };
    let mut changed = Vec::new();
    if crate::calendar::mirrored_summary(&now.summary) != seen.summary {
        changed.push("summary".to_string());
    }
    if now.start != seen.start || now.all_day != seen.all_day {
        changed.push("start".to_string());
    }
    if now.end != seen.end {
        changed.push("end".to_string());
    }
    let location = now
        .location
        .as_deref()
        .map(crate::calendar::mirrored_location);
    if location.as_deref().unwrap_or("") != seen.location.as_deref().unwrap_or("") {
        changed.push("location".to_string());
    }
    if changed.is_empty() {
        DeleteCheck::Matches
    } else {
        DeleteCheck::Changed(changed)
    }
}

/// Whether a description is formatted rather than plain text. Editing it in a plain-text box would
/// strip the formatting, so a formatted one is read-only until the rich-text layer (C19). Errs
/// towards "formatted": a plain description that merely looks like markup is only made read-only.
pub fn description_is_html(s: &str) -> bool {
    for (i, c) in s.char_indices() {
        let rest = &s[i + c.len_utf8()..];
        match c {
            // A tag: `<b>`, `</p>`, `<a href=…>`, `<br/>`.
            '<' => {
                let name = rest.strip_prefix('/').unwrap_or(rest);
                if name.starts_with(|n: char| n.is_ascii_alphabetic()) && name.contains('>') {
                    return true;
                }
            }
            // An entity: `&amp;`, `&nbsp;`, `&#39;`.
            '&' => {
                if let Some(end) = rest.find(';').filter(|e| (1..=8).contains(e)) {
                    let entity = &rest[..end];
                    let numeric = entity
                        .strip_prefix('#')
                        .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
                    if numeric || entity.chars().all(|c| c.is_ascii_alphabetic()) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

// --- comparing one field the way Google means it ---

/// A text field, with absent read as empty (Google omits empty ones).
fn text_of<'a>(event: &'a Value, key: &str) -> &'a str {
    event.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Absent `transparency` is Google's default, `opaque` (busy).
fn transparency_of(event: &Value) -> &str {
    event
        .get("transparency")
        .and_then(Value::as_str)
        .unwrap_or("opaque")
}

/// Absent `visibility` is Google's default, `default`.
fn visibility_of(event: &Value) -> &str {
    event
        .get("visibility")
        .and_then(Value::as_str)
        .unwrap_or("default")
}

/// Whether one patchable field has the same value in `a` and `b`, each a field value as it appears
/// in an event (or in a PATCH body), `None` when absent.
fn field_eq(key: &str, a: Option<&Value>, b: Option<&Value>) -> bool {
    match key {
        "start" | "end" => when_eq(a, b),
        "transparency" => {
            a.and_then(Value::as_str).unwrap_or("opaque")
                == b.and_then(Value::as_str).unwrap_or("opaque")
        }
        "visibility" => {
            a.and_then(Value::as_str).unwrap_or("default")
                == b.and_then(Value::as_str).unwrap_or("default")
        }
        _ => a.and_then(Value::as_str).unwrap_or("") == b.and_then(Value::as_str).unwrap_or(""),
    }
}

/// Whether two start (or end) nodes name the same time: the same date for all-day, or the same
/// instant in the same zone for a timed one (offsets may be written differently). A `null` key, as a
/// kind-switching PATCH sends, counts as absent.
fn when_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let key = |node: Option<&Value>, k: &str| -> Option<String> {
        node.and_then(|n| n.get(k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let (a_date, b_date) = (key(a, "date"), key(b, "date"));
    let (a_dt, b_dt) = (key(a, "dateTime"), key(b, "dateTime"));
    match (a_date, b_date, a_dt, b_dt) {
        (Some(x), Some(y), None, None) => x == y,
        (None, None, Some(x), Some(y)) => {
            let instant = |s: &str| chrono::DateTime::parse_from_rfc3339(s.trim()).ok();
            match (instant(&x), instant(&y)) {
                (Some(i), Some(j)) => i == j && key(a, "timeZone") == key(b, "timeZone"),
                _ => x == y,
            }
        }
        (None, None, None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_write::dto::{ShowAs, TimeDraft, Visibility};
    use serde_json::json;

    /// Google's fresh copy of a one-off event, with fields PM must never touch.
    fn fresh() -> Value {
        json!({
            "id": "ev1",
            "etag": "\"100\"",
            "status": "confirmed",
            "summary": "Dentist",
            "location": "High St",
            "start": { "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London" },
            "end": { "dateTime": "2026-10-12T11:00:00+01:00", "timeZone": "Europe/London" },
            "iCalUID": "ev1@google.com",
            "sequence": 2,
            "eventType": "default",
            "organizer": { "email": "me@example.com", "self": true },
            "creator": { "email": "me@example.com" },
            "attachments": [{ "fileUrl": "https://drive.google.com/x" }],
            "extendedProperties": { "private": { "k": "v" } },
            "eventLabelId": "label-1"
        })
    }

    fn timed(sd: &str, st: &str, ed: &str, et: &str) -> TimeDraft {
        TimeDraft::Timed {
            start_date: sd.into(),
            start_time: st.into(),
            start_zone: "Europe/London".into(),
            end_date: ed.into(),
            end_time: et.into(),
            end_zone: "Europe/London".into(),
        }
    }

    #[test]
    fn only_touched_fields_that_differ_are_sent() {
        let draft = EventPatchDraft {
            summary: Some("Dentist (moved)".into()),
            // Touched but unchanged: not sent.
            location: Some("High St".into()),
            show_as: Some(ShowAs::Busy),
            visibility: Some(Visibility::Default),
            ..Default::default()
        };
        let body = build_patch(&fresh(), &draft).unwrap();
        assert_eq!(Value::Object(body), json!({ "summary": "Dentist (moved)" }));
        // Nothing touched, or everything already so: an empty body, and nothing is sent.
        assert!(build_patch(&fresh(), &EventPatchDraft::default())
            .unwrap()
            .is_empty());
    }

    /// Google's fields stay Google's: nothing outside the patchable set is ever in a body, however the
    /// draft is built.
    #[test]
    fn nothing_but_the_patchable_fields_is_ever_sent() {
        let draft = EventPatchDraft {
            summary: Some("x".into()),
            location: Some("".into()),
            description: Some("notes".into()),
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-13".into(),
                last_day: "2026-10-13".into(),
            }),
            show_as: Some(ShowAs::Free),
            visibility: Some(Visibility::Private),
        };
        let body = build_patch(&fresh(), &draft).unwrap();
        for key in body.keys() {
            assert!(PATCHABLE.contains(&key.as_str()), "{key}");
        }
        for never in [
            "eventType",
            "recurringEventId",
            "originalStartTime",
            "iCalUID",
            "sequence",
            "organizer",
            "creator",
            "attachments",
            "extendedProperties",
            "eventLabelId",
            "id",
            "etag",
        ] {
            assert!(!body.contains_key(never), "{never}");
        }
        assert_eq!(body["transparency"], "transparent");
        assert_eq!(body["visibility"], "private");
        // Clearing the location sends an empty string.
        assert_eq!(body["location"], "");
    }

    #[test]
    fn start_and_end_always_travel_together() {
        // Only the end moves; both are sent.
        let body = build_patch(
            &fresh(),
            &EventPatchDraft {
                time: Some(timed("2026-10-12", "10:00", "2026-10-12", "11:30")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(body["end"]["dateTime"], "2026-10-12T11:30:00+01:00");
        // The same instants written with another offset are no change.
        let mut utc = fresh();
        utc["start"]["dateTime"] = "2026-10-12T09:00:00Z".into();
        utc["end"]["dateTime"] = "2026-10-12T10:00:00Z".into();
        let same = build_patch(
            &utc,
            &EventPatchDraft {
                time: Some(timed("2026-10-12", "10:00", "2026-10-12", "11:00")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(same.is_empty(), "{same:?}");
    }

    /// INFERRED until L6: a kind switch nulls the key the event no longer uses.
    #[test]
    fn switching_between_timed_and_all_day_drops_the_old_key() {
        let to_all_day = build_patch(
            &fresh(),
            &EventPatchDraft {
                time: Some(TimeDraft::AllDay {
                    first_day: "2026-10-12".into(),
                    last_day: "2026-10-12".into(),
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            to_all_day["start"],
            json!({ "date": "2026-10-12", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            to_all_day["end"],
            json!({ "date": "2026-10-13", "dateTime": null, "timeZone": null })
        );

        let mut all_day = fresh();
        all_day["start"] = json!({ "date": "2026-10-12" });
        all_day["end"] = json!({ "date": "2026-10-13" });
        let to_timed = build_patch(
            &all_day,
            &EventPatchDraft {
                time: Some(timed("2026-10-12", "10:00", "2026-10-12", "11:00")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(to_timed["start"]["date"], Value::Null);
        assert_eq!(to_timed["start"]["timeZone"], "Europe/London");
        // And a body that switched counts as applied once Google holds the new kind.
        let mut landed = all_day.clone();
        landed["start"] =
            json!({ "dateTime": "2026-10-12T10:00:00+01:00", "timeZone": "Europe/London" });
        landed["end"] =
            json!({ "dateTime": "2026-10-12T11:00:00+01:00", "timeZone": "Europe/London" });
        assert!(already_applied(&landed, &to_timed));
    }

    #[test]
    fn a_bad_time_is_refused_before_anything_is_built() {
        let err = build_patch(
            &fresh(),
            &EventPatchDraft {
                summary: Some("x".into()),
                time: Some(timed("2026-03-29", "01:30", "2026-03-29", "03:00")),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, TimeError::Gap { .. }));
    }

    /// An end-only change to an event that starts at the second 01:30 of London's fall-back: the
    /// start goes out exactly as Google holds it, never an hour earlier.
    #[test]
    fn the_untouched_half_is_sent_as_google_holds_it() {
        let mut folded = fresh();
        folded["start"] =
            json!({ "dateTime": "2026-10-25T01:30:00Z", "timeZone": "Europe/London" });
        folded["end"] = json!({ "dateTime": "2026-10-25T02:30:00Z", "timeZone": "Europe/London" });
        let body = build_patch(
            &folded,
            &EventPatchDraft {
                time: Some(timed("2026-10-25", "01:30", "2026-10-25", "03:00")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(body["start"], folded["start"]);
        assert_eq!(body["end"]["dateTime"], "2026-10-25T03:00:00+00:00");
        // And with nothing moved, nothing is sent at all.
        let unchanged = build_patch(
            &folded,
            &EventPatchDraft {
                time: Some(timed("2026-10-25", "01:30", "2026-10-25", "02:30")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(unchanged.is_empty(), "{unchanged:?}");
    }

    /// R2 for an update: a field Google changed that the save also sends is a conflict; one it
    /// doesn't send isn't, and both changes are kept.
    #[test]
    fn an_update_conflicts_only_on_fields_it_sends() {
        let base = fresh();
        let body = build_patch(
            &base,
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let mut location_moved = base.clone();
        location_moved["location"] = "Low St".into();
        location_moved["etag"] = "\"101\"".into();
        assert_eq!(
            update_check(&base, &location_moved, &body),
            UpdateCheck::Proceed
        );

        let mut retitled = base.clone();
        retitled["summary"] = "Dentist (cancelled?)".into();
        assert_eq!(
            update_check(&base, &retitled, &body),
            UpdateCheck::Conflict(vec!["summary".into()])
        );

        // Only the etag moved (a reminder changed): go ahead.
        let mut etag_only = base.clone();
        etag_only["etag"] = "\"102\"".into();
        assert_eq!(update_check(&base, &etag_only, &body), UpdateCheck::Proceed);
    }

    /// R2: a fresh `status: cancelled` is gone, for a save as for a delete, even though `get` still
    /// returns all its details (so it can be restored) and they match.
    #[test]
    fn an_event_deleted_meanwhile_is_gone_not_saved_onto() {
        let base = fresh();
        let body = build_patch(
            &base,
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let mut deleted = base.clone();
        deleted["status"] = "cancelled".into();
        deleted["etag"] = "\"103\"".into();
        assert_eq!(update_check(&base, &deleted, &body), UpdateCheck::Gone);
        deleted["summary"] = "Dentist (moved)".into();
        assert!(
            !already_applied(&deleted, &body),
            "a deleted copy never counts as landed"
        );
    }

    #[test]
    fn a_landed_save_is_recognised_after_a_lost_answer() {
        let body = build_patch(
            &fresh(),
            &EventPatchDraft {
                summary: Some("Dentist (moved)".into()),
                show_as: Some(ShowAs::Free),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!already_applied(&fresh(), &body));
        let mut landed = fresh();
        landed["summary"] = "Dentist (moved)".into();
        landed["transparency"] = "transparent".into();
        assert!(already_applied(&landed, &body));
    }

    fn seen() -> SeenSummary {
        SeenSummary {
            summary: "Dentist".into(),
            start: "2026-10-12T09:00:00Z".into(),
            end: Some("2026-10-12T10:00:00Z".into()),
            all_day: false,
            location: Some("High St".into()),
        }
    }

    /// R2 for a delete: only the etag moving (a reminder, a colour) doesn't stop it; anything the
    /// user saw changing does.
    #[test]
    fn a_delete_goes_ahead_only_on_what_the_user_saw() {
        let mut etag_only = fresh();
        etag_only["etag"] = "\"200\"".into();
        assert_eq!(
            delete_still_matches(&seen(), &etag_only),
            DeleteCheck::Matches
        );

        let mut moved = fresh();
        moved["start"]["dateTime"] = "2026-10-13T10:00:00+01:00".into();
        moved["end"]["dateTime"] = "2026-10-13T11:00:00+01:00".into();
        assert_eq!(
            delete_still_matches(&seen(), &moved),
            DeleteCheck::Changed(vec!["start".into(), "end".into()])
        );

        let mut renamed = fresh();
        renamed["summary"] = "Dentist\nreschedule".into();
        assert_eq!(
            delete_still_matches(&seen(), &renamed),
            DeleteCheck::Changed(vec!["summary".into()])
        );

        let mut gone = fresh();
        gone["status"] = "cancelled".into();
        assert_eq!(delete_still_matches(&seen(), &gone), DeleteCheck::Gone);
    }

    #[test]
    fn a_delete_compares_in_the_mirrors_own_terms() {
        // The mirror clips a title to 300 characters; the fresh copy's full title still matches.
        let long = "x".repeat(400);
        let mut fresh_long = fresh();
        fresh_long["summary"] = long.clone().into();
        let seen_long = SeenSummary {
            summary: long.chars().take(300).collect(),
            ..seen()
        };
        assert_eq!(
            delete_still_matches(&seen_long, &fresh_long),
            DeleteCheck::Matches
        );
        // An empty location and an absent one are the same.
        let mut no_location = fresh();
        no_location.as_object_mut().unwrap().remove("location");
        let seen_none = SeenSummary {
            location: None,
            ..seen()
        };
        assert_eq!(
            delete_still_matches(&seen_none, &no_location),
            DeleteCheck::Matches
        );
    }

    #[test]
    fn formatted_descriptions_are_recognised() {
        for html in [
            "<b>Agenda</b>",
            "line one<br>line two",
            "see <a href=\"https://x\">this</a>",
            "</p>",
            "fish &amp; chips",
            "it&#39;s",
            "a&nbsp;b",
        ] {
            assert!(description_is_html(html), "{html}");
        }
        for plain in [
            "a < b and c > d",
            "Q&A at 3",
            "5<6",
            "just notes",
            "",
            "café ☕ <",
        ] {
            assert!(!description_is_html(plain), "{plain}");
        }
    }
}
