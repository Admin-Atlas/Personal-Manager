// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The structured flag layer (board card 9) — proactive flags as first-class records,
//! evaluated BEFORE the briefing/chat model writes and then rendered into prose. It
//! replaces the free-associating daily briefing with a stable decision layer under it:
//! the generated sentence is volatile, the flag underneath is stable, so identity and
//! resolution attach to the FLAG, never to the rendered text — which is what makes daily
//! regeneration idempotent.
//!
//! A flag hangs off a STABLE anchor that already exists elsewhere in the store, so this
//! layer needs no anchor migration of its own:
//! - `anchor_kind = "calendar"` → an iCal `UID` ([`crate::calendar`] mirror, `uid` column).
//! - `anchor_kind = "milestone"` → a milestone surrogate id ([`crate::milestones`],
//!   `project_milestones.id`). Deadline flags anchor on the MILESTONE, never the project,
//!   so each of a project's dated milestones (pitch, presentation, internal) carries its
//!   own independent flags.
//!
//! Resolution keys on `(anchor_kind, anchor, type)` (the table's UNIQUE key), so resolving
//! "pitch prep" never touches "presentation prep" or "happening-today" on the same anchor.
//!
//! **Storage seam (done-vs-preference, kept physically separate):** THIS layer holds the
//! per-instance flag-STATE — transient, scoped to the anchored instance, garbage-collected
//! once it passes. A CROSS-instance PREFERENCE ("stop nagging me two hours out") is durable
//! and lives in [`crate::preferences`], never here. The two are never co-mingled.
//!
//! **Detection (PR2):** [`detect`] is a PURE reducer — given the focus-view projects and the
//! upcoming calendar, it proposes the flag set the current state implies (deadline-approaching /
//! overdue per unmet milestone; happening-today / prepare-ahead per calendar event series).
//! [`detect_and_store`] runs it and RECONCILES the stored set: it upserts every proposed flag and
//! prunes any detection-owned active flag the state no longer implies (a passed event, a met or
//! slipped milestone) — resolved flags are protected state and never touched. Because the briefing
//! path and the [`spawn_flag_detection_scheduler`] backstop both derive from the same deterministic
//! reducer, they converge on the same set and never fight. The briefing then renders that active
//! set (see [`crate::briefing`]); assertion/HITL and chat grounding follow in later PRs.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::calendar::{self, CalendarEvent};
use crate::error::Result;
use crate::milestones;
use crate::projects::{self, ProjectOverview};
use crate::{clock, db, AppState};

// Which identity space a flag's `anchor` string lives in.
pub const ANCHOR_CALENDAR: &str = "calendar";
pub const ANCHOR_MILESTONE: &str = "milestone";

// The flag taxonomy. `(anchor, type)` is the resolution key.
pub const TYPE_PREPARE_AHEAD: &str = "prepare-ahead";
pub const TYPE_DEADLINE_APPROACHING: &str = "deadline-approaching";
pub const TYPE_HAPPENING_TODAY: &str = "happening-today";
pub const TYPE_OVERDUE: &str = "overdue";

// Lifecycle + provenance. Detection/reconcile query these states as SQL literals; the Rust-side
// constants (and the `SOURCE_*` pair) are the assertion/resolution vocabulary the manual "done"
// flow (PR3) compares against, so they're exercised only by unit tests until then — hence the
// scoped `#[allow(dead_code)]`.
#[allow(dead_code)] // consumed by assertion/resolution (PR3)
pub const STATE_ACTIVE: &str = "active";
#[allow(dead_code)] // consumed by assertion/resolution (PR3)
pub const STATE_RESOLVED: &str = "resolved";

// Which path CLOSED a flag (NULL while active). On conflict, assertion outranks detection.
#[allow(dead_code)] // consumed by assertion/resolution (PR3)
pub const SOURCE_DETECTION: &str = "detection";
#[allow(dead_code)] // consumed by assertion/resolution (PR3)
pub const SOURCE_ASSERTION: &str = "assertion";

/// A flag as stored, and as the briefing/chat layer will read it. `type` is a Rust keyword,
/// so the field is `r#type` — serde serialises it as `"type"` for the frontend.
#[derive(Clone, Debug, Serialize)]
pub struct Flag {
    pub id: i64,
    pub anchor_kind: String,
    pub anchor: String,
    pub r#type: String,
    /// How far ahead of the anchored time the flag fires; `None` = the type's default.
    pub threshold: Option<String>,
    pub state: String,
    /// Which path closed it (`detection`/`assertion`); `None` while still active.
    pub source: Option<String>,
    pub confidence: f64,
    /// A deliberate user vouch — true iff the flag was closed by assertion.
    pub user_confirmed: bool,
    /// `documents.source_id` of the satisfying artifact (the prep doc / Drive file), if found.
    pub artifact_ptr: Option<String>,
    /// `documents.external_ref` — the open URL for `artifact_ptr`; display-only (it moves on
    /// a rename, whereas `artifact_ptr` is the rename-survives identity).
    pub artifact_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
}

/// A flag proposed by detection, before it is persisted. `upsert_active` inserts it or
/// refreshes a live row's detection-derived fields — but never disturbs one already resolved.
#[derive(Clone, Debug)]
pub struct DraftFlag {
    pub anchor_kind: String,
    pub anchor: String,
    pub r#type: String,
    pub threshold: Option<String>,
    pub artifact_ptr: Option<String>,
    pub artifact_url: Option<String>,
}

const FLAG_COLUMNS: &str = "id, anchor_kind, anchor, type, threshold, state, source, \
     confidence, user_confirmed, artifact_ptr, artifact_url, created_at, updated_at, resolved_at";

fn row_to_flag(r: &rusqlite::Row) -> rusqlite::Result<Flag> {
    Ok(Flag {
        id: r.get(0)?,
        anchor_kind: r.get(1)?,
        anchor: r.get(2)?,
        r#type: r.get(3)?,
        threshold: r.get(4)?,
        state: r.get(5)?,
        source: r.get(6)?,
        confidence: r.get(7)?,
        user_confirmed: r.get(8)?,
        artifact_ptr: r.get(9)?,
        artifact_url: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        resolved_at: r.get(13)?,
    })
}

/// One flag by id, or `None` if the id is unknown. Used by the resolution flow (PR3) and the
/// tests; not yet reached from lib code, so it's allowed to be "dead" until then.
#[allow(dead_code)]
pub fn get(conn: &Connection, id: i64) -> Result<Option<Flag>> {
    Ok(conn
        .query_row(
            &format!("SELECT {FLAG_COLUMNS} FROM flags WHERE id = ?1"),
            params![id],
            row_to_flag,
        )
        .optional()?)
}

/// The active flags the briefing and chat render, in insertion order. When `anchor_kind` is
/// given, only that identity space (e.g. only milestone-anchored flags for a project surface).
pub fn list_active(conn: &Connection, anchor_kind: Option<&str>) -> Result<Vec<Flag>> {
    let rows = match anchor_kind {
        Some(kind) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FLAG_COLUMNS} FROM flags \
                 WHERE state = 'active' AND anchor_kind = ?1 ORDER BY id"
            ))?;
            let out: Vec<Flag> = stmt
                .query_map(params![kind], row_to_flag)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            out
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FLAG_COLUMNS} FROM flags WHERE state = 'active' ORDER BY id"
            ))?;
            let out: Vec<Flag> = stmt
                .query_map([], row_to_flag)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            out
        }
    };
    Ok(rows)
}

/// Insert a detected flag, or refresh the live row's detection-derived fields if one already
/// exists for its `(anchor_kind, anchor, type)`. **Resolution-preserving:** a row the user (or
/// a confirmed detection) has already RESOLVED is left completely untouched — the `WHERE
/// flags.state = 'active'` guard on the upsert makes re-detection a no-op there, so a resolved
/// flag never flips back to active across daily rescans (decision 1's idempotency). Returns the
/// flag's stable id whether it was inserted or already present.
pub fn upsert_active(conn: &Connection, f: &DraftFlag) -> Result<i64> {
    conn.execute(
        "INSERT INTO flags(anchor_kind, anchor, type, threshold, artifact_ptr, artifact_url) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(anchor_kind, anchor, type) DO UPDATE SET \
             threshold    = excluded.threshold, \
             artifact_ptr = excluded.artifact_ptr, \
             artifact_url = excluded.artifact_url, \
             updated_at   = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE flags.state = 'active'",
        params![
            f.anchor_kind,
            f.anchor,
            f.r#type,
            f.threshold,
            f.artifact_ptr,
            f.artifact_url
        ],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM flags WHERE anchor_kind = ?1 AND anchor = ?2 AND type = ?3",
        params![f.anchor_kind, f.anchor, f.r#type],
        |r| r.get(0),
    )?)
}

/// Resolve a flag: record WHICH path closed it and, optionally, the satisfying artifact it now
/// points at. Assertion is a deliberate user vouch (`user_confirmed = 1`); a detection verdict
/// is machine-derived (`user_confirmed = 0`) and — per the HITL-confirm-before-suppress rule —
/// must be confirmed before it is allowed to cross anything off (that gate lives in the
/// resolution PR). Passing `artifact_ptr = None` keeps whatever pointer detection already found.
/// The `resolve_flag` command (PR3) is its first lib consumer; until then only the tests call it.
#[allow(dead_code)]
pub fn resolve(conn: &Connection, id: i64, source: &str, artifact_ptr: Option<&str>) -> Result<()> {
    let user_confirmed = i64::from(source == SOURCE_ASSERTION);
    conn.execute(
        "UPDATE flags SET \
             state = 'resolved', source = ?2, user_confirmed = ?3, \
             artifact_ptr = COALESCE(?4, artifact_ptr), \
             updated_at  = strftime('%Y-%m-%dT%H:%M:%fZ','now'), \
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, source, user_confirmed, artifact_ptr],
    )?;
    Ok(())
}

/// The default lead time (in whole days) a flag type fires ahead of its anchored date, used
/// when a flag carries no explicit `threshold`. Pure — detection reads it to decide when a
/// flag should appear; `happening-today` and `overdue` are day-of/after, so their lead is 0.
pub fn default_threshold_days(flag_type: &str) -> f64 {
    match flag_type {
        TYPE_DEADLINE_APPROACHING => 7.0,
        TYPE_PREPARE_AHEAD => 3.0,
        TYPE_HAPPENING_TODAY | TYPE_OVERDUE => 0.0,
        _ => 0.0,
    }
}

// --- detection (pure reducer + reconciling executor + backstop scheduler) --------------------

/// How far ahead the calendar half of detection fetches events. It matches the briefing agenda
/// window; [`detect`] itself filters to each type's own lead (`prepare-ahead` within its 3-day
/// default, `happening-today` on the day), so a wider fetch just means a few events the reducer
/// then ignores. Kept in step with `briefing::BRIEFING_AGENDA_DAYS` by value.
const DETECT_EVENT_WINDOW_DAYS: i64 = 7;

/// The outcome of a pure detection pass: the flags the current state implies, plus how many
/// calendar events were skipped for having no iCal `uid` (no stable anchor to hang a flag on).
/// The skip count is surfaced, never silently swallowed — a caller logs it (no silent cap).
#[derive(Debug, Default)]
pub struct Detection {
    pub drafts: Vec<DraftFlag>,
    pub skipped_no_uid: usize,
}

/// Derive the flag set the current state implies — PURE, so it unit-tests without a DB or clock
/// (mirroring [`crate::projects::derive_status`] / [`crate::milestones::governing`]). Two families:
///
/// - **Milestone-anchored** (`deadline-approaching` / `overdue`), one per UNMET dated milestone:
///   `overdue` once its date is past, else `deadline-approaching` within the type's default lead.
///   Each milestone is its own anchor (its stable id), so resolving one never touches another.
/// - **Calendar-anchored** (`happening-today` / `prepare-ahead`), one per event SERIES: recurrences
///   share a `uid`, so each `uid` is collapsed to its soonest instance and classified once (a daily
///   standup is one flag, not one per day). An event with no `uid` can't be stably anchored, so it
///   is counted in `skipped_no_uid` and dropped rather than anchored on a volatile row id.
///
/// Detection only ever proposes `active` drafts (it never resolves). Artifact matching (a confident
/// prep-doc → `artifact_ptr`) is a later refinement, so drafts carry no pointer yet. The returned
/// drafts are sorted for a deterministic order (the stored set keys on `(anchor,type)` regardless).
pub fn detect(projects: &[ProjectOverview], events: &[CalendarEvent], today: &str) -> Detection {
    let mut out = Detection::default();

    // Milestone-anchored: one flag per unmet, dated milestone inside its lead window.
    for p in projects {
        for m in &p.milestones {
            if m.is_met() {
                continue;
            }
            let Some(due) = m.due_date.as_deref() else {
                continue;
            };
            let Some(days) = milestones::days_until(today, due) else {
                continue; // unparseable date → not flaggable, never silently "due"
            };
            let flag_type = if days < 0.0 {
                TYPE_OVERDUE
            } else if days <= default_threshold_days(TYPE_DEADLINE_APPROACHING) {
                TYPE_DEADLINE_APPROACHING
            } else {
                continue; // still comfortably ahead — no flag yet
            };
            out.drafts
                .push(draft_flag(ANCHOR_MILESTONE, &m.id.to_string(), flag_type));
        }
    }

    // Calendar-anchored: collapse each uid to its soonest upcoming instance, then classify once.
    let mut soonest: HashMap<&str, f64> = HashMap::new();
    for e in events {
        let Some(uid) = e.uid.as_deref() else {
            out.skipped_no_uid += 1;
            continue;
        };
        let Some(days) = milestones::days_until(today, &e.start) else {
            continue;
        };
        soonest
            .entry(uid)
            .and_modify(|cur| {
                if days < *cur {
                    *cur = days;
                }
            })
            .or_insert(days);
    }
    for (uid, days) in soonest {
        let flag_type = if days <= 0.0 {
            TYPE_HAPPENING_TODAY
        } else if days <= default_threshold_days(TYPE_PREPARE_AHEAD) {
            TYPE_PREPARE_AHEAD
        } else {
            continue; // beyond the prepare-ahead lead — still just an agenda item
        };
        out.drafts.push(draft_flag(ANCHOR_CALENDAR, uid, flag_type));
    }

    out.drafts.sort_by(|a, b| {
        (&a.anchor_kind, &a.r#type, &a.anchor).cmp(&(&b.anchor_kind, &b.r#type, &b.anchor))
    });
    out
}

/// A detection-proposed draft: an `active` flag with no artifact pointer yet.
fn draft_flag(anchor_kind: &str, anchor: &str, flag_type: &str) -> DraftFlag {
    DraftFlag {
        anchor_kind: anchor_kind.into(),
        anchor: anchor.into(),
        r#type: flag_type.into(),
        threshold: None,
        artifact_ptr: None,
        artifact_url: None,
    }
}

/// What a detection run reports for the log line: how many flags are active afterward, how many
/// stale detection flags were pruned, and how many calendar events had no anchor.
#[derive(Debug, Default)]
pub struct DetectionSummary {
    pub active: usize,
    pub pruned: usize,
    pub skipped_no_uid: usize,
}

/// Run detection over ALREADY-FETCHED inputs and reconcile the stored flag set to it, in one
/// transaction. The briefing path uses this (it already holds `projects` + `events`), so the
/// rendered briefing joins against the exact same snapshot detection saw.
///
/// **Reconcile, not just insert.** After upserting every drafted flag, any *detection-owned* active
/// flag whose `(anchor_kind, anchor, type)` was NOT re-proposed this pass is deleted — its condition
/// no longer holds (the event passed, the milestone was met or slipped, the deadline flipped to
/// `overdue` under a new key). This is what a time-based `gc_expired` stood for, generalised: a
/// passed or withdrawn anchor simply stops being re-emitted. RESOLVED flags (any source) are never
/// touched — resolution is durable state downstream flags read (decision 3), not something a rescan
/// may undo (decision 1). The prune is scoped to `state='active' AND user_confirmed=0`, so a
/// user-vouched or resolved flag is always protected.
pub fn detect_and_store(
    conn: &Connection,
    projects: &[ProjectOverview],
    events: &[CalendarEvent],
    today: &str,
) -> Result<DetectionSummary> {
    let det = detect(projects, events, today);
    let emitted: HashSet<(String, String, String)> = det
        .drafts
        .iter()
        .map(|d| (d.anchor_kind.clone(), d.anchor.clone(), d.r#type.clone()))
        .collect();

    let tx = conn.unchecked_transaction()?;
    for d in &det.drafts {
        upsert_active(&tx, d)?;
    }
    // Collect the detection-owned active flags the current state no longer implies (stmt is scoped
    // so it's dropped before the delete loop reborrows `tx`).
    let stale: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id, anchor_kind, anchor, type FROM flags \
             WHERE state = 'active' AND user_confirmed = 0",
        )?;
        let rows: Vec<(i64, (String, String, String))> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ),
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .filter(|(_, key)| !emitted.contains(key))
            .map(|(id, _)| id)
            .collect()
    };
    for id in &stale {
        tx.execute("DELETE FROM flags WHERE id = ?1", params![id])?;
    }
    tx.commit()?;

    Ok(DetectionSummary {
        active: list_active(conn, None)?.len(),
        pruned: stale.len(),
        skipped_no_uid: det.skipped_no_uid,
    })
}

/// Fetch the focus-view inputs, then [`detect_and_store`] — the entry the background scheduler uses
/// (it has nothing pre-fetched). The briefing path calls `detect_and_store` directly with the
/// inputs it already holds. Both fetch the same event window and share the same reducer, so they
/// agree on the flag set.
pub fn run_detection(conn: &Connection, today: &str) -> Result<DetectionSummary> {
    let projects = projects::list_overviews(conn, today)?;
    let events = calendar::list_upcoming(conn, DETECT_EVENT_WINDOW_DAYS)?;
    detect_and_store(conn, &projects, &events, today)
}

/// RFC3339 timestamp of the last flag-detection pass; stamped on success so the cadence survives
/// restarts (mirrors [`crate::project_activity::LAST_ROLLUP_AT_KEY`]).
pub const LAST_FLAG_SCAN_AT_KEY: &str = "last_flag_scan_at";

/// Backstop tick; detection also runs synchronously on every briefing refresh, so this only has to
/// catch the app being left open past a day boundary (a `deadline-approaching` should become
/// `overdue`) without a refresh in between.
const TICK_SECS: u64 = 3_600;
/// Only scan once the user has been idle this long — politeness; the pass is cheap.
const IDLE_THRESHOLD_SECS: u64 = 60;
/// Re-scan at most this often from the backstop.
const SCAN_INTERVAL_HOURS: i64 = 6;

/// Whether a backstop scan is due: never run, or the interval has elapsed. Pure (unit-tested
/// without a clock), mirroring [`crate::project_activity`]'s `rollup_due`.
fn scan_due(last_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last_at {
        None => true,
        Some(last) => now.signed_duration_since(last) >= ChronoDuration::hours(SCAN_INTERVAL_HOURS),
    }
}

/// Idle-gated backstop that keeps the flag set current even when the briefing isn't refreshed for a
/// while. Spawned once from `setup` alongside the other schedulers; gated on unlocked + idle +
/// not-mid-sync (so it never reconciles against a half-synced calendar mirror), and — like every
/// scheduler here — never holds the DB guard across an `.await` (repo rule #4).
pub fn spawn_flag_detection_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let threshold = Duration::from_secs(IDLE_THRESHOLD_SECS);
        loop {
            tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;

            // Gate on idle/sync first (no lock), then take the DB guard. Everything below is
            // synchronous, so the guard is never held across an `.await` (repo rule #4).
            let state = app.state::<AppState>();
            if state.idle_for() < threshold || state.sync_active() {
                continue;
            }
            let Ok(conn) = state.conn() else { continue };

            let last_at = db::get_setting(&conn, LAST_FLAG_SCAN_AT_KEY)
                .ok()
                .flatten()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc));
            let now = Utc::now();
            if !scan_due(last_at, now) {
                continue;
            }
            let zone = crate::commands::resolve_zone(&conn);
            let today = clock::today_sql_in(zone);
            match run_detection(&conn, &today) {
                Ok(s) => {
                    if s.skipped_no_uid > 0 {
                        eprintln!(
                            "flag detection: {} active, {} pruned, {} calendar event(s) skipped (no uid)",
                            s.active, s.pruned, s.skipped_no_uid
                        );
                    }
                    let _ = db::set_setting(&conn, LAST_FLAG_SCAN_AT_KEY, &now.to_rfc3339());
                }
                Err(e) => eprintln!("flag detection skipped: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh encrypted store at the latest schema (the tempdir must outlive the connection).
    fn open_test_db() -> (tempfile::TempDir, Connection) {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn draft(anchor: &str, flag_type: &str) -> DraftFlag {
        DraftFlag {
            anchor_kind: ANCHOR_MILESTONE.into(),
            anchor: anchor.into(),
            r#type: flag_type.into(),
            threshold: None,
            artifact_ptr: None,
            artifact_url: None,
        }
    }

    #[test]
    fn upsert_inserts_then_refreshes_the_same_active_row() {
        let (_dir, conn) = open_test_db();
        let id = upsert_active(&conn, &draft("7", TYPE_DEADLINE_APPROACHING)).unwrap();

        // A second upsert on the same (anchor_kind, anchor, type) reuses the row and refreshes
        // its detection-derived fields rather than creating a duplicate.
        let mut d = draft("7", TYPE_DEADLINE_APPROACHING);
        d.artifact_ptr = Some("gdrive:me@x.com:abc".into());
        d.artifact_url = Some("https://drive/abc".into());
        let id2 = upsert_active(&conn, &d).unwrap();
        assert_eq!(id, id2, "same key upserts in place");

        let f = get(&conn, id).unwrap().unwrap();
        assert_eq!(f.artifact_ptr.as_deref(), Some("gdrive:me@x.com:abc"));
        assert_eq!(f.state, STATE_ACTIVE);
        assert_eq!(list_active(&conn, None).unwrap().len(), 1, "no duplicate");
    }

    #[test]
    fn upsert_never_unresolves_a_resolved_flag() {
        let (_dir, conn) = open_test_db();
        let id = upsert_active(&conn, &draft("7", TYPE_PREPARE_AHEAD)).unwrap();
        resolve(&conn, id, SOURCE_ASSERTION, Some("gdrive:me@x.com:prep")).unwrap();

        // Re-detection (the daily rescan) must NOT flip a resolved flag back to active or clobber
        // its user-asserted resolution — the storage-level guard behind decision 1's idempotency.
        let mut d = draft("7", TYPE_PREPARE_AHEAD);
        d.artifact_ptr = Some("gdrive:me@x.com:OTHER".into());
        let id2 = upsert_active(&conn, &d).unwrap();
        assert_eq!(id, id2);

        let f = get(&conn, id).unwrap().unwrap();
        assert_eq!(f.state, STATE_RESOLVED, "stays resolved");
        assert_eq!(
            f.source.as_deref(),
            Some(SOURCE_ASSERTION),
            "source untouched"
        );
        assert!(f.user_confirmed, "user vouch preserved");
        assert_eq!(
            f.artifact_ptr.as_deref(),
            Some("gdrive:me@x.com:prep"),
            "the asserted artifact is not overwritten by detection"
        );
        assert!(
            list_active(&conn, None).unwrap().is_empty(),
            "a resolved flag is out of the active set the briefing renders"
        );
    }

    #[test]
    fn resolve_records_source_and_derives_user_confirmed() {
        let (_dir, conn) = open_test_db();
        let asserted = upsert_active(&conn, &draft("1", TYPE_OVERDUE)).unwrap();
        let detected = upsert_active(&conn, &draft("2", TYPE_OVERDUE)).unwrap();

        resolve(&conn, asserted, SOURCE_ASSERTION, None).unwrap();
        resolve(&conn, detected, SOURCE_DETECTION, None).unwrap();

        let a = get(&conn, asserted).unwrap().unwrap();
        assert!(a.user_confirmed, "assertion is a confirmed vouch");
        assert!(a.resolved_at.is_some());

        let d = get(&conn, detected).unwrap().unwrap();
        assert!(
            !d.user_confirmed,
            "a detection verdict is unconfirmed until HITL confirms it"
        );
    }

    #[test]
    fn list_active_filters_by_state_and_anchor_kind() {
        let (_dir, conn) = open_test_db();
        upsert_active(&conn, &draft("9", TYPE_DEADLINE_APPROACHING)).unwrap(); // milestone
        let cal = DraftFlag {
            anchor_kind: ANCHOR_CALENDAR.into(),
            anchor: "uid-123".into(),
            r#type: TYPE_HAPPENING_TODAY.into(),
            threshold: None,
            artifact_ptr: None,
            artifact_url: None,
        };
        upsert_active(&conn, &cal).unwrap();

        assert_eq!(
            list_active(&conn, None).unwrap().len(),
            2,
            "both kinds active"
        );
        assert_eq!(
            list_active(&conn, Some(ANCHOR_MILESTONE)).unwrap().len(),
            1,
            "kind filter narrows to milestone-anchored"
        );
    }

    #[test]
    fn default_threshold_days_per_type() {
        assert_eq!(default_threshold_days(TYPE_DEADLINE_APPROACHING), 7.0);
        assert_eq!(default_threshold_days(TYPE_PREPARE_AHEAD), 3.0);
        assert_eq!(default_threshold_days(TYPE_HAPPENING_TODAY), 0.0);
        assert_eq!(default_threshold_days(TYPE_OVERDUE), 0.0);
    }

    // --- detection ---

    use crate::calendar::CalendarEvent;
    use crate::milestones::Milestone;
    use crate::projects::{ProjectOverview, ProjectStatus};

    const TODAY: &str = "2026-07-03";

    fn ms(id: i64, due: Option<&str>, met: bool) -> Milestone {
        Milestone {
            id,
            project_name: "Atlas".into(),
            label: format!("m{id}"),
            due_date: due.map(|s| s.into()),
            event_uid: None,
            calendar_linked: false,
            event_missing: false,
            state: Some(if met { "met" } else { "unmet" }.into()),
            sort_order: id,
        }
    }

    fn overview(milestones: Vec<Milestone>) -> ProjectOverview {
        ProjectOverview {
            name: "Atlas".into(),
            status: ProjectStatus::OnTrack,
            doc_count: 1,
            last_activity: None,
            deadline: None,
            size: None,
            blocked_by: None,
            parent: None,
            importance: None,
            auto_importance: None,
            calendar_event: None,
            milestones,
            governing_milestone: None,
        }
    }

    fn cal_event(uid: Option<&str>, start: &str) -> CalendarEvent {
        CalendarEvent {
            id: format!("c:{start}"),
            calendar_id: "c".into(),
            summary: "Standup".into(),
            description: None,
            location: None,
            start: start.into(),
            end: None,
            all_day: false,
            html_link: None,
            uid: uid.map(|s| s.into()),
        }
    }

    /// Milestones anchor deadline/overdue flags; calendar events anchor today/prepare-ahead. A
    /// milestone comfortably ahead, a met one, and a date-less one produce no flag.
    #[test]
    fn detect_maps_milestones_and_calendar_to_the_right_types() {
        let projects = vec![overview(vec![
            ms(10, Some("2026-06-20"), false), // 13 days past -> overdue
            ms(11, Some("2026-07-06"), false), // +3 -> deadline-approaching
            ms(12, Some("2026-09-01"), false), // far off -> no flag
            ms(13, Some("2026-07-04"), true),  // met -> no flag
            ms(14, None, false),               // date-less -> no flag
        ])];
        let events = vec![
            cal_event(Some("uid-today"), "2026-07-03T15:00:00Z"), // today
            cal_event(Some("uid-prep"), "2026-07-05T09:00:00Z"),  // +2 -> prepare-ahead
            cal_event(Some("uid-far"), "2026-07-30T09:00:00Z"),   // beyond lead -> no flag
        ];

        let det = detect(&projects, &events, TODAY);
        let got: Vec<(&str, &str, &str)> = det
            .drafts
            .iter()
            .map(|d| (d.anchor_kind.as_str(), d.anchor.as_str(), d.r#type.as_str()))
            .collect();

        assert!(got.contains(&(ANCHOR_MILESTONE, "10", TYPE_OVERDUE)));
        assert!(got.contains(&(ANCHOR_MILESTONE, "11", TYPE_DEADLINE_APPROACHING)));
        assert!(got.contains(&(ANCHOR_CALENDAR, "uid-today", TYPE_HAPPENING_TODAY)));
        assert!(got.contains(&(ANCHOR_CALENDAR, "uid-prep", TYPE_PREPARE_AHEAD)));
        assert_eq!(
            got.len(),
            4,
            "comfortably-ahead / met / date-less / far events don't flag"
        );
        assert_eq!(det.skipped_no_uid, 0);
    }

    /// A calendar event with no iCal uid can't be anchored — it's counted, not silently dropped,
    /// and a recurring series (one uid, many instances) collapses to a single flag on its soonest.
    #[test]
    fn detect_skips_uidless_events_and_dedupes_a_series() {
        let events = vec![
            cal_event(None, "2026-07-03T15:00:00Z"),
            cal_event(None, "2026-07-04T15:00:00Z"),
            cal_event(Some("uid-daily"), "2026-07-05T09:00:00Z"), // +2 prepare-ahead
            cal_event(Some("uid-daily"), "2026-07-03T09:00:00Z"), // today — soonest instance wins
        ];
        let det = detect(&[], &events, TODAY);
        assert_eq!(det.skipped_no_uid, 2, "both uid-less events counted");
        assert_eq!(det.drafts.len(), 1, "series collapses to one flag");
        assert_eq!(
            det.drafts[0].r#type, TYPE_HAPPENING_TODAY,
            "classified on the soonest instance"
        );
    }

    /// The executor reconciles: flags whose condition no longer holds are pruned, while a RESOLVED
    /// flag survives untouched even though detection would still propose it.
    #[test]
    fn detect_and_store_reconciles_and_preserves_resolved() {
        let (_dir, conn) = open_test_db();

        // First pass: two overdue milestones + one today event → three active flags.
        let p1 = overview(vec![
            ms(10, Some("2026-06-01"), false),
            ms(11, Some("2026-06-02"), false),
        ]);
        let e1 = vec![cal_event(Some("uid-a"), "2026-07-03T09:00:00Z")];
        let s1 = detect_and_store(&conn, &[p1], &e1, TODAY).unwrap();
        assert_eq!(s1.active, 3);
        assert_eq!(s1.pruned, 0);

        // The user asserts milestone 10's flag done.
        let m10 = list_active(&conn, Some(ANCHOR_MILESTONE))
            .unwrap()
            .into_iter()
            .find(|f| f.anchor == "10")
            .unwrap();
        resolve(&conn, m10.id, SOURCE_ASSERTION, None).unwrap();

        // Second pass: milestone 11 is now met (drops out) and the event is gone. Milestone 10 is
        // still unmet+overdue, so detection re-proposes it — but it's resolved, so it stays resolved.
        let p2 = overview(vec![
            ms(10, Some("2026-06-01"), false),
            ms(11, Some("2026-06-02"), true),
        ]);
        let s2 = detect_and_store(&conn, &[p2], &[], TODAY).unwrap();
        assert_eq!(s2.pruned, 2, "met milestone + vanished event pruned");
        assert_eq!(
            s2.active, 0,
            "nothing active — 11 & event gone, 10 is resolved"
        );

        let f10 = get(&conn, m10.id).unwrap().unwrap();
        assert_eq!(
            f10.state, STATE_RESOLVED,
            "a resolved flag is never reconciled away"
        );
        assert!(f10.user_confirmed);
    }

    #[test]
    fn scan_due_respects_the_interval() {
        let now = DateTime::parse_from_rfc3339("2026-07-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(scan_due(None, now), "never run → due");
        assert!(
            !scan_due(Some(now - ChronoDuration::hours(1)), now),
            "within interval → not due"
        );
        assert!(
            scan_due(Some(now - ChronoDuration::hours(7)), now),
            "past interval → due"
        );
    }
}
