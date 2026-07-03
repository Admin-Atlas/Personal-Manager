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
//! **PR1 scope (this file):** the schema, the typed vocabulary, and the CRUD seam — nothing
//! detects, renders, or resolves yet. Detection (a pure reducer over calendar + milestones)
//! arrives with the briefing rewrite; the assertion/HITL rules and chat grounding follow.
//! Until a consumer wires in, this public surface is exercised only by the unit tests below
//! — hence the `#[allow(dead_code)]` on the module declaration in `lib.rs`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::Result;

// Which identity space a flag's `anchor` string lives in.
pub const ANCHOR_CALENDAR: &str = "calendar";
pub const ANCHOR_MILESTONE: &str = "milestone";

// The flag taxonomy. `(anchor, type)` is the resolution key.
pub const TYPE_PREPARE_AHEAD: &str = "prepare-ahead";
pub const TYPE_DEADLINE_APPROACHING: &str = "deadline-approaching";
pub const TYPE_HAPPENING_TODAY: &str = "happening-today";
pub const TYPE_OVERDUE: &str = "overdue";

// Lifecycle.
pub const STATE_ACTIVE: &str = "active";
pub const STATE_RESOLVED: &str = "resolved";

// Which path CLOSED a flag (NULL while active). On conflict, assertion outranks detection.
pub const SOURCE_DETECTION: &str = "detection";
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

/// One flag by id, or `None` if the id is unknown.
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
}
