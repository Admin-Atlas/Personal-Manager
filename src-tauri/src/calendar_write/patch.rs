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

use std::fmt;

use serde_json::{Map, Value};

use super::dto::{EventPatchDraft, NewEventDraft, SeenSummary, TimeDraft, Visibility};
use super::time::{self, TimeError};
use crate::calendar_recur::expand::sync_start;
use crate::calendar_recur::spec::{rule_from_spec, SpecError};
use crate::calendar_recur::wire::RepeatRule;
use crate::calendar_recur::SeriesStart;

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

/// What a create may send: [`PATCHABLE`]'s fields and the repeat rule (C11). Still no attendees,
/// reminders, conference, colour or private notes until their layers. Widening it is a decision,
/// never a slip: `creatable_is_patchable_and_the_rule` pins it to exactly this, and the create
/// plan's own refusal test names what it still refuses (C10 critique 1 L10).
pub const CREATABLE: &[&str] = &[
    "summary",
    "location",
    "description",
    "start",
    "end",
    "transparency",
    "visibility",
    "recurrence",
];

/// Why a create's body can't be built. Shown to the user as is: the inner words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertError {
    Time(TimeError),
    Rule(SpecError),
}

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertError::Time(e) => e.fmt(f),
            InsertError::Rule(e) => e.fmt(f),
        }
    }
}

impl From<TimeError> for InsertError {
    fn from(e: TimeError) -> Self {
        InsertError::Time(e)
    }
}

impl From<SpecError> for InsertError {
    fn from(e: SpecError) -> Self {
        InsertError::Rule(e)
    }
}

/// The insert body for `draft`. Google's `start`/`end` come from [`time::resolve`] (explicit zones,
/// a skipped hour refused, a repeated hour's first occurrence, an all-day end the day after the
/// last day); empty text is left out, so Google keeps no empty strings. No `id`: the insert request
/// writes PM's own in ([`super::plan::insert_event`]).
///
/// A repeating event starts on the first day its rule makes ([`sync_start`]: "Weekly on Monday and
/// Wednesday" chosen on a Tuesday starts on the Wednesday, A27), its end moved the same number of
/// days, and carries one rule line and nothing else (no EXDATE or RDATE). A timed one names its
/// zone on both nodes, as every timed node does; an all-day one sends dates only (probe P-R1).
pub fn insert_body(draft: &NewEventDraft) -> Result<Map<String, Value>, InsertError> {
    let (time, rule) = match &draft.recurrence {
        None => (draft.time.clone(), None),
        Some(rule) => {
            let spec = rule.to_spec()?;
            let asked = time::series_start(&draft.time)?;
            let synced = sync_start(&spec, &asked)?;
            let moved = (synced.date() - asked.date()).num_days();
            (
                time::shifted(&draft.time, moved)?,
                Some(rule_from_spec(&spec, &synced)?),
            )
        }
    };
    let when = time::resolve(&time)?;
    let mut body = Map::new();
    for (key, text) in [
        ("summary", &draft.summary),
        ("location", &draft.location),
        ("description", &draft.description),
    ] {
        // As typed, untrimmed, as a save sends it; only text that is all blank is left out.
        if !text.trim().is_empty() {
            body.insert(key.into(), Value::String(text.clone()));
        }
    }
    body.insert("start".into(), when.start);
    body.insert("end".into(), when.end);
    if let Some(show_as) = draft.show_as {
        body.insert(
            "transparency".into(),
            Value::String(show_as.as_transparency().into()),
        );
    }
    if let Some(visibility) = draft.visibility.filter(|v| *v != Visibility::Default) {
        body.insert(
            "visibility".into(),
            Value::String(visibility.as_param().into()),
        );
    }
    if let Some(line) = rule {
        body.insert("recurrence".into(), Value::from(vec![line]));
    }
    Ok(body)
}

/// Make a one-off repeat by `rule` (C11 §10): its `start`, `end` and `recurrence` into `body`, and
/// the start it repeats from. The times are the draft's (`time`), or the event's own as the editor
/// showed them ([`time::time_draft_of`]: each node's zone, else the calendar's, `fallback_zone`;
/// never UTC, R6), moved to the first day the rule makes ([`sync_start`]) with their length kept, as
/// a new repeating event's are. Both nodes are always sent, a timed one naming its `timeZone` (G6):
/// Google's own node, which a save hands back for a half the user left alone, may have none. A
/// switch between timed and all-day sends the other kind's keys as `null`, as any save does.
pub fn add_repeat(
    body: &mut Map<String, Value>,
    fresh: &Value,
    time: Option<&TimeDraft>,
    rule: &RepeatRule,
    fallback_zone: Option<&str>,
) -> Result<SeriesStart, InsertError> {
    let own;
    let time = match time {
        Some(time) => time,
        // The editor opened on these very nodes, so only a node with no zone, on a calendar with
        // none, can't be read here.
        None => {
            own =
                time::time_draft_of(fresh, fallback_zone.unwrap_or("")).ok_or(TimeError::NoZone)?;
            &own
        }
    };
    let spec = rule.to_spec()?;
    let asked = time::series_start(time)?;
    let synced = sync_start(&spec, &asked)?;
    let moved = (synced.date() - asked.date()).num_days();
    let when = time::resolve(&time::shifted(time, moved)?)?;
    let line = rule_from_spec(&spec, &synced)?;
    let (mut start, mut end) = (when.start, when.end);
    let was_all_day = fresh.get("start").is_some_and(|s| s.get("date").is_some());
    if was_all_day != when.all_day {
        for node in [&mut start, &mut end] {
            drop_other_kind(node);
        }
    }
    body.insert("start".into(), start);
    body.insert("end".into(), end);
    body.insert("recurrence".into(), Value::from(vec![line]));
    Ok(synced)
}

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
                for node in [&mut start, &mut end] {
                    drop_other_kind(node);
                }
            }
            body.insert("start".into(), start);
            body.insert("end".into(), end);
        }
    }
    Ok(body)
}

/// A start or end node sent onto an event of the other kind (timed onto all-day, or back), with the
/// other kind's keys set to null: `events.patch` merges nested objects, so a key left out would stay.
pub(crate) fn drop_other_kind(node: &mut Value) {
    let all_day = node.get("date").is_some_and(|d| !d.is_null());
    let dropped: &[&str] = if all_day {
        &["dateTime", "timeZone"]
    } else {
        &["date"]
    };
    if let Some(obj) = node.as_object_mut() {
        for key in dropped {
            obj.insert((*key).to_string(), Value::Null);
        }
    }
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
        // A list or an object (`recurrence`, `reminders`, `attendees`…) by value: read as text, any two
        // would be "" and equal, so a changed one would never be a conflict (G4, found by C8's survey;
        // no field PM sends today is one).
        _ if [a, b]
            .iter()
            .any(|v| v.is_some_and(|v| v.is_array() || v.is_object())) =>
        {
            a.filter(|v| !v.is_null()) == b.filter(|v| !v.is_null())
        }
        _ => a.and_then(Value::as_str).unwrap_or("") == b.and_then(Value::as_str).unwrap_or(""),
    }
}

/// Whether two start (or end) nodes name the same time: the same date for all-day, or the same
/// instant in the same zone for a timed one (offsets may be written differently). A `null` key, as a
/// kind-switching PATCH sends, counts as absent.
pub(crate) fn when_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
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
    use crate::calendar_recur::wire::{DayCode, RepeatEnds, RepeatPattern, RepeatRule};
    use crate::calendar_write::dto::{ShowAs, TimeDraft, Visibility};
    use serde_json::json;

    fn day(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

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
            "colorId": "5",
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
            // A repeat is never a one-event body's: the series planners write it (C11).
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
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
            // An event's own colour stays as Google holds it: a PATCH that leaves a field out keeps
            // it (events.patch), so a title save never clears it (live test L-C1).
            "colorId",
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

    /// C8b review R7: a list or an object is compared by value, a null as absent; never as text,
    /// which would read any two as equal.
    #[test]
    fn a_list_or_an_object_conflicts_by_value() {
        let base = fresh();
        let body: Map<String, Value> = [(
            "reminders".to_string(),
            serde_json::json!({ "useDefault": true }),
        )]
        .into_iter()
        .collect();
        let mut own = base.clone();
        own["reminders"] = serde_json::json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": 5 }] });
        assert_eq!(
            update_check(&base, &own, &body),
            UpdateCheck::Conflict(vec!["reminders".into()])
        );
        let mut nulled = base.clone();
        nulled["reminders"] = Value::Null;
        let mut absent = base.clone();
        if let Some(obj) = absent.as_object_mut() {
            obj.retain(|k, _| k != "reminders");
        }
        assert_eq!(update_check(&nulled, &absent, &body), UpdateCheck::Proceed);
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

    // --- creating an event (C10) ---

    fn new_event(time: TimeDraft) -> NewEventDraft {
        NewEventDraft {
            summary: "Dentist".into(),
            time,
            location: "High St".into(),
            description: "Bring the form\nand a pen".into(),
            show_as: Some(ShowAs::Free),
            visibility: Some(Visibility::Private),
            recurrence: None,
        }
    }

    /// `new_event` repeating by `pattern`, every time, until `ends`.
    fn repeating(time: TimeDraft, pattern: RepeatPattern, ends: RepeatEnds) -> NewEventDraft {
        NewEventDraft {
            recurrence: Some(RepeatRule {
                pattern,
                interval: 1,
                ends,
            }),
            ..new_event(time)
        }
    }

    fn weekly_on(days: &[DayCode]) -> RepeatPattern {
        RepeatPattern::Weekly {
            days: days.to_vec(),
        }
    }

    /// Every field a create may send, the rule included, and nothing else.
    #[test]
    fn insert_body_sends_only_what_a_create_may() {
        let body = insert_body(&repeating(
            timed("2026-10-26", "10:00", "2026-10-26", "11:00"),
            weekly_on(&[DayCode::Mon]),
            RepeatEnds::After { count: 10 },
        ))
        .unwrap();
        let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "description",
                "end",
                "location",
                "recurrence",
                "start",
                "summary",
                "transparency",
                "visibility"
            ]
        );
        assert!(keys.iter().all(|k| CREATABLE.contains(k)));
        assert_eq!(body["summary"], "Dentist");
        assert_eq!(body["description"], "Bring the form\nand a pen");
        assert_eq!(body["transparency"], "transparent");
        assert_eq!(body["visibility"], "private");
        assert_eq!(
            body["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"])
        );
        assert!(!body.contains_key("id"), "the insert request writes the id");
        // No rule, no `recurrence` at all.
        let once = insert_body(&new_event(timed(
            "2026-10-26",
            "10:00",
            "2026-10-26",
            "11:00",
        )))
        .unwrap();
        assert!(!once.contains_key("recurrence"));
    }

    /// C11 §3.3: what a create may send is a single save's fields and the rule, exactly.
    #[test]
    fn creatable_is_patchable_and_the_rule() {
        let mut expected: Vec<&str> = PATCHABLE.to_vec();
        expected.push("recurrence");
        assert_eq!(CREATABLE, expected.as_slice());
    }

    /// G6: a weekly event on its own weekday sends one rule line, and both nodes name the zone.
    #[test]
    fn a_repeating_insert_sends_one_rule_and_names_its_zone() {
        let body = insert_body(&repeating(
            timed("2026-10-13", "09:00", "2026-10-13", "10:00"),
            weekly_on(&[DayCode::Tue]),
            RepeatEnds::Never,
        ))
        .unwrap();
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]));
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-13T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(body["end"]["timeZone"], "Europe/London");
    }

    /// A27 (live test L-R10's case): Monday and Wednesday chosen on a Tuesday starts on the
    /// Wednesday, and the hour's length goes with it.
    #[test]
    fn a_repeating_insert_starts_on_the_first_day_its_rule_makes() {
        let body = insert_body(&repeating(
            timed("2026-10-13", "09:00", "2026-10-13", "10:00"),
            weekly_on(&[DayCode::Wed, DayCode::Mon]),
            RepeatEnds::OnDate {
                date: "2026-12-31".into(),
            },
        ))
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-14T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            body["end"],
            json!({ "dateTime": "2026-10-14T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        // The end date's last second in London, as UTC (GMT by then).
        assert_eq!(
            body["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261231T235959Z"])
        );
    }

    /// An all-day repeating event names days only (probe P-R1): no zone on either node.
    #[test]
    fn an_all_day_repeating_insert_sends_dates_only() {
        let body = insert_body(&repeating(
            TimeDraft::AllDay {
                first_day: "2026-10-26".into(),
                last_day: "2026-10-26".into(),
            },
            RepeatPattern::Daily,
            RepeatEnds::Never,
        ))
        .unwrap();
        assert_eq!(body["start"], json!({ "date": "2026-10-26" }));
        assert_eq!(body["end"], json!({ "date": "2026-10-27" }));
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=DAILY"]));
    }

    /// London skips 01:00-02:00 on Sunday 29-03-2026. A Sunday rule chosen at 01:30 on the Saturday
    /// would start in it; an end the move puts in it is refused as any end in a gap is.
    #[test]
    fn a_repeating_insert_in_a_gap_is_refused() {
        let gap = insert_body(&repeating(
            timed("2026-03-28", "01:30", "2026-03-28", "03:00"),
            weekly_on(&[DayCode::Sun]),
            RepeatEnds::Never,
        ));
        assert!(gap == Err(InsertError::Rule(SpecError::Gap(day(2026, 3, 29)))));
        assert_eq!(
            gap.unwrap_err().to_string(),
            "The first occurrence would be on 29-03-2026, when the clocks skip that time. Pick a \
             time outside that hour."
        );
        let end_gap = insert_body(&repeating(
            timed("2026-03-27", "23:00", "2026-03-28", "01:30"),
            weekly_on(&[DayCode::Sat]),
            RepeatEnds::Never,
        ));
        assert!(matches!(
            end_gap,
            Err(InsertError::Time(TimeError::Gap { .. }))
        ));
    }

    /// Google allows 730 occurrences (Help 37115): 731 builds nothing to send.
    #[test]
    fn a_rule_past_730_builds_nothing() {
        let draft = |count: u16| {
            repeating(
                timed("2026-10-26", "10:00", "2026-10-26", "11:00"),
                RepeatPattern::Daily,
                RepeatEnds::After { count },
            )
        };
        assert!(insert_body(&draft(731)) == Err(InsertError::Rule(SpecError::TooMany)));
        assert_eq!(
            insert_body(&draft(730)).unwrap()["recurrence"],
            json!(["RRULE:FREQ=DAILY;COUNT=730"])
        );
        // An end date PM can't read, or a time it can't, is said in its own words.
        let unread = NewEventDraft {
            recurrence: Some(RepeatRule {
                pattern: RepeatPattern::Daily,
                interval: 1,
                ends: RepeatEnds::OnDate {
                    date: "31-12-2026".into(),
                },
            }),
            ..draft(1)
        };
        assert!(insert_body(&unread) == Err(InsertError::Rule(SpecError::BadDate)));
        let bad_time = repeating(
            timed("2026-10-26", "late", "2026-10-26", "11:00"),
            RepeatPattern::Daily,
            RepeatEnds::Never,
        );
        assert!(
            insert_body(&bad_time) == Err(InsertError::Time(TimeError::BadTime("late".into())))
        );
    }

    fn weekly_rule(days: &[DayCode]) -> RepeatRule {
        RepeatRule {
            pattern: RepeatPattern::Weekly {
                days: days.to_vec(),
            },
            interval: 1,
            ends: RepeatEnds::Never,
        }
    }

    /// C11 §10: a one-off given a repeat, its time left alone, starts on the first day the rule
    /// makes at its own wall time and length, both nodes naming their zone even where Google's own
    /// had none; it repeats from there.
    #[test]
    fn a_repeat_given_to_a_one_off_starts_on_the_rules_first_day_with_its_zone() {
        let mut bare = fresh();
        bare["start"] = json!({ "dateTime": "2026-10-12T10:00:00+01:00" });
        bare["end"] = json!({ "dateTime": "2026-10-12T11:30:00+01:00" });
        let mut body = build_patch(&bare, &EventPatchDraft::default()).unwrap();
        let start = add_repeat(
            &mut body,
            &bare,
            None,
            &weekly_rule(&[DayCode::Wed]),
            Some("Europe/London"),
        )
        .unwrap();
        assert_eq!(start.date(), day(2026, 10, 14));
        assert_eq!(
            Value::Object(body),
            json!({
                "start": { "dateTime": "2026-10-14T10:00:00+01:00", "timeZone": "Europe/London" },
                "end": { "dateTime": "2026-10-14T11:30:00+01:00", "timeZone": "Europe/London" },
                "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=WE"],
            })
        );
        // A node with no zone, on a calendar with none, is never read as UTC (R6).
        let mut body = Map::new();
        let refused = add_repeat(&mut body, &bare, None, &weekly_rule(&[DayCode::Wed]), None);
        assert!(refused == Err(InsertError::Time(TimeError::NoZone)));
        assert!(body.is_empty(), "nothing built");
    }

    /// The draft's own time wins, and a switch to all day clears the timed keys, as any save's does.
    #[test]
    fn a_repeat_given_with_a_switch_to_all_day_clears_the_other_kind() {
        let all_day = TimeDraft::AllDay {
            first_day: "2026-10-12".into(),
            last_day: "2026-10-12".into(),
        };
        let draft = EventPatchDraft {
            time: Some(all_day.clone()),
            ..Default::default()
        };
        let mut body = build_patch(&fresh(), &draft).unwrap();
        add_repeat(
            &mut body,
            &fresh(),
            Some(&all_day),
            &weekly_rule(&[DayCode::Mon]),
            Some("Europe/London"),
        )
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "date": "2026-10-12", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            body["end"],
            json!({ "date": "2026-10-13", "dateTime": null, "timeZone": null })
        );
        assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"]));
        // A rule outside the limits builds nothing.
        let mut body = Map::new();
        let too_many = RepeatRule {
            ends: RepeatEnds::After { count: 731 },
            ..weekly_rule(&[DayCode::Mon])
        };
        assert!(
            add_repeat(&mut body, &fresh(), None, &too_many, Some("Europe/London"))
                == Err(InsertError::Rule(SpecError::TooMany))
        );
        assert!(body.is_empty());
    }

    /// Blank text is left out, so Google stores no empty strings; text with words in is sent as
    /// typed, spaces and all.
    #[test]
    fn an_empty_title_and_empty_text_are_left_out() {
        let draft = NewEventDraft {
            summary: "   ".into(),
            location: String::new(),
            description: "\n ".into(),
            ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
        };
        let body = insert_body(&draft).unwrap();
        for key in ["summary", "location", "description"] {
            assert!(!body.contains_key(key), "{key}");
        }
        let spaced = NewEventDraft {
            summary: " Dentist ".into(),
            ..draft
        };
        assert_eq!(insert_body(&spaced).unwrap()["summary"], " Dentist ");
    }

    /// A flight: each half carries its own zone and offset.
    #[test]
    fn a_timed_insert_names_its_zones() {
        let body = insert_body(&new_event(TimeDraft::Timed {
            start_date: "2026-10-26".into(),
            start_time: "10:00".into(),
            start_zone: "Europe/London".into(),
            end_date: "2026-10-26".into(),
            end_time: "13:00".into(),
            end_zone: "America/New_York".into(),
        }))
        .unwrap();
        assert_eq!(
            body["start"],
            json!({ "dateTime": "2026-10-26T10:00:00+00:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            body["end"],
            json!({ "dateTime": "2026-10-26T13:00:00-04:00", "timeZone": "America/New_York" })
        );
    }

    #[test]
    fn an_all_day_insert_ends_the_day_after() {
        let body = insert_body(&new_event(TimeDraft::AllDay {
            first_day: "2026-10-26".into(),
            last_day: "2026-10-26".into(),
        }))
        .unwrap();
        assert_eq!(body["start"], json!({ "date": "2026-10-26" }));
        assert_eq!(body["end"], json!({ "date": "2026-10-27" }));
    }

    #[test]
    fn a_gap_or_an_end_before_start_builds_nothing() {
        let gap = insert_body(&new_event(timed(
            "2026-03-29",
            "01:30",
            "2026-03-29",
            "03:00",
        )));
        assert!(matches!(gap, Err(InsertError::Time(TimeError::Gap { .. }))));
        for end in ["10:00", "09:00"] {
            let backwards =
                insert_body(&new_event(timed("2026-10-26", "10:00", "2026-10-26", end)));
            assert!(
                matches!(backwards, Err(InsertError::Time(TimeError::EndBeforeStart))),
                "{end}"
            );
        }
    }

    #[test]
    fn default_visibility_and_no_show_as_are_left_out() {
        for visibility in [None, Some(Visibility::Default)] {
            let body = insert_body(&NewEventDraft {
                show_as: None,
                visibility,
                ..new_event(timed("2026-10-26", "10:00", "2026-10-26", "11:00"))
            })
            .unwrap();
            assert!(!body.contains_key("transparency"));
            assert!(!body.contains_key("visibility"), "{visibility:?}");
        }
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
