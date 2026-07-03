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
//! Retention (raw rows compacted into per-day counts after a recent window, then pruned)
//! lands alongside the rollup job; this module currently just appends.

use rusqlite::{params, Connection};

/// The engagement discriminator, at call-site granularity. A closed enum so call sites can't
/// typo the stored string, and so the `CHECK (kind IN (...))` constraint on `project_activity`
/// (migration v31) and the code stay in lockstep. New variants are added together with the
/// emit site that produces them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A message sent in a project-scoped chat.
    Chat,
}

impl Kind {
    /// The `kind` column value. Must stay within the v31 `CHECK` list ('chat','ingest','milestone').
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Chat => "chat",
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
}
