// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Project Activity Log (Stage 3): an append-only, name-keyed, EMIT-ONLY engagement
//! record. Every meaningful engagement with a project — a message in its scoped chat, a
//! document filed into it, a milestone edit — appends one `project_activity` row keyed on
//! `projects(name)`. Rows are OBSERVATIONS, not scores: they carry no weight; a future
//! Stage-4 heat scorer maps [`Kind`] → weight at READ time. NOTHING reads this log yet.
//!
//! Writing is best-effort (mirrors [`crate::commands`]'s `log_usage`): logging must never
//! fail the primary op, so [`record`] swallows its errors. Name-keying — not `entity_id` —
//! is deliberate: `projects.name` is the identity every project surface already uses
//! (`projects::touch`, `project_milestones.project_name`, `conversations.project`), whereas
//! `entity_id` is nullable and NULL until a document resolves it. A never-triaged project
//! still logs: [`record`] lazily ensures the parent `projects` row (mirroring
//! `milestones::add`), so a name is all that's required.
//!
//! Retention is baked in (so the log never grows unbounded like `usage_log`): raw rows are kept
//! for a recent window ([`RAW_WINDOW_DAYS`]), then compacted into per-(project, day, kind) counts
//! in `project_activity_daily` and pruned by [`run_rollup`], which [`spawn_rollup_scheduler`]
//! drives about once a day when the vault is unlocked and the user is idle.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection};
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::{db, AppState};

/// The engagement discriminator, at call-site granularity. A closed enum so call sites can't
/// typo the stored string, and so the `CHECK (kind IN (...))` constraint on `project_activity`
/// (migration v31) and the code stay in lockstep. New variants are added together with the
/// emit site that produces them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A message sent in a project-scoped chat.
    Chat,
    /// A document filed into a real project (assigned at organize time — NOT raw byte ingest,
    /// where a document is always `Unsorted` and hasn't been attributed to a project yet).
    Ingest,
    /// A milestone added, edited, re-linked, marked, deleted, or reordered on a project.
    Milestone,
}

impl Kind {
    /// The `kind` column value. Must stay within the v31 `CHECK` list ('chat','ingest','milestone').
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Chat => "chat",
            Kind::Ingest => "ingest",
            Kind::Milestone => "milestone",
        }
    }
}

/// Append one engagement observation for `project`. Best-effort (mirrors `log_usage`): a failure
/// here must never fail the caller's primary op, so every error is swallowed. The FK parent row is
/// ensured lazily so a never-triaged project (entity_id NULL) still logs. A blank name is a no-op
/// (mirrors `projects::touch`). `source_ref` is a free-form back-pointer (document / conversation /
/// milestone id), NULL where none applies — it is NOT a foreign key, so a later-deleted target
/// leaves the historical observation intact.
pub fn record(conn: &Connection, project: &str, kind: Kind, source_ref: Option<i64>) {
    let project = project.trim();
    if project.is_empty() {
        return;
    }
    let _ = conn.execute(
        "INSERT INTO projects(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        params![project],
    );
    let _ = conn.execute(
        "INSERT INTO project_activity(project, kind, source_ref) VALUES (?1, ?2, ?3)",
        params![project, kind.as_str(), source_ref],
    );
}

// --- retention / rollup ----------------------------------------------------------------------

/// Raw events younger than this stay in `project_activity`; older ones are compacted into
/// `project_activity_daily` and pruned. A placeholder window (~30d) that comfortably exceeds any
/// day-scale heat half-life a Stage-4 scorer will use — a const, not a user setting, this stage.
pub const RAW_WINDOW_DAYS: i64 = 30;

const SECS_PER_DAY: i64 = 86_400;

/// Compact every raw event older than the recent window into per-(project, day, kind) counts, then
/// prune the rolled raw rows — both in ONE transaction, so a crash between them rolls the whole
/// thing back (never a double count). Idempotent + re-run safe: rolled rows are deleted, so a
/// re-run finds nothing to add. The upsert is ADDITIVE (`count + excluded.count`) because a day
/// straddling the moving cutoff is rolled across two passes — the later pass adds the remainder.
pub fn run_rollup(conn: &Connection, now_unix: i64) -> Result<()> {
    let cutoff = now_unix - RAW_WINDOW_DAYS * SECS_PER_DAY;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO project_activity_daily(project, day, kind, count) \
         SELECT project, occurred_at / 86400 AS day, kind, COUNT(*) \
           FROM project_activity \
          WHERE occurred_at < ?1 \
          GROUP BY project, day, kind \
         ON CONFLICT(project, day, kind) DO UPDATE SET count = count + excluded.count",
        params![cutoff],
    )?;
    tx.execute(
        "DELETE FROM project_activity WHERE occurred_at < ?1",
        params![cutoff],
    )?;
    tx.commit()?;
    Ok(())
}

// --- scheduler (mirrors backup::schedule) ----------------------------------------------------

/// RFC3339 timestamp of the last successful rollup; stamped on success so the daily cadence is
/// honest across restarts (mirrors `backup::schedule::LAST_BACKUP_AT_KEY`).
pub const LAST_ROLLUP_AT_KEY: &str = "last_activity_rollup_at";

/// Hourly backstop tick; the rollup is tiny, but the cadence it enforces is daily.
const TICK_SECS: u64 = 3_600;
/// Only roll up once the user has been idle this long — politeness; the job is cheap.
const IDLE_THRESHOLD_SECS: u64 = 60;
/// Roll up at most once a day.
const ROLLUP_INTERVAL_HOURS: i64 = 24;

/// Whether a rollup is due now: never run, or the daily interval has elapsed. Pure, so the cadence
/// is unit-tested without a clock (mirrors `backup_due`).
fn rollup_due(last_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last_at {
        None => true,
        Some(last) => {
            now.signed_duration_since(last) >= ChronoDuration::hours(ROLLUP_INTERVAL_HOURS)
        }
    }
}

/// Idle-gated daily maintenance: compact the activity log's raw window into daily counts and prune
/// it. Spawned once from `setup` alongside the other background schedulers; mirrors
/// `backup::schedule::spawn_backup_scheduler` but far cheaper, so it needs no launch catch-up and
/// only a light idle gate. A no-op until there are events older than the window.
pub fn spawn_rollup_scheduler(app: AppHandle) {
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

            let last_at = db::get_setting_time(&conn, LAST_ROLLUP_AT_KEY);
            let now = Utc::now();
            if !rollup_due(last_at, now) {
                continue;
            }
            match run_rollup(&conn, now.timestamp()) {
                Ok(()) => {
                    let _ = db::set_setting(&conn, LAST_ROLLUP_AT_KEY, &now.to_rfc3339());
                }
                Err(e) => eprintln!("activity rollup skipped: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    /// A recorded event lands with the right kind + ref, and the parent project row is created
    /// lazily so a never-triaged project can still be logged.
    #[test]
    fn record_appends_and_lazily_creates_the_parent_project() {
        let (_dir, conn) = store();

        // "Fresh" does not exist yet — record must mint it, then append the observation.
        record(&conn, "Fresh", Kind::Chat, Some(7));

        let project_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE name = 'Fresh'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(project_exists, 1, "parent project ensured lazily");

        let (project, kind, source_ref): (String, String, Option<i64>) = conn
            .query_row(
                "SELECT project, kind, source_ref FROM project_activity",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(project, "Fresh");
        assert_eq!(kind, "chat");
        assert_eq!(source_ref, Some(7));
    }

    /// A blank / whitespace-only project name is a no-op (mirrors `projects::touch`), and a NULL
    /// `source_ref` is allowed.
    #[test]
    fn blank_name_is_a_noop_and_null_ref_is_allowed() {
        let (_dir, conn) = store();

        record(&conn, "   ", Kind::Chat, Some(1));
        record(&conn, "", Kind::Chat, None);
        let after_blank: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_activity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_blank, 0, "blank names record nothing");

        record(&conn, "Atlas", Kind::Chat, None);
        let null_ref: Option<i64> = conn
            .query_row(
                "SELECT source_ref FROM project_activity WHERE project = 'Atlas'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_ref, None, "a NULL back-pointer is allowed");
    }

    /// Dropping a project cascades its activity rows away (proves `ON DELETE CASCADE` fires —
    /// i.e. `PRAGMA foreign_keys = ON` is set on the connection).
    #[test]
    fn deleting_a_project_cascades_its_activity() {
        let (_dir, conn) = store();
        record(&conn, "Doomed", Kind::Chat, Some(1));
        record(&conn, "Doomed", Kind::Chat, Some(2));

        conn.execute("DELETE FROM projects WHERE name = 'Doomed'", [])
            .unwrap();

        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_activity WHERE project = 'Doomed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "ON DELETE CASCADE removed the activity rows");
    }

    // --- rollup / retention ---

    const DAY: i64 = 86_400;

    fn seed_event(conn: &Connection, project: &str, kind: &str, occurred_at: i64) {
        conn.execute(
            "INSERT INTO projects(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            params![project],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_activity(project, kind, occurred_at) VALUES (?1, ?2, ?3)",
            params![project, kind, occurred_at],
        )
        .unwrap();
    }

    fn daily(conn: &Connection) -> Vec<(String, i64, String, i64)> {
        let mut s = conn
            .prepare(
                "SELECT project, day, kind, count FROM project_activity_daily ORDER BY day, kind",
            )
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }

    /// Events older than the raw window roll up into per-(project, day, kind) counts — where `day`
    /// is the UTC unix-day (`occurred_at / 86400`) — and the raw rows are pruned; recent events stay.
    #[test]
    fn run_rollup_compacts_old_events_and_keeps_recent_ones() {
        let (_dir, conn) = store();
        let now = 100 * DAY; // cutoff = day 70
                             // Old (rolled): 2 chat + 1 ingest on day 10, 1 chat on day 11 — all Atlas.
        seed_event(&conn, "Atlas", "chat", 10 * DAY);
        seed_event(&conn, "Atlas", "chat", 10 * DAY + 5);
        seed_event(&conn, "Atlas", "ingest", 10 * DAY + 9);
        seed_event(&conn, "Atlas", "chat", 11 * DAY);
        // Recent (kept): a chat on day 90.
        seed_event(&conn, "Atlas", "chat", 90 * DAY);

        run_rollup(&conn, now).unwrap();

        assert_eq!(
            daily(&conn),
            vec![
                ("Atlas".to_string(), 10, "chat".to_string(), 2),
                ("Atlas".to_string(), 10, "ingest".to_string(), 1),
                ("Atlas".to_string(), 11, "chat".to_string(), 1),
            ]
        );
        // Only the recent (day-90) raw row survives.
        let raw: Vec<i64> = {
            let mut s = conn
                .prepare("SELECT occurred_at FROM project_activity")
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(raw, vec![90 * DAY]);
    }

    /// A second pass is a no-op (rolled rows are deleted), and a later batch for the same
    /// (project, day, kind) ADDS to the existing daily count (the straddle case).
    #[test]
    fn run_rollup_is_idempotent_and_additive() {
        let (_dir, conn) = store();
        let now = 100 * DAY;
        seed_event(&conn, "Atlas", "chat", 10 * DAY);
        seed_event(&conn, "Atlas", "chat", 10 * DAY + 1);

        run_rollup(&conn, now).unwrap();
        run_rollup(&conn, now).unwrap(); // idempotent — nothing new to roll
        assert_eq!(
            daily(&conn),
            vec![("Atlas".to_string(), 10, "chat".to_string(), 2)]
        );

        // A later batch for the same bucket accumulates rather than replacing.
        seed_event(&conn, "Atlas", "chat", 10 * DAY + 2);
        seed_event(&conn, "Atlas", "chat", 10 * DAY + 3);
        seed_event(&conn, "Atlas", "chat", 10 * DAY + 4);
        run_rollup(&conn, now).unwrap();
        assert_eq!(
            daily(&conn),
            vec![("Atlas".to_string(), 10, "chat".to_string(), 5)]
        );
    }

    /// The daily cadence gate: never-run is due; inside 24h is not; past 24h is.
    #[test]
    fn rollup_due_respects_the_daily_interval() {
        let now = DateTime::parse_from_rfc3339("2026-07-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(rollup_due(None, now), "never run → due");
        assert!(
            !rollup_due(Some(now - ChronoDuration::hours(23)), now),
            "within a day → not due"
        );
        assert!(
            rollup_due(Some(now - ChronoDuration::hours(25)), now),
            "past a day → due"
        );
    }
}
