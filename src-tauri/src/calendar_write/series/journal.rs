// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What PM keeps of a "this and following" change while it runs, in the settings store, so a quit or a
//! crash mid-change leaves a record (C9 design review H3). Google's ids, the plan's note and the
//! original's rule before and after the cut, never a title or a description. After a restart it
//! decides nothing by itself: Google's copies are read again, and only what they prove counts.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::calendar_recur::lines::{recurrence_verdict, Applied};
use crate::calendar_recur::Point;

use super::plan::{is_tail_of, start_of};
use crate::calendar_write::patch::is_gone;

/// `settings` key: the [`Journal`] as JSON. Absent when no change is running or left over.
pub const SETTINGS_KEY: &str = "calendar_split_journal";

/// One "this and following" change, from just before its first write until it's settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Google's calendar id.
    pub calendar: String,
    /// The original series (its master's id), and the new part PM inserts.
    pub series: String,
    pub tail: String,
    /// The new part's `pmSplitPlan` note.
    pub plan: String,
    /// The split's day, `YYYY-MM-DD` in the series' zone, for the words.
    pub from: String,
    /// The occurrence the new part starts at ([`point_text`]), and the zone the series repeats in:
    /// which occurrences of the original the change reaches.
    pub at: String,
    pub zone: String,
    /// The original's `start` node, which places its rule's dates.
    pub start: Value,
    /// The original's rule before the change, and as the cut leaves it: Google holding either, by
    /// meaning, is the only proof of which.
    pub before: Vec<String>,
    pub cut: Vec<String>,
    /// The change also moves or cancels occurrences changed on their own after the split.
    pub cleanup: bool,
}

impl Entry {
    /// [`Entry::at`] read back, `None` when PM can't.
    pub fn point(&self) -> Option<Point> {
        if let Ok(day) = NaiveDate::parse_from_str(&self.at, "%Y-%m-%d") {
            return Some(Point::Day(day));
        }
        DateTime::parse_from_rfc3339(&self.at)
            .ok()
            .map(|t| Point::At(t.with_timezone(&Utc)))
    }

    pub fn tz(&self) -> Option<Tz> {
        self.zone.parse().ok()
    }
}

/// A point as an [`Entry`] keeps it: a day as `YYYY-MM-DD`, an instant as RFC 3339 in UTC.
pub fn point_text(point: Point) -> String {
    match point {
        Point::Day(day) => day.format("%Y-%m-%d").to_string(),
        Point::At(t) => t.to_rfc3339(),
    }
}

/// Entries by [`key`].
pub type Journal = BTreeMap<String, Entry>;

/// An entry's key: its calendar and original series.
pub fn key(calendar: &str, series: &str) -> String {
    format!("{calendar}\u{1f}{series}")
}

/// The stored journal. A value PM can't read reads as empty: PM wrote it, and every decision reads
/// Google again anyway; the backstop on overlapping parts doesn't need it.
pub fn read(stored: Option<&str>) -> Journal {
    stored
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

/// The journal to store, `None` when it's empty and the key goes.
pub fn write(journal: &Journal) -> Option<String> {
    if journal.is_empty() {
        return None;
    }
    serde_json::to_string(journal).ok()
}

/// One series as a GET found it.
#[derive(Debug, Clone, Copy)]
pub enum Look<'a> {
    /// 404 or 410.
    Gone,
    Copy(&'a Value),
    /// Not read: offline, a sign-in, a reply PM couldn't read.
    Unread,
}

/// What Google's copies say of an entry no split in memory stands for: one left from before PM
/// restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orphan {
    /// The new part isn't there, or isn't this plan's, or the original is gone: nothing can show
    /// twice. The entry goes.
    Clear,
    /// The original holds the cut rule, so the change stands, but the occurrences it would have moved
    /// may not have moved. Said once, then the entry goes.
    CutLanded,
    /// The original holds a rule that is neither: changed in Google since. PM can't tell how the change
    /// ended, so it says so once and the entry goes; the overlapping-parts backstop then guards the
    /// series.
    Changed,
    /// The original still holds its rule as it was before the change, and the new part is there: it
    /// shows twice. Only then may the new part be deleted from PM, since the original still holds
    /// those days.
    Twice,
    /// PM couldn't read one of them.
    Unknown,
}

/// Read an entry against Google's copies of its new part and its original (C9a-3b critique C1-H1:
/// the rules by what they mean, never by their text).
pub fn orphan(entry: &Entry, tail: Look<'_>, original: Look<'_>) -> Orphan {
    match tail {
        Look::Unread => return Orphan::Unknown,
        Look::Gone => return Orphan::Clear,
        Look::Copy(copy) if is_gone(copy) || !is_tail_of(copy, &entry.series, &entry.plan) => {
            return Orphan::Clear
        }
        Look::Copy(_) => {}
    }
    let original = match original {
        Look::Unread => return Orphan::Unknown,
        Look::Gone => return Orphan::Clear,
        Look::Copy(copy) if is_gone(copy) => return Orphan::Clear,
        Look::Copy(copy) => copy,
    };
    let Some(start) = start_of(&entry.start) else {
        return Orphan::Changed;
    };
    let now: Vec<&str> = original
        .get("recurrence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match recurrence_verdict(&entry.cut, &entry.before, &now, &start) {
        Applied::Yes if entry.cleanup => Orphan::CutLanded,
        Applied::Yes => Orphan::Clear,
        Applied::No => Orphan::Twice,
        Applied::Unknown => Orphan::Changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> Entry {
        Entry {
            calendar: "me@example.com".into(),
            series: "abc".into(),
            tail: "pmtail".into(),
            plan: "digest".into(),
            from: "2026-10-19".into(),
            at: "2026-10-19T08:00:00+00:00".into(),
            zone: "Europe/London".into(),
            start: json!({ "dateTime": "2026-10-05T09:00:00+01:00", "timeZone": "Europe/London" }),
            before: vec!["RRULE:FREQ=WEEKLY;BYDAY=MO".into()],
            cut: vec!["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261018T225959Z".into()],
            cleanup: true,
        }
    }

    fn tail() -> Value {
        json!({
            "id": "pmtail",
            "status": "confirmed",
            "extendedProperties": { "private": { "pmSplitOf": "abc", "pmSplitPlan": "digest" } },
        })
    }

    fn original(rule: &[&str]) -> Value {
        json!({ "id": "abc", "status": "confirmed", "recurrence": rule })
    }

    #[test]
    fn a_cut_or_an_uncut_rule_is_read_by_what_it_means() {
        let e = entry();
        let t = tail();
        let read = |rule: &[&str]| orphan(&e, Look::Copy(&t), Look::Copy(&original(rule)));
        assert_eq!(read(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]), Orphan::Twice);
        // Google writing the same rules out another way changes nothing.
        assert_eq!(
            read(&["RRULE:BYDAY=MO;INTERVAL=1;FREQ=WEEKLY"]),
            Orphan::Twice
        );
        assert_eq!(
            read(&["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261018T225959Z"]),
            Orphan::CutLanded
        );
        // The same rule with its parts in another order.
        assert_eq!(
            read(&["RRULE:UNTIL=20261018T225959Z;BYDAY=MO;FREQ=WEEKLY;WKST=MO"]),
            Orphan::CutLanded
        );
        // Changed in Google since: neither, so never "twice" and never a delete.
        assert_eq!(read(&["RRULE:FREQ=WEEKLY;BYDAY=TU"]), Orphan::Changed);
        assert_eq!(
            read(&["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261101T000000Z"]),
            Orphan::Changed
        );
        assert_eq!(read(&[]), Orphan::Changed);
    }

    #[test]
    fn a_landed_cut_with_nothing_to_move_needs_no_words() {
        let e = Entry {
            cleanup: false,
            ..entry()
        };
        let cut = original(&["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261018T225959Z"]);
        assert_eq!(
            orphan(&e, Look::Copy(&tail()), Look::Copy(&cut)),
            Orphan::Clear
        );
    }

    #[test]
    fn nothing_there_to_show_twice_clears_and_what_isnt_read_is_unknown() {
        let e = entry();
        let uncut = original(&["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        let mut other_plan = tail();
        other_plan["extendedProperties"]["private"]["pmSplitPlan"] = json!("another");
        let mut cancelled = tail();
        cancelled["status"] = json!("cancelled");
        let bare = json!({ "id": "pmtail", "status": "confirmed" });
        for t in [&other_plan, &cancelled, &bare] {
            assert_eq!(orphan(&e, Look::Copy(t), Look::Copy(&uncut)), Orphan::Clear);
        }
        assert_eq!(orphan(&e, Look::Gone, Look::Unread), Orphan::Clear);
        // The original deleted in Google: the new part is the only copy, never "twice".
        let mut gone = uncut.clone();
        gone["status"] = json!("cancelled");
        assert_eq!(
            orphan(&e, Look::Copy(&tail()), Look::Copy(&gone)),
            Orphan::Clear
        );
        assert_eq!(orphan(&e, Look::Copy(&tail()), Look::Gone), Orphan::Clear);
        assert_eq!(
            orphan(&e, Look::Unread, Look::Copy(&uncut)),
            Orphan::Unknown
        );
        assert_eq!(
            orphan(&e, Look::Copy(&tail()), Look::Unread),
            Orphan::Unknown
        );
        // A start PM can't read proves nothing either way.
        let no_start = Entry {
            start: json!({}),
            ..entry()
        };
        assert_eq!(
            orphan(&no_start, Look::Copy(&tail()), Look::Copy(&uncut)),
            Orphan::Changed
        );
    }

    #[test]
    fn the_journal_round_trips_and_an_empty_one_is_no_key() {
        let mut journal = Journal::new();
        assert_eq!(write(&journal), None);
        journal.insert(key("me@example.com", "abc"), entry());
        let stored = write(&journal).unwrap();
        assert_eq!(read(Some(&stored)), journal);
        assert!(read(Some("not json")).is_empty());
        assert!(read(None).is_empty());
        assert_ne!(key("a", "bc"), key("ab", "c"));
    }

    #[test]
    fn an_entry_reads_its_split_point_back() {
        let e = entry();
        let at = e.point().unwrap();
        assert_eq!(point_text(at), "2026-10-19T08:00:00+00:00");
        let day = Entry {
            at: "2026-10-19".into(),
            ..entry()
        };
        assert_eq!(point_text(day.point().unwrap()), "2026-10-19");
        assert_eq!(e.tz(), Some(chrono_tz::Europe::London));
        let bad = Entry {
            at: "soon".into(),
            zone: "Nowhere/Else".into(),
            ..entry()
        };
        assert!(bad.point().is_none() && bad.tz().is_none());
    }
}
