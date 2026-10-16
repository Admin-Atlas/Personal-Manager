// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A series re-fetched after a write, as mirror rows (R3, R4). One write to a series changes many
//! rows (a split ends one series and starts another; a new time can give every occurrence a new id),
//! and PM can't expand a Google series itself, so the mirror takes what an `events.instances`
//! re-fetch shows: [`diff_series`] turns it into rows to write and rows to remove, and says whether it
//! shows the write at all (a list can lag; A14). C9 records the result in `RecentWrites`, so a sync
//! that began before the write can't put the old rows back.
//!
//! Rows are matched by where each occurrence sits in its series (`original_start`), not by id: ids
//! are opaque, and an occurrence's may change when its series moves.

use std::cmp::Ordering;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::Value;

use crate::calendar::{parse_event, CalendarEvent};
use crate::calendar_recur::Point;

use super::{original_of, Listing};

/// What a re-fetch has to show before PM takes it as the series after the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Some occurrence of the series was updated at or after `after`, the write's own `updated`
    /// (C9 adds this from the reply: an occurrence follows its master's; INFERRED, L-R14).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "first made in C9, from a write's reply")
    )]
    Updated { series: String, after: String },
    /// The occurrence at `original` is cancelled, or gone.
    Cancelled { series: String, original: Point },
    /// Nothing of the series from `at` on, but the occurrences at `left`: ones the run reported it
    /// couldn't cancel ([`super::plan::SeriesPlan::expect_after`]; the matching removals are
    /// [`super::plan::SeriesPlan::removes_for`]).
    EndsBefore {
        series: String,
        at: Point,
        left: Vec<Point>,
    },
    /// Something of the series (a new one).
    Present { series: String },
    /// Nothing of the series.
    Absent { series: String },
}

/// The rows a plan removes from the mirror, so a stale re-fetch can't bring them back (A14): a
/// delete's whatever a re-fetch shows, a split's once its new series shows
/// ([`super::plan::SeriesPlan::removes_for`]). By where each occurrence sits in its series (DS17,
/// AS14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removes {
    Nothing,
    /// One occurrence, by Google's id.
    Occurrence {
        event: String,
    },
    /// Every occurrence of a series.
    Series {
        series: String,
    },
    /// A series' occurrences from `at` on, but those at `left` (still in Google, see
    /// [`Expect::EndsBefore`]).
    From {
        series: String,
        at: Point,
        left: Vec<Point>,
    },
}

/// A mirror row a write removes, with the version removed (for `RecentWrites::record_delete`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoneRow {
    pub row_id: String,
    pub etag: Option<String>,
    pub updated: Option<String>,
}

impl GoneRow {
    /// `row` removed, its version no older than `floor`, a moment at or after every version of it
    /// before the write: a cut's or a patch's own reply `updated`; for a whole series' delete (a 204,
    /// no body) [`floor_of`] the series' fresh copy, every fresh exception, and the 204's `Date`,
    /// since an occurrence changed on its own doesn't move its series' `updated`. The mirror can lag
    /// Google, and a sync already running may list a version newer than the mirror's but older than
    /// the write; that must never read as the row restored after it (A14; C8b reviews R4).
    fn of(row: &CalendarEvent, floor: Option<&str>) -> GoneRow {
        let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok();
        let updated = match (row.updated.as_deref(), floor) {
            (Some(u), Some(f)) => match (parse(u), parse(f)) {
                (Some(a), Some(b)) if b > a => Some(f.to_string()),
                _ => Some(u.to_string()),
            },
            (None, Some(f)) => Some(f.to_string()),
            (u, None) => u.map(str::to_string),
        };
        GoneRow {
            row_id: row.id.clone(),
            etag: row.etag.clone(),
            updated,
        }
    }
}

/// The latest of `moments` (RFC 3339; others are ignored), for a removal's floor ([`GoneRow`]). An
/// HTTP `Date` is whole seconds, so C9 passes it a second on, as RFC 3339.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9, editing repeating events"
    )
)]
pub fn floor_of<'a>(moments: impl IntoIterator<Item = &'a str>) -> Option<String> {
    moments
        .into_iter()
        .filter_map(|m| DateTime::parse_from_rfc3339(m).ok().map(|t| (t, m)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, m)| m.to_string())
}

/// Where a mirror row sits in its series: its `original_start` (or its start, for a row without one),
/// as the mirror normalises them (`…Z`, or a bare date).
fn row_point(row: &CalendarEvent) -> Option<Point> {
    mirror_point(row.original_start.as_deref().unwrap_or(&row.start))
}

fn mirror_point(s: &str) -> Option<Point> {
    if let Ok(day) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(Point::Day(day));
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| Point::At(t.with_timezone(&Utc)))
}

impl Removes {
    /// The rows of `before` (the mirror's rows of `calendar_id`) this removes, each no older than
    /// `floor` ([`GoneRow`]).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn rows(
        &self,
        calendar_id: &str,
        before: &[CalendarEvent],
        zone: Tz,
        floor: Option<&str>,
    ) -> Vec<GoneRow> {
        before
            .iter()
            .filter(|row| match self {
                Removes::Nothing => false,
                Removes::Occurrence { event } => row.id == format!("{calendar_id}:{event}"),
                Removes::Series { series } => row.series_id.as_deref() == Some(series.as_str()),
                Removes::From { series, at, left } => {
                    row.series_id.as_deref() == Some(series.as_str())
                        && row_point(row).is_some_and(|p| {
                            p.cmp_in(at, zone) != Ordering::Less && !left.contains(&p)
                        })
                }
            })
            .map(|row| GoneRow::of(row, floor))
            .collect()
    }
}

/// One series re-fetched: `events.instances` with cancelled ones, over the mirror's own window.
#[derive(Debug, Clone, Copy)]
pub struct Refetch<'a> {
    pub series: &'a str,
    pub listing: &'a Listing,
}

/// A re-fetch as mirror changes.
#[derive(Clone)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first read in C9, landing a series write")
)]
pub struct SeriesDiff {
    /// Every occurrence listed, as a sync would mirror it.
    pub upserts: Vec<CalendarEvent>,
    /// The ids among them the mirror didn't have (new rows: a split's new series).
    pub created: Vec<String>,
    pub deletes: Vec<GoneRow>,
    /// The re-fetch doesn't show the write yet: record nothing from it, and keep the rows on screen
    /// until a later fetch does (A14).
    pub stale: bool,
}

/// Turn the re-fetched series into mirror changes. `before`: the mirror's rows of those series, in
/// `calendar_id`. `window`: the re-fetch's `timeMin`/`timeMax` (the mirror's own window); a row
/// outside it is neither written nor removed by omission (DS13). A row missing from a listing is
/// removed only when that listing is complete. `floor`: the write's own `updated` ([`GoneRow`]).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9, editing repeating events"
    )
)]
pub fn diff_series(
    calendar_id: &str,
    before: &[CalendarEvent],
    refetched: &[Refetch],
    window: (DateTime<Utc>, DateTime<Utc>),
    expect: &[Expect],
    zone: Tz,
    floor: Option<&str>,
) -> SeriesDiff {
    let in_window = |row: &CalendarEvent| {
        let start = mirror_point(&row.start).map(instant);
        let end = row.end.as_deref().and_then(mirror_point).map(instant);
        match (start, end.or(start)) {
            // Google lists what ends after timeMin and starts before timeMax.
            (Some(s), Some(e)) => s < window.1 && e > window.0,
            _ => false,
        }
    };
    let mut upserts: Vec<CalendarEvent> = Vec::new();
    let mut cancelled: Vec<String> = Vec::new();
    for r in refetched {
        for item in &r.listing.items {
            if item.get("status").and_then(Value::as_str) == Some("cancelled") {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    cancelled.push(format!("{calendar_id}:{id}"));
                }
                continue;
            }
            if let Some(row) = parse_event(calendar_id, item).filter(|row| in_window(row)) {
                upserts.push(row);
            }
        }
    }
    let complete = |series: &str| {
        refetched
            .iter()
            .any(|r| r.series == series && r.listing.complete)
    };
    let deletes = before
        .iter()
        .filter(|row| {
            if cancelled.contains(&row.id) {
                return true;
            }
            if upserts.iter().any(|u| u.id == row.id) {
                return false;
            }
            // The same occurrence now under another id.
            let moved_id = upserts.iter().any(|u| {
                u.series_id.is_some()
                    && u.series_id == row.series_id
                    && row_point(u).is_some()
                    && row_point(u) == row_point(row)
            });
            moved_id || (row.series_id.as_deref().is_some_and(complete) && in_window(row))
        })
        .map(|row| GoneRow::of(row, floor))
        .collect();
    let created = upserts
        .iter()
        .filter(|u| !before.iter().any(|b| b.id == u.id))
        .map(|u| u.id.clone())
        .collect();
    let stale = !expect.iter().all(|e| shows(e, refetched, zone));
    SeriesDiff {
        upserts,
        created,
        deletes,
        stale,
    }
}

fn instant(p: Point) -> DateTime<Utc> {
    match p {
        Point::At(t) => t,
        Point::Day(d) => d.and_time(chrono::NaiveTime::MIN).and_utc(),
    }
}

/// Whether the re-fetch shows `expect`. A series that wasn't re-fetched shows nothing.
fn shows(expect: &Expect, refetched: &[Refetch], zone: Tz) -> bool {
    let series_of = |e: &Expect| match e {
        Expect::Updated { series, .. }
        | Expect::Cancelled { series, .. }
        | Expect::EndsBefore { series, .. }
        | Expect::Present { series }
        | Expect::Absent { series } => series.clone(),
    };
    let series = series_of(expect);
    let Some(r) = refetched.iter().find(|r| r.series == series) else {
        return false;
    };
    let live = || {
        r.listing
            .items
            .iter()
            .filter(|i| i.get("status").and_then(Value::as_str) != Some("cancelled"))
    };
    match expect {
        Expect::Updated { after, .. } => {
            let after = DateTime::parse_from_rfc3339(after).ok();
            live().any(|i| {
                let updated = i
                    .get("updated")
                    .and_then(Value::as_str)
                    .and_then(|u| DateTime::parse_from_rfc3339(u).ok());
                matches!((updated, after), (Some(u), Some(a)) if u >= a)
            })
        }
        Expect::Cancelled { original, .. } => !live().any(|i| original_of(i) == Some(*original)),
        Expect::EndsBefore { at, left, .. } => {
            r.listing.complete
                && !live().any(|i| {
                    original_of(i)
                        .is_some_and(|p| p.cmp_in(at, zone) != Ordering::Less && !left.contains(&p))
                })
        }
        Expect::Present { .. } => live().next().is_some(),
        Expect::Absent { .. } => r.listing.complete && live().next().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::*;
    use chrono_tz::Europe::London;
    use serde_json::json;

    const CAL: &str = "gcal:me@example.com:me@example.com";

    fn row(v: &Value) -> CalendarEvent {
        parse_event(CAL, v).unwrap()
    }

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            "2026-10-01T00:00:00Z".parse().unwrap(),
            "2027-11-01T00:00:00Z".parse().unwrap(),
        )
    }

    fn point(s: &str) -> Point {
        Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    fn ids(rows: &[GoneRow]) -> Vec<&str> {
        rows.iter().map(|r| r.row_id.as_str()).collect()
    }

    /// T12: a series re-fetched after a cut: the occurrences listed are written, those it no longer
    /// has are removed, and a cancelled one is removed even from a listing that didn't run to its end.
    #[test]
    fn a_refetch_writes_what_it_lists_and_removes_what_it_lost() {
        let before = vec![
            row(&plain("2026-10-12T09:00:00+01:00")),
            row(&plain("2026-10-19T09:00:00+01:00")),
            row(&plain("2026-10-26T09:00:00Z")),
        ];
        let listing = Listing {
            items: vec![
                plain("2026-10-12T09:00:00+01:00"),
                cancelled("2026-10-19T09:00:00+01:00"),
            ],
            complete: true,
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &listing,
        }];
        let expect = [Expect::EndsBefore {
            series: "abc".into(),
            at: point("2026-10-19T08:00:00Z"),
            left: Vec::new(),
        }];
        let diff = diff_series(CAL, &before, &refetch, window(), &expect, London, None);
        assert_eq!(diff.upserts.len(), 1);
        assert_eq!(
            ids(&diff.deletes),
            vec![
                format!("{CAL}:abc_20261019T080000Z"),
                format!("{CAL}:abc_20261026T090000Z")
            ]
        );
        assert!(diff.created.is_empty());
        assert!(!diff.stale);
        // Not complete: only the cancelled one goes; the missing 26-10 proves nothing.
        let partial = Listing {
            complete: false,
            ..listing.clone()
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &partial,
        }];
        let diff = diff_series(CAL, &before, &refetch, window(), &[], London, None);
        assert_eq!(
            ids(&diff.deletes),
            vec![format!("{CAL}:abc_20261019T080000Z")]
        );
    }

    /// An occurrence listed under a new id (its series moved) replaces its old row; a row outside
    /// the re-fetch's window is left alone (DS13); a new series' rows are "created".
    #[test]
    fn rows_are_matched_by_their_place_in_the_series() {
        let old = row(&plain("2026-10-12T09:00:00+01:00"));
        let mut renamed_id = plain("2026-10-12T09:00:00+01:00");
        renamed_id["id"] = json!("abc_r2_20261012");
        let far = row(&plain("2028-01-03T09:00:00Z"));
        let tail = with(
            with(
                plain("2026-10-19T09:00:00+01:00"),
                "recurringEventId",
                json!("tail"),
            ),
            "id",
            json!("tail_20261019"),
        );
        let listing = Listing {
            items: vec![renamed_id],
            complete: true,
        };
        let tail_listing = Listing {
            items: vec![tail],
            complete: true,
        };
        let refetch = [
            Refetch {
                series: "abc",
                listing: &listing,
            },
            Refetch {
                series: "tail",
                listing: &tail_listing,
            },
        ];
        let diff = diff_series(
            CAL,
            &[old.clone(), far],
            &refetch,
            window(),
            &[Expect::Present {
                series: "tail".into(),
            }],
            London,
            None,
        );
        assert_eq!(ids(&diff.deletes), vec![old.id.as_str()]);
        assert_eq!(
            diff.created,
            vec![
                format!("{CAL}:abc_r2_20261012"),
                format!("{CAL}:tail_20261019")
            ]
        );
        assert!(!diff.stale);
    }

    /// A14: a re-fetch that doesn't show the write yet is stale; so is one missing a series.
    #[test]
    fn a_refetch_that_doesnt_show_the_write_is_stale() {
        let listing = Listing {
            items: vec![
                plain("2026-10-12T09:00:00+01:00"),
                plain("2026-10-19T09:00:00+01:00"),
            ],
            complete: true,
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &listing,
        }];
        let stale = |expect: Expect| {
            diff_series(CAL, &[], &refetch, window(), &[expect], London, None).stale
        };
        assert!(stale(Expect::EndsBefore {
            series: "abc".into(),
            at: point("2026-10-19T08:00:00Z"),
            left: Vec::new(),
        }));
        assert!(stale(Expect::Present {
            series: "tail".into()
        }));
        assert!(stale(Expect::Absent {
            series: "abc".into()
        }));
        assert!(stale(Expect::Cancelled {
            series: "abc".into(),
            original: point("2026-10-19T08:00:00Z")
        }));
        assert!(!stale(Expect::Cancelled {
            series: "abc".into(),
            original: point("2026-10-26T09:00:00Z")
        }));
        // An occurrence updated by the write (its master's `updated`) shows it.
        assert!(stale(Expect::Updated {
            series: "abc".into(),
            after: "2026-10-16T12:00:00.000Z".into()
        }));
        assert!(!stale(Expect::Updated {
            series: "abc".into(),
            after: "2026-10-01T09:00:00.000Z".into()
        }));
    }

    /// DS17: what a plan removes is chosen by each row's place in the series, not its start: a row
    /// moved past the split from before it stays.
    #[test]
    fn a_plans_removals_go_by_place_in_the_series() {
        let early_moved_late = row(&moved(
            "2026-10-12T09:00:00+01:00",
            "2026-10-28T09:00:00Z",
            "2026-10-28T10:00:00Z",
        ));
        let at_split = row(&plain("2026-10-19T09:00:00+01:00"));
        let later = row(&plain("2026-10-26T09:00:00Z"));
        let before = vec![early_moved_late, at_split.clone(), later.clone()];
        let from = Removes::From {
            series: "abc".into(),
            at: point("2026-10-19T08:00:00Z"),
            left: Vec::new(),
        };
        assert_eq!(
            ids(&from.rows(CAL, &before, London, None)),
            vec![at_split.id.as_str(), later.id.as_str()]
        );
        assert_eq!(
            Removes::Series {
                series: "abc".into()
            }
            .rows(CAL, &before, London, None)
            .len(),
            3
        );
        assert_eq!(
            ids(&Removes::Occurrence {
                event: "abc_20261026T090000Z".into()
            }
            .rows(CAL, &before, London, None)),
            vec![later.id.as_str()]
        );
        assert!(Removes::Nothing.rows(CAL, &before, London, None).is_empty());
    }

    /// C8b review R7: a listing that didn't run to its end proves no series ended; a complete one
    /// with only cancelled items proves a series gone; a series that wasn't re-fetched loses a row only
    /// to a cancelled id; an occurrence outside the window is neither written nor new.
    #[test]
    fn what_a_refetch_can_and_cant_prove() {
        let at = point("2026-10-19T08:00:00Z");
        let cut = Listing {
            items: vec![plain("2026-10-12T09:00:00+01:00")],
            complete: false,
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &cut,
        }];
        let ends = Expect::EndsBefore {
            series: "abc".into(),
            at,
            left: Vec::new(),
        };
        let diff = diff_series(CAL, &[], &refetch, window(), &[ends], London, None);
        assert!(diff.stale);

        let gone = Listing {
            items: vec![cancelled("2026-10-12T09:00:00+01:00")],
            complete: true,
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &gone,
        }];
        let absent = Expect::Absent {
            series: "abc".into(),
        };
        assert!(!diff_series(CAL, &[], &refetch, window(), &[absent], London, None).stale);

        let other = with(
            plain("2026-10-13T09:00:00+01:00"),
            "recurringEventId",
            json!("other"),
        );
        let other = row(&with(other, "id", json!("other_20261013")));
        let diff = diff_series(CAL, &[other], &refetch, window(), &[], London, None);
        assert!(diff.deletes.is_empty());

        let far = Listing {
            items: vec![plain("2028-01-03T09:00:00Z")],
            complete: true,
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &far,
        }];
        let diff = diff_series(CAL, &[], &refetch, window(), &[], London, None);
        assert!(diff.upserts.is_empty() && diff.created.is_empty());
    }

    /// C8b review R2: an occurrence the run reported it couldn't cancel is still in Google. A
    /// re-fetch listing it still shows the cut, and its row stays.
    #[test]
    fn an_occurrence_left_behind_neither_stales_the_refetch_nor_leaves_the_mirror() {
        let left = point("2026-11-09T09:00:00Z");
        let listing = Listing {
            items: vec![
                plain("2026-10-12T09:00:00+01:00"),
                with(plain("2026-11-09T09:00:00Z"), "summary", json!("Off-site")),
            ],
            complete: true,
        };
        let refetch = [Refetch {
            series: "abc",
            listing: &listing,
        }];
        let ends = |left: Vec<Point>| Expect::EndsBefore {
            series: "abc".into(),
            at: point("2026-10-19T08:00:00Z"),
            left,
        };
        let stale = |e: Expect| diff_series(CAL, &[], &refetch, window(), &[e], London, None).stale;
        assert!(stale(ends(Vec::new())));
        assert!(!stale(ends(vec![left])));
        let kept = row(&plain("2026-11-09T09:00:00Z"));
        let cut = row(&plain("2026-10-26T09:00:00Z"));
        let from = Removes::From {
            series: "abc".into(),
            at: point("2026-10-19T08:00:00Z"),
            left: vec![left],
        };
        assert_eq!(
            ids(&from.rows(CAL, &[kept, cut.clone()], London, None)),
            vec![cut.id.as_str()]
        );
    }

    /// C8b review R4: a removed row's version is never older than the write, so a sync that began
    /// before it can't read a newer pre-write copy as the row restored.
    #[test]
    fn a_removed_row_is_never_older_than_the_write() {
        let mirror = row(&plain("2026-10-26T09:00:00Z"));
        let series = Removes::Series {
            series: "abc".into(),
        };
        let removed = |floor| series.rows(CAL, std::slice::from_ref(&mirror), London, floor);
        assert_eq!(
            removed(Some("2026-10-16T12:00:00.000Z"))[0]
                .updated
                .as_deref(),
            Some("2026-10-16T12:00:00.000Z")
        );
        // A floor older than the row's own version, or none, keeps the row's.
        assert_eq!(
            removed(Some("2026-09-01T00:00:00Z"))[0].updated.as_deref(),
            Some("2026-10-01T10:00:00.000Z")
        );
        assert_eq!(
            removed(None)[0].updated.as_deref(),
            Some("2026-10-01T10:00:00.000Z")
        );
        // A whole series' delete has no reply: its floor is the latest of the series' fresh copy,
        // its fresh exceptions (one changed on its own doesn't move the series) and the 204's Date.
        let floor = floor_of([
            "2026-10-01T10:00:00.000Z",
            "2026-10-15T08:30:00.000Z",
            "not a time",
            "2026-10-16T12:00:01+00:00",
        ]);
        assert_eq!(floor.as_deref(), Some("2026-10-16T12:00:01+00:00"));
        assert_eq!(floor_of(["not a time"]), None);
    }
}
