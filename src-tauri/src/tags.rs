// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tag registry (#275) — one table for both kinds of tag, and the join that binds a document
//! to them.
//!
//! The organising idea is Bobby's: **every project IS a tag**. A document belonging to several
//! projects then falls out of ordinary multi-tagging instead of needing its own many-to-many
//! machinery, and one `@tag` grammar can later reach both (#276). The `kind` column keeps the two
//! populations apart where they genuinely differ:
//!
//! - `kind = 'project'` rows mirror a real project. Their names keep the user's **verbatim
//!   casing**, because `projects.name` is a primary key and `entities.canonical_name` is the alias
//!   key — force-lowercasing here would collide with both. Only these rows touch the entity/alias
//!   space.
//! - `kind = 'group'` is the banked second kind: the free-form lowercase labels the tag editor
//!   already writes into `documents.tags` and the vault's `tags:` line. **Nothing populates it
//!   yet.** Those labels are inert today — no retrieval, search, filter or score reads them — so
//!   migrating them now would move a population with no consumer. They land with #276, which is
//!   the first thing that will actually read a tag. The `kind` column exists now because adding it
//!   later would mean a second migration over the same table (the `calendar_events.kind_override`
//!   precedent in v45).
//!
//! Matching is case-insensitive, via a stored `norm` column rather than `COLLATE NOCASE` — the
//! same shape `preferences` already uses, and for the same reason recorded there: SQLite's
//! `lower()` is ASCII-only with no ICU, so the normalisation has to be visible and identical on
//! both sides rather than hidden in a collation.
//!
//! **Membership keys on tag id, never on the name string.** Re-keying a name across a dozen tables
//! is exactly the failure the entity layer was built to end; a rename here moves one row.
//!
//! The join is a *derived* index over what the vault already says (`project:` +
//! `linked_projects:` front-matter, or the index-only manifest). [`crate::ingest`]'s
//! `write_document_truth` is the only writer, which is what keeps the two from drifting — see
//! INVARIANTS I-02.

use rusqlite::{params, Connection};
use std::collections::HashMap;

use crate::error::Result;

/// A tag that mirrors a real project. Only these reach the entity/alias space, and only these are
/// populated today — see the module doc for why `group` is banked rather than backfilled.
pub const KIND_PROJECT: &str = "project";

/// A subquery naming every `(document_id, name, norm)` project membership in the store.
///
/// It is the UNION of two things, and the asymmetry is deliberate: a document's HOME comes straight
/// from `documents.project`, and only its EXTRA memberships come from the join.
///
/// The join is a derived index, and derived state can be behind — a store part-way through a
/// migration, one restored from a backup taken before a Rebuild, a code path that writes the column
/// without going through [`crate::ingest::write_document_truth`]. If the roster, the file lists and
/// the retrieval scope all read the join alone, any of those makes a project's own documents
/// silently vanish from the project. Deriving the home from the column it is stored in means the
/// join can only ever ADD memberships, never authorise the ones that were always there.
///
/// The `NOT EXISTS` guard keeps a document from appearing twice when the join already holds its
/// home under different casing — which would otherwise inflate its project's file count.
pub const MEMBERSHIPS_SQL: &str = "SELECT d.id AS document_id, d.project AS name, \
            lower(trim(d.project)) AS norm \
     FROM documents d \
     WHERE trim(COALESCE(d.project,'')) <> '' \
       AND NOT EXISTS (SELECT 1 FROM document_tags dt JOIN tags t ON t.id = dt.tag_id \
                       WHERE dt.document_id = d.id AND t.kind = 'project' \
                         AND t.norm = lower(trim(d.project))) \
     UNION ALL \
     SELECT dt.document_id, t.name, t.norm \
     FROM document_tags dt JOIN tags t ON t.id = dt.tag_id AND t.kind = 'project'";

/// The matching key for a tag name: trimmed and ASCII-lowercased.
///
/// `to_ascii_lowercase`, NOT `to_lowercase` — this has to agree byte-for-byte with the SQL
/// `lower(trim(...))` the v46 backfill computes, and SQLite's `lower()` is ASCII-only with no ICU.
/// Rust's Unicode-aware version would fold "ÉTÉ" to "été" where SQLite leaves it alone, so one
/// project would be looked up under a key the migration never wrote, mint a second tag row, and
/// split its memberships across both — with the unique index none the wiser, because the two norms
/// genuinely differ. `preferences.rs` learned this and says so at its own normalisation.
///
/// Deliberately NOT applied to the stored `name`: a project displays as the user typed it
/// ("Atlas, Inc.") and matches however they type it next time.
pub fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Find-or-create the tag row for `(kind, name)`, returning its id.
///
/// Find-first, so a project that already exists under different casing keeps its established
/// display name rather than being renamed by whoever typed it most recently. Renaming a project is
/// a deliberate act with its own command; it should not be a side effect of tagging.
pub fn intern(conn: &Connection, kind: &str, name: &str) -> Result<i64> {
    let display = name.trim();
    let norm = normalize(display);
    if norm.is_empty() {
        return Err(crate::error::Error::Other("a tag needs a name".into()));
    }
    if let Some(id) = lookup(conn, kind, &norm)? {
        return Ok(id);
    }
    conn.execute(
        "INSERT OR IGNORE INTO tags (kind, name, norm) VALUES (?1, ?2, ?3)",
        params![kind, display, norm],
    )?;
    // Re-read rather than trusting `last_insert_rowid`: the OR IGNORE above may have been a no-op
    // if a concurrent path interned the same name first, and returning 0 there would bind the
    // document to nothing.
    lookup(conn, kind, &norm)?
        .ok_or_else(|| crate::error::Error::Other("could not intern tag".into()))
}

fn lookup(conn: &Connection, kind: &str, norm: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM tags WHERE kind = ?1 AND norm = ?2",
            params![kind, norm],
            |r| r.get(0),
        )
        .ok())
}

/// Replace a document's whole project membership set — the home plus its other projects — in one
/// pass.
///
/// Called only from `ingest::write_document_truth`, right after the vault (or manifest) truth is
/// written, so the join can never claim something the vault doesn't say. Delete-then-insert rather
/// than a diff: the set is tiny, and a diff has failure modes ("removed everywhere but one place")
/// that a replace does not.
///
/// The home is interned unconditionally, so a project that exists ONLY as somewhere documents are
/// filed still has a registry row for the pickers to offer.
pub fn set_document_projects(
    conn: &Connection,
    doc_id: i64,
    home: &str,
    linked: &[String],
) -> Result<()> {
    conn.execute(
        "DELETE FROM document_tags WHERE document_id = ?1 AND tag_id IN \
         (SELECT id FROM tags WHERE kind = 'project')",
        params![doc_id],
    )?;

    let mut seen: Vec<String> = Vec::new();
    for name in std::iter::once(home).chain(linked.iter().map(String::as_str)) {
        let norm = normalize(name);
        if norm.is_empty() || seen.contains(&norm) {
            continue;
        }
        seen.push(norm);
        let tag_id = intern(conn, KIND_PROJECT, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
            params![doc_id, tag_id],
        )?;
    }
    gc_orphan_project_tags(conn)
}

/// Drop project tags that nothing refers to any more.
///
/// Without this the registry only grows: unlinking the last document from a project would leave the
/// name in every picker forever, and a typo made once would be offered as a real project for the
/// life of the store. A row survives if it still has a membership OR the project has a triage row
/// of its own — a project with a deadline and no documents yet is real and must stay offerable.
fn gc_orphan_project_tags(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM tags WHERE kind = 'project' \
           AND NOT EXISTS (SELECT 1 FROM document_tags dt WHERE dt.tag_id = tags.id) \
           AND NOT EXISTS (SELECT 1 FROM projects p WHERE lower(trim(p.name)) = tags.norm)",
        [],
    )?;
    Ok(())
}

/// A document's project memberships OTHER than `home`, in stable (name) order.
///
/// This is what the vault's `linked_projects:` key holds, so every rewrite path can re-derive the
/// list it must write without threading it through the whole call stack. Comparing on the
/// normalised form means a hand-edited file that repeats the home under different casing still
/// yields "no extras" rather than a document linked to itself.
pub fn linked_projects(conn: &Connection, doc_id: i64, home: &str) -> Result<Vec<String>> {
    let home_norm = normalize(home);
    let mut stmt = conn.prepare(&format!(
        "SELECT name FROM ({MEMBERSHIPS_SQL}) WHERE document_id = ?1 AND norm <> ?2 ORDER BY name"
    ))?;
    let rows = stmt
        .query_map(params![doc_id, home_norm], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every document that carries the project tag `name` — home or linked.
///
/// The rename/merge/delete paths need this BEFORE they mutate the tag, because those documents'
/// vault files name the project in their `linked_projects:` line and must be rewritten. Miss them
/// and the next Rebuild reads the dead name straight back out of the vault and re-mints the
/// project that was just merged away.
pub fn documents_tagged(conn: &Connection, name: &str) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT document_id FROM ({MEMBERSHIPS_SQL}) WHERE norm = ?1 ORDER BY document_id"
    ))?;
    let rows = stmt
        .query_map(params![normalize(name)], |r| r.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Point the project tag `old` at `new`, folding into `new`'s existing tag if there is one.
///
/// The rename arm is a one-row update. The merge arm can't be: `document_tags` is keyed
/// `(document_id, tag_id)`, so a document already in BOTH projects would collide — hence
/// INSERT OR IGNORE the survivor's rows first, then drop the old tag and let the join cascade.
/// This mirrors `projects::rename_project_satellites`' treatment of `project_activity_daily`,
/// which has the same shape of problem for the same reason.
pub fn rename_project_tag(conn: &Connection, old: &str, new: &str) -> Result<()> {
    let (old_norm, new_norm) = (normalize(old), normalize(new));
    if old_norm.is_empty() || new_norm.is_empty() || old_norm == new_norm {
        return Ok(());
    }
    let Some(old_id) = lookup(conn, KIND_PROJECT, &old_norm)? else {
        return Ok(());
    };
    match lookup(conn, KIND_PROJECT, &new_norm)? {
        Some(new_id) => {
            conn.execute(
                "INSERT OR IGNORE INTO document_tags (document_id, tag_id) \
                 SELECT document_id, ?2 FROM document_tags WHERE tag_id = ?1",
                params![old_id, new_id],
            )?;
            conn.execute("DELETE FROM tags WHERE id = ?1", params![old_id])?;
        }
        None => {
            conn.execute(
                "UPDATE tags SET name = ?2, norm = ?3 WHERE id = ?1",
                params![old_id, new.trim(), new_norm],
            )?;
        }
    }
    Ok(())
}

/// Drop a project tag and, by cascade, every membership of it.
///
/// Only the registry side. Re-homing or deleting the documents themselves is the caller's
/// disposition to make (`delete_project`), and the vault rewrite that follows is what makes this
/// stick past the next Rebuild.
pub fn delete_project_tag(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM tags WHERE kind = 'project' AND norm = ?1",
        params![normalize(name)],
    )?;
    Ok(())
}

/// Every project-kind tag name, for the pickers. Ordered case-insensitively so a list mixing
/// "atlas" and "Atlas Ltd" reads the way a person would sort it.
pub fn project_names(conn: &Connection) -> Result<Vec<String>> {
    // Registry rows — which include every project that exists as a triage row only — plus,
    // defensively, any home project the join has not caught up with. A picker that cannot offer a
    // project the user is looking at is worse than one that offers a stale name.
    let mut stmt = conn.prepare(&format!(
        "SELECT name FROM ( \
             SELECT name, norm FROM tags WHERE kind = 'project' \
             UNION \
             SELECT name, norm FROM ({MEMBERSHIPS_SQL}) \
         ) GROUP BY norm ORDER BY norm, name"
    ))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every document's project memberships in one query, keyed by document id.
///
/// The list surfaces need this for hundreds of rows at a time; asking per document would be an
/// N+1 across the whole library. One scan of a small join beats that at every size.
pub fn all_project_memberships(conn: &Connection) -> Result<HashMap<i64, Vec<String>>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT document_id, name FROM ({MEMBERSHIPS_SQL}) ORDER BY document_id, name"
    ))?;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (doc_id, name) = row?;
        out.entry(doc_id).or_default().push(name);
    }
    Ok(out)
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

    fn doc(conn: &Connection, id: i64, project: &str) {
        conn.execute(
            "INSERT INTO documents (id, vault_path, title, content_hash, project) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                format!("d{id}.md"),
                format!("Doc {id}"),
                format!("h{id}"),
                project
            ],
        )
        .unwrap();
    }

    #[test]
    fn interning_is_case_insensitive_but_keeps_the_first_spelling() {
        let (_dir, conn) = store();
        let a = intern(&conn, KIND_PROJECT, "Atlas, Inc.").unwrap();
        let b = intern(&conn, KIND_PROJECT, "atlas, inc.").unwrap();
        assert_eq!(a, b, "the same project however it is typed");
        let name: String = conn
            .query_row("SELECT name FROM tags WHERE id = ?1", params![a], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            name, "Atlas, Inc.",
            "a later, differently-cased mention must not rename the project"
        );
    }

    #[test]
    fn memberships_replace_wholesale_and_exclude_the_home_from_linked() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        set_document_projects(&conn, 1, "Sales", &["Marketing".into()]).unwrap();
        assert_eq!(linked_projects(&conn, 1, "Sales").unwrap(), ["Marketing"]);

        // Re-filing to a project it was merely linked to flips which one is the home. The column
        // moves with it, exactly as `write_document_truth` moves both together — the home is read
        // from `documents.project`, so a test that changed only the join would be testing a state
        // production never produces.
        rehome(&conn, 1, "Marketing");
        set_document_projects(&conn, 1, "Marketing", &["Sales".into()]).unwrap();
        assert_eq!(linked_projects(&conn, 1, "Marketing").unwrap(), ["Sales"]);

        // And dropping the extra leaves only the home.
        set_document_projects(&conn, 1, "Marketing", &[]).unwrap();
        assert!(linked_projects(&conn, 1, "Marketing").unwrap().is_empty());
    }

    /// Move a document's HOME the way production does — the column and the join together.
    fn rehome(conn: &Connection, id: i64, project: &str) {
        conn.execute(
            "UPDATE documents SET project = ?2 WHERE id = ?1",
            params![id, project],
        )
        .unwrap();
    }

    #[test]
    fn a_hand_edited_file_repeating_the_home_does_not_link_a_document_to_itself() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        // Someone edits the vault file and writes the home into `linked_projects:` too.
        set_document_projects(&conn, 1, "Sales", &["sales".into(), "Ops".into()]).unwrap();
        assert_eq!(linked_projects(&conn, 1, "Sales").unwrap(), ["Ops"]);
    }

    #[test]
    fn renaming_a_project_tag_moves_the_memberships() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Old");
        set_document_projects(&conn, 1, "Old", &[]).unwrap();
        rename_project_tag(&conn, "Old", "New").unwrap();
        // The caller re-homes the documents right after (`rewrite_documents`), which is what makes
        // the rename visible to the home-derived half of the membership set.
        rehome(&conn, 1, "New");
        assert_eq!(documents_tagged(&conn, "New").unwrap(), [1]);
        assert!(documents_tagged(&conn, "Old").unwrap().is_empty());
    }

    #[test]
    fn merging_into_a_project_a_document_is_already_in_does_not_collide() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Keep");
        // Homed in Keep, also linked to Fold — the exact overlap a naive re-key would fail on.
        set_document_projects(&conn, 1, "Keep", &["Fold".into()]).unwrap();
        rename_project_tag(&conn, "Fold", "Keep").unwrap();
        assert_eq!(documents_tagged(&conn, "Keep").unwrap(), [1]);
        assert!(linked_projects(&conn, 1, "Keep").unwrap().is_empty());
    }

    #[test]
    fn deleting_a_project_tag_drops_its_memberships_but_leaves_the_others() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Home");
        set_document_projects(&conn, 1, "Home", &["Doomed".into(), "Other".into()]).unwrap();
        delete_project_tag(&conn, "Doomed").unwrap();
        assert_eq!(linked_projects(&conn, 1, "Home").unwrap(), ["Other"]);
    }

    #[test]
    fn deleting_a_document_takes_its_memberships_with_it() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Home");
        set_document_projects(&conn, 1, "Home", &["Other".into()]).unwrap();
        conn.execute("DELETE FROM documents WHERE id = 1", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT count(*) FROM document_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "the join cascades from documents");
    }
}
