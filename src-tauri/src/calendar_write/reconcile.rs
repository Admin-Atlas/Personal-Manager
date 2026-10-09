// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keeping a just-saved event on screen until a sync shows it (rule R4).
//!
//! A save writes Google's reply into the mirror at once. But a background sync that started before
//! the save can finish after it, holding the event as it was, and would put the old version back. So
//! every save leaves an entry here for ten minutes, and each sync merges its fetch through them:
//! - the fetch shows the version PM saved (the same etag): it landed, and the entry goes;
//! - Google's `updated` for the event is strictly later than the saved version's: someone changed it
//!   since, upstream wins, and the entry goes;
//! - otherwise the fetch is older than the save, and the saved version stays.
//!
//! Only a fetch that STARTED after the save may settle its entry. One that started before can't have
//! seen the save, whatever it shows, so it is overlaid and the entry stays for the next. (C4 also
//! makes the sync single-flight, so an older fetch can never arrive after a newer one has settled an
//! entry.)
//!
//! `updated` is only ever compared with another `updated` from Google, never with this computer's
//! clock (which has run hours off). An entry expires after ten minutes by the monotonic clock OR the
//! wall clock: `Instant` stands still while Linux and macOS are suspended (the lesson `power.rs`
//! records), so a save from before an overnight sleep would otherwise still outrank the first fetch
//! after it, and bring back an event deleted overnight.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use crate::calendar::CalendarEvent;

/// How long a save outranks a fetch that doesn't show it yet.
pub const TTL: Duration = Duration::from_secs(10 * 60);

/// A moment on both clocks.
#[derive(Debug, Clone, Copy)]
pub struct Stamp {
    pub mono: Instant,
    pub wall: SystemTime,
}

impl Stamp {
    pub fn now() -> Self {
        Stamp {
            mono: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    /// Whether `TTL` has passed since `self` by either clock. A wall clock that jumped backwards
    /// counts as no time, so a clock correction can only end a hold early, never extend it.
    fn expired_at(&self, now: Stamp) -> bool {
        now.mono.saturating_duration_since(self.mono) >= TTL
            || now
                .wall
                .duration_since(self.wall)
                .is_ok_and(|gap| gap >= TTL)
    }
}

/// What PM last did to one mirror row.
#[derive(Clone)]
enum Entry {
    /// Saved: Google's reply, as a row. Boxed so a delete entry doesn't carry a whole row's space.
    Upsert(Box<CalendarEvent>),
    /// Deleted: the version that was deleted.
    Delete {
        etag: Option<String>,
        updated: Option<String>,
    },
}

/// The saves of the last ten minutes, by (calendar id, row id).
#[derive(Default)]
pub struct RecentWrites {
    entries: HashMap<(String, String), (Entry, Stamp)>,
}

/// A fetch after the recent saves are laid over it.
pub struct Merged {
    pub rows: Vec<CalendarEvent>,
    /// Rows a save deleted that an INCOMPLETE fetch can't drop by omission (it only upserts), so the
    /// caller deletes them explicitly. Empty after a complete fetch, which drops them by leaving them
    /// out of `rows`.
    pub deletes: Vec<String>,
}

impl RecentWrites {
    /// Remember a saved row (Google's reply, parsed).
    pub fn record_upsert(&mut self, row: CalendarEvent, at: Stamp) {
        self.entries.insert(
            (row.calendar_id.clone(), row.id.clone()),
            (Entry::Upsert(Box::new(row)), at),
        );
    }

    /// Remember a deleted row, with the version that was deleted.
    pub fn record_delete(
        &mut self,
        calendar_id: &str,
        row_id: &str,
        etag: Option<String>,
        updated: Option<String>,
        at: Stamp,
    ) {
        self.entries.insert(
            (calendar_id.to_string(), row_id.to_string()),
            (Entry::Delete { etag, updated }, at),
        );
    }

    /// Whether anything is waiting to be reconciled.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Lay this calendar's recent saves over a fresh fetch of it, dropping each entry the fetch
    /// settles. `complete` is the fetch's own verdict on whether it saw the whole calendar;
    /// `fetch_started` is when it began, which decides whether it may settle anything.
    pub fn merge(
        &mut self,
        calendar_id: &str,
        fetched: Vec<CalendarEvent>,
        complete: bool,
        fetch_started: Instant,
        now: Stamp,
    ) -> Merged {
        self.entries.retain(|_, (_, at)| !at.expired_at(now));
        let mut rows = fetched;
        let mut deletes = Vec::new();
        let keys: Vec<(String, String)> = self
            .entries
            .keys()
            .filter(|(cal, _)| cal == calendar_id)
            .cloned()
            .collect();
        for key in keys {
            let (entry, at) = &self.entries[&key];
            // A fetch that began before the save can't have seen it: it may only be overlaid.
            let may_settle = fetch_started >= at.mono;
            let row_id = key.1.as_str();
            let position = rows.iter().position(|r| r.id == row_id);
            let settled = match (entry, position) {
                (Entry::Upsert(saved), Some(i)) => {
                    let fetched = &rows[i];
                    let landed = fetched.etag.is_some() && fetched.etag == saved.etag;
                    // Changed in Google after the save: upstream wins, whenever the fetch began.
                    let upstream_won =
                        strictly_later(fetched.updated.as_deref(), saved.updated.as_deref());
                    if !(landed || upstream_won) {
                        rows[i] = (**saved).clone(); // the fetch is older than the save
                    }
                    may_settle && (landed || upstream_won)
                }
                // Missing from a complete fetch that began after the save: deleted in Google since
                // (or moved out of the window), and upstream wins. Every save here edits an event
                // that already existed; C10's creates, which a lagging list may not show yet, will
                // need their own kind of entry. An incomplete or older fetch proves nothing, so it
                // shows the save.
                (Entry::Upsert(saved), None) => {
                    if complete && may_settle {
                        true
                    } else {
                        rows.push((**saved).clone());
                        false
                    }
                }
                (Entry::Delete { etag, updated }, Some(i)) => {
                    let fetched = &rows[i];
                    let restored = strictly_later(fetched.updated.as_deref(), updated.as_deref())
                        && fetched.etag != *etag;
                    if restored {
                        may_settle // restored or changed in Google after the delete: upstream wins
                    } else {
                        rows.remove(i); // the fetch still shows the deleted version
                        if !complete {
                            deletes.push(row_id.to_string());
                        }
                        false
                    }
                }
                // Gone from a complete fetch that began after the delete: it landed. An incomplete
                // or older fetch proves nothing, so the entry stays (and an incomplete one deletes the
                // row explicitly).
                (Entry::Delete { .. }, None) => {
                    if !complete {
                        deletes.push(row_id.to_string());
                    }
                    complete && may_settle
                }
            };
            if settled {
                self.entries.remove(&key);
            }
        }
        Merged { rows, deletes }
    }
}

/// Whether Google's `a` is strictly after Google's `b`. Unknown or unparseable on either side is
/// "not later": the save keeps its place until it expires.
fn strictly_later(a: Option<&str>, b: Option<&str>) -> bool {
    let parse = |s: Option<&str>| s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, summary: &str, etag: &str, updated: &str) -> CalendarEvent {
        CalendarEvent {
            id: format!("cal:{id}"),
            calendar_id: "cal".into(),
            summary: summary.into(),
            start: "2026-10-12T09:00:00Z".into(),
            etag: Some(etag.into()),
            updated: Some(updated.into()),
            ..Default::default()
        }
    }

    const T0: &str = "2026-10-12T08:00:00.000Z";
    const T1: &str = "2026-10-12T08:05:00.000Z";
    const T2: &str = "2026-10-12T08:10:00.000Z";

    fn summaries(m: &Merged) -> Vec<&str> {
        m.rows.iter().map(|r| r.summary.as_str()).collect()
    }

    /// A save at `saved`, and a moment a little after it on both clocks.
    fn clocks() -> (Stamp, Stamp) {
        let saved = Stamp::now();
        let after = Stamp {
            mono: saved.mono + Duration::from_secs(5),
            wall: saved.wall + Duration::from_secs(5),
        };
        (saved, after)
    }

    #[test]
    fn a_fetch_older_than_the_save_keeps_the_save_on_screen() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "New title", "\"2\"", T1), saved);
        let merged = w.merge(
            "cal",
            vec![row("a", "Old title", "\"1\"", T0)],
            true,
            after.mono,
            after,
        );
        assert_eq!(summaries(&merged), vec!["New title"]);
        assert!(!w.is_empty(), "kept until a fetch shows it");
    }

    #[test]
    fn a_fetch_showing_the_save_settles_it() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "New title", "\"2\"", T1), saved);
        let merged = w.merge(
            "cal",
            vec![row("a", "New title", "\"2\"", T1)],
            true,
            after.mono,
            after,
        );
        assert_eq!(summaries(&merged), vec!["New title"]);
        assert!(w.is_empty());
    }

    /// A reminder-only save changes the etag but not `updated`: the matching etag still settles it,
    /// and an older fetch with the same `updated` doesn't count as upstream winning.
    #[test]
    fn a_reminder_only_save_reconciles_on_the_etag() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "Standup", "\"2\"", T0), saved);
        let stale = w.merge(
            "cal",
            vec![row("a", "Standup", "\"1\"", T0)],
            true,
            after.mono,
            after,
        );
        assert_eq!(stale.rows[0].etag.as_deref(), Some("\"2\""));
        assert!(!w.is_empty());
        w.merge(
            "cal",
            vec![row("a", "Standup", "\"2\"", T0)],
            true,
            after.mono,
            after,
        );
        assert!(w.is_empty());
    }

    #[test]
    fn a_later_change_in_google_wins_over_the_save() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "Mine", "\"2\"", T1), saved);
        let merged = w.merge(
            "cal",
            vec![row("a", "Theirs", "\"3\"", T2)],
            true,
            after.mono,
            after,
        );
        assert_eq!(summaries(&merged), vec!["Theirs"]);
        assert!(w.is_empty());
    }

    #[test]
    fn a_saved_event_missing_from_a_later_complete_fetch_was_deleted_in_google() {
        let (saved, after) = clocks();
        let began_before = saved.mono - Duration::from_secs(1);
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "Saved", "\"2\"", T1), saved);
        let others = || vec![row("b", "Other", "\"1\"", T0)];
        // An older fetch, or one that didn't see the whole calendar, proves nothing: the save shows.
        let older = w.merge("cal", others(), true, began_before, after);
        assert_eq!(summaries(&older), vec!["Other", "Saved"]);
        let partial = w.merge("cal", others(), false, after.mono, after);
        assert_eq!(summaries(&partial), vec!["Other", "Saved"]);
        // A complete one that began after the save and lacks it: gone in Google, and settled.
        let complete = w.merge("cal", others(), true, after.mono, after);
        assert_eq!(summaries(&complete), vec!["Other"]);
        assert!(w.is_empty());
    }

    /// A fetch that began before the save shows what it shows, but settles nothing: only a fetch that
    /// began afterwards can know the save landed.
    #[test]
    fn a_fetch_that_began_before_the_save_settles_nothing() {
        let (saved, after) = clocks();
        let began_before = saved.mono - Duration::from_secs(1);
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "New title", "\"2\"", T1), saved);
        // Even showing the saved version (Google answered the save first, the list caught up).
        let early = w.merge(
            "cal",
            vec![row("a", "New title", "\"2\"", T1)],
            true,
            began_before,
            after,
        );
        assert_eq!(summaries(&early), vec!["New title"]);
        assert!(!w.is_empty());
        // A later-starting fetch settles it.
        w.merge(
            "cal",
            vec![row("a", "New title", "\"2\"", T1)],
            true,
            after.mono,
            after,
        );
        assert!(w.is_empty());

        // The same for a delete: an older complete fetch without the row proves nothing.
        w.record_delete("cal", "cal:b", Some("\"1\"".into()), Some(T0.into()), saved);
        w.merge("cal", vec![], true, began_before, after);
        assert!(!w.is_empty());
        let stale = w.merge(
            "cal",
            vec![row("b", "Dentist", "\"1\"", T0)],
            true,
            began_before,
            after,
        );
        assert!(stale.rows.is_empty(), "still hidden");
        w.merge("cal", vec![], true, after.mono, after);
        assert!(w.is_empty());
    }

    #[test]
    fn a_delete_stays_deleted_until_google_agrees() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_delete("cal", "cal:a", Some("\"1\"".into()), Some(T0.into()), saved);
        // A stale fetch still has it: hidden.
        let stale = w.merge(
            "cal",
            vec![row("a", "Dentist", "\"1\"", T0)],
            true,
            after.mono,
            after,
        );
        assert!(stale.rows.is_empty());
        assert!(
            stale.deletes.is_empty(),
            "a complete fetch drops it by omission"
        );
        // A complete fetch without it: landed.
        let landed = w.merge("cal", vec![], true, after.mono, after);
        assert!(landed.rows.is_empty());
        assert!(w.is_empty());
    }

    #[test]
    fn an_incomplete_fetch_deletes_explicitly_and_proves_nothing() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_delete("cal", "cal:a", Some("\"1\"".into()), Some(T0.into()), saved);
        let partial = w.merge("cal", vec![], false, after.mono, after);
        assert_eq!(partial.deletes, vec!["cal:a"]);
        assert!(!w.is_empty(), "an incomplete fetch can't show it landed");
        let still = w.merge(
            "cal",
            vec![row("a", "Dentist", "\"1\"", T0)],
            false,
            after.mono,
            after,
        );
        assert!(still.rows.is_empty());
        assert_eq!(still.deletes, vec!["cal:a"]);
    }

    #[test]
    fn an_event_restored_in_google_after_the_delete_comes_back() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_delete("cal", "cal:a", Some("\"1\"".into()), Some(T0.into()), saved);
        let merged = w.merge(
            "cal",
            vec![row("a", "Dentist", "\"5\"", T2)],
            true,
            after.mono,
            after,
        );
        assert_eq!(summaries(&merged), vec!["Dentist"]);
        assert!(w.is_empty());
    }

    #[test]
    fn entries_expire_after_ten_minutes_and_stay_per_calendar() {
        let (saved, after) = clocks();
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "New title", "\"2\"", T1), saved);
        // Another calendar's fetch leaves it alone.
        let other = w.merge("other", vec![], true, after.mono, after);
        assert!(other.rows.is_empty());
        assert!(!w.is_empty());
        // Ten minutes on, the fetch simply wins.
        let later = Stamp {
            mono: saved.mono + TTL + Duration::from_secs(1),
            wall: saved.wall + TTL + Duration::from_secs(1),
        };
        let merged = w.merge(
            "cal",
            vec![row("a", "Old title", "\"1\"", T0)],
            true,
            later.mono,
            later,
        );
        assert_eq!(summaries(&merged), vec!["Old title"]);
        assert!(w.is_empty());
    }

    /// The laptop slept all night: the monotonic clock moved two minutes, the wall clock eight
    /// hours. The save is long stale, and an event deleted overnight must not come back.
    #[test]
    fn a_save_from_before_a_long_sleep_has_expired() {
        let saved = Stamp::now();
        let woke = Stamp {
            mono: saved.mono + Duration::from_secs(120),
            wall: saved.wall + Duration::from_secs(8 * 3600),
        };
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "Dentist", "\"2\"", T1), saved);
        let merged = w.merge("cal", vec![], true, woke.mono, woke);
        assert!(merged.rows.is_empty());
        assert!(w.is_empty());
        // A wall clock set BACK doesn't extend a hold, and doesn't end it either.
        let mut w = RecentWrites::default();
        w.record_upsert(row("a", "Dentist", "\"2\"", T1), saved);
        let back = Stamp {
            mono: saved.mono + Duration::from_secs(60),
            wall: saved.wall - Duration::from_secs(3600),
        };
        // (An incomplete fetch, so only the clocks could end the hold.)
        let held = w.merge("cal", vec![], false, back.mono, back);
        assert_eq!(summaries(&held), vec!["Dentist"]);
        assert!(!w.is_empty());
    }

    #[test]
    fn unparseable_updated_never_counts_as_later() {
        assert!(!strictly_later(Some("yesterday"), Some(T0)));
        assert!(!strictly_later(None, Some(T0)));
        assert!(strictly_later(Some(T1), Some(T0)));
        assert!(!strictly_later(Some(T0), Some(T0)));
    }
}
