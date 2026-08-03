// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every PLACE a document's file lives (#710).
//!
//! Until v54 a document *was* its location: `documents.source_id` named exactly one place, so one
//! file reachable through two Drive accounts — or as both a shared-drive item and a shared-with-me
//! item — became two documents, with two filings, in every list. #703 closed one such overlap by
//! refusing to enumerate the file twice, which only works for overlaps visible from a single
//! account's listing; the general case (one owner, one recipient, two accounts) it cannot see.
//!
//! The model, and Bobby's reasoning for choosing it over a primary-plus-record one: **a document
//! survives while ANY of its locations does**. If a "primary" vanished while another copy was still
//! live and still being edited, a primary-only model would either go stale or reap a document the
//! user still has. Each location is reconciled by its own connector, on its own cursor, against its
//! own change pointer.
//!
//! ## The anchor, and why nothing is ever promoted
//!
//! `documents.source_id` stays — `vault_path` (`idx://<source_id>`) and `content_hash` are both
//! NOT NULL UNIQUE and derived from it, and rule #3 forbids dropping a column. It is now a
//! permanent identity **anchor**: assigned once, never rewritten.
//!
//! That immutability is what dissolves the promotion problem. Nothing reads the anchor to decide
//! whether a body is reachable any more — [`rollup`] does — so an anchor whose location has died
//! costs nothing, and no document ever has to be re-hashed, re-pathed or re-embedded to hand the
//! crown to a sibling. What the anchor still decides is *identity*, which is exactly what it is
//! good at.
//!
//! ## One writer for the mirror
//!
//! The anchor's own row lives in this table like any other location, not outside it as a special
//! case. `documents`' pointer columns (`source_state`, `external_ref`, `source_modified_at`,
//! `source_content_hash`) are a MIRROR of the anchor location plus the reachability rollup, and
//! [`sync_document`] is their only writer — so the two cannot drift, and every existing reader of
//! those columns keeps working untouched.
//!
//! ## What this is NOT
//!
//! A location row is a claim about **provenance** — that two source ids name the same underlying
//! object. Per INVARIANTS I-07 that is a fourth identity claim, beside `documents.content_hash`
//! (derived Markdown), `photos.file_hash` (original bytes) and `source_content_hash` (whatever the
//! provider reports), and it is never inferred from a hash match alone. Nothing in this module
//! makes that claim; it only records one somebody else established.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;
use crate::index_only::SourceState;

/// One place a document's file lives, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The connector's stable id for THIS place — the reconcile key.
    pub source_id: String,
    pub state: SourceState,
    /// The path or URL at this location. Two locations of one file routinely differ here: the same
    /// Drive file is `/Shared/Q3.docx` to its owner and sits at a shared-with-me root for everyone
    /// else, and that difference is the whole reason the duplicate panel was unreadable.
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    /// This location's own change pointer. Per-location on purpose: a shared connection between the
    /// two would make location B's `Update` compare against location A's hash, and the two corpora
    /// would re-embed each other in a loop on every pass.
    pub source_content_hash: Option<String>,
    pub source_parent_folder_id: Option<String>,
    pub source_parent_folder_name: Option<String>,
    /// True for the location matching `documents.source_id` — the identity anchor. Never a
    /// statement about which copy is better or which is live.
    pub anchor: bool,
}

const COLUMNS: &str = "l.source_id, l.source_state, l.external_ref, l.source_modified_at, \
                       l.source_content_hash, l.source_parent_folder_id, \
                       l.source_parent_folder_name, (l.source_id IS d.source_id)";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Location> {
    let state: String = r.get(1)?;
    Ok(Location {
        source_id: r.get(0)?,
        state: SourceState::from_db(&state),
        external_ref: r.get(2)?,
        source_modified_at: r.get(3)?,
        source_content_hash: r.get(4)?,
        source_parent_folder_id: r.get(5)?,
        source_parent_folder_name: r.get(6)?,
        anchor: r.get(7)?,
    })
}

/// How reachable a document is, given every one of its locations. **PURE.**
///
/// A document is as reachable as its BEST location — one live copy keeps it live, which is the
/// property the whole model exists for. The order of the two failures matters and is not
/// arbitrary: `unreachable` means "ask again later" (expired auth, an unmounted drive) while
/// `source_missing` means "it is gone". A file deleted at one source but merely unreachable at
/// another must read as unreachable, or a transient outage at the surviving copy would be reported
/// to the user as a deletion.
///
/// No locations at all rolls up to `source_missing` rather than `ok`: every place PM knew about has
/// been forgotten, and claiming the body is fetchable would produce an error at the reader instead
/// of the honest "only its saved summary is available".
pub fn rollup(states: &[SourceState]) -> SourceState {
    if states.contains(&SourceState::Ok) {
        SourceState::Ok
    } else if states.contains(&SourceState::Unreachable) {
        SourceState::Unreachable
    } else {
        SourceState::SourceMissing
    }
}

/// Every location of one document, anchor first and then oldest-first — the order the reader and
/// the duplicate panel show them in, so "where did this come from" reads top-down.
pub fn list(conn: &Connection, document_id: i64) -> Result<Vec<Location>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM document_locations l \
         JOIN documents d ON d.id = l.document_id \
         WHERE l.document_id = ?1 \
         ORDER BY (l.source_id IS d.source_id) DESC, l.first_seen_at, l.id"
    ))?;
    let rows = stmt.query_map(params![document_id], row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Which document a source id belongs to, if any.
pub fn document_of(conn: &Connection, source_id: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT document_id FROM document_locations WHERE source_id = ?1",
        params![source_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Record (or update) one location, then re-derive the document's mirror.
///
/// Upsert rather than insert: a connector re-observing a location it already knows must not fail,
/// and `first_seen_at` deliberately survives an update — it answers "since when has PM known the
/// file was here", which a re-observation does not change.
///
/// **Silence never unlearns.** Every nullable column is COALESCE'd, the same rule #708 settled on
/// for the source facts: a connector that didn't report a path or a hash has not discovered that
/// there isn't one, and writing its silence through would blank a good `external_ref` (the reader's
/// only way back to the file) or a good change pointer (making the next pass re-embed for nothing).
/// The one column that assigns outright is `source_state`, because "I could not reach this" is a
/// finding rather than a silence — and a rename, which genuinely does replace a path, goes through
/// [`set_external_ref`].
pub fn record(conn: &Connection, document_id: i64, loc: &Location) -> Result<()> {
    conn.execute(
        "INSERT INTO document_locations (document_id, source_id, source_state, external_ref, \
             source_modified_at, source_content_hash, source_parent_folder_id, \
             source_parent_folder_name) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(source_id) DO UPDATE SET \
             document_id = excluded.document_id, \
             source_state = excluded.source_state, \
             external_ref = COALESCE(excluded.external_ref, external_ref), \
             source_modified_at = COALESCE(excluded.source_modified_at, source_modified_at), \
             source_content_hash = COALESCE(excluded.source_content_hash, source_content_hash), \
             source_parent_folder_id = COALESCE(excluded.source_parent_folder_id, source_parent_folder_id), \
             source_parent_folder_name = COALESCE(excluded.source_parent_folder_name, source_parent_folder_name)",
        params![
            document_id,
            loc.source_id,
            loc.state.as_str(),
            loc.external_ref,
            loc.source_modified_at,
            loc.source_content_hash,
            loc.source_parent_folder_id,
            loc.source_parent_folder_name,
        ],
    )?;
    sync_document(conn, document_id)
}

/// Forget every location of a document — used when it stops being an index-only pointer at all
/// (promotion to a full local import). Not a deletion of the document.
pub fn forget_all(conn: &Connection, document_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM document_locations WHERE document_id = ?1",
        params![document_id],
    )?;
    Ok(())
}

/// Set one location's reachability, then re-derive its document's mirror. `Ok(false)` when the id
/// names no location PM knows.
pub fn set_state(conn: &Connection, source_id: &str, state: SourceState) -> Result<bool> {
    let Some(document_id) = document_of(conn, source_id)? else {
        return Ok(false);
    };
    conn.execute(
        "UPDATE document_locations SET source_state = ?2 WHERE source_id = ?1",
        params![source_id, state.as_str()],
    )?;
    sync_document(conn, document_id)?;
    Ok(true)
}

/// Set one location's external ref (a rename/move kept the stable id).
pub fn set_external_ref(
    conn: &Connection,
    source_id: &str,
    external_ref: Option<&str>,
) -> Result<bool> {
    let Some(document_id) = document_of(conn, source_id)? else {
        return Ok(false);
    };
    conn.execute(
        "UPDATE document_locations SET external_ref = ?2 WHERE source_id = ?1",
        params![source_id, external_ref],
    )?;
    sync_document(conn, document_id)?;
    Ok(true)
}

/// Set the reachability of every location belonging to one source — the `SourceFailure` fan-out.
///
/// Matches an exact id or anything namespaced `<source>:<localid>`, exactly as the pre-location
/// fan-out did. The critical difference is what it means afterwards: a document with a live copy
/// somewhere else keeps reading `ok`, because the fan-out moves LOCATIONS and the document's own
/// state is re-derived from all of them. An expired Drive token no longer flips a file the user
/// also has in a tracked folder.
pub fn set_source_state(conn: &Connection, source: &str, state: SourceState) -> Result<usize> {
    let affected: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT document_id FROM document_locations \
             WHERE source_id = ?1 OR source_id LIKE ?1 || ':%'",
        )?;
        let rows = stmt.query_map(params![source], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let moved = conn.execute(
        "UPDATE document_locations SET source_state = ?2 \
         WHERE source_id = ?1 OR source_id LIKE ?1 || ':%'",
        params![source, state.as_str()],
    )?;
    for document_id in affected {
        sync_document(conn, document_id)?;
    }
    Ok(moved)
}

/// Re-derive `documents`' mirror of its locations: the anchor location's pointer columns, and the
/// reachability [`rollup`] across all of them.
///
/// The ONE writer of that mirror. Everything else in PM reads `documents.source_state` — the reader
/// error, the known-set filter, the UI badge, the Rebuild count — and they all keep working because
/// this keeps the column true rather than because they were each taught about locations.
///
/// A document with no locations is left entirely alone: that is a vault document, a chat, a photo or
/// a promoted import, none of which this table describes, and blanking their columns would be
/// destructive rather than merely wrong.
pub fn sync_document(conn: &Connection, document_id: i64) -> Result<()> {
    let states: Vec<SourceState> = {
        let mut stmt =
            conn.prepare("SELECT source_state FROM document_locations WHERE document_id = ?1")?;
        let rows = stmt.query_map(params![document_id], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .map(|s| SourceState::from_db(s))
            .collect()
    };
    if states.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE documents SET \
             source_state = ?2, \
             external_ref = COALESCE( \
                 (SELECT external_ref FROM document_locations \
                  WHERE document_id = ?1 AND source_id IS documents.source_id), external_ref), \
             source_modified_at = COALESCE( \
                 (SELECT source_modified_at FROM document_locations \
                  WHERE document_id = ?1 AND source_id IS documents.source_id), source_modified_at), \
             source_content_hash = COALESCE( \
                 (SELECT source_content_hash FROM document_locations \
                  WHERE document_id = ?1 AND source_id IS documents.source_id), source_content_hash) \
         WHERE id = ?1",
        params![document_id, rollup(&states).as_str()],
    )?;
    Ok(())
}

/// Every currently-healthy location id matching `prefix` — the set a folder-scoped reconcile diffs
/// its live enumeration against. `not_prefix` excludes a nested namespace (My Drive has to exclude
/// its own shared-drive items, which share the account prefix).
///
/// **This is the query #711 depends on.** Fold a duplicate into an existing document and its id
/// stops being a `documents.source_id` — but it is still a location, so it is still known here, and
/// the next sync of that corpus sees a file it already has instead of ingesting a fresh copy. Read
/// off `documents.source_id`, as every connector did before v54, a folded id would come back as a
/// brand-new file on the very next pass and the duplicate would rebuild itself forever.
pub fn known_ids(conn: &Connection, prefix: &str, not_prefix: Option<&str>) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT l.source_id FROM document_locations l \
         JOIN documents d ON d.id = l.document_id \
         WHERE d.source_type = 'index_only' AND l.source_state = 'ok' \
           AND l.source_id LIKE ?1 || '%' \
           AND (?2 IS NULL OR l.source_id NOT LIKE ?2 || '%')",
    )?;
    let rows = stmt.query_map(params![prefix, not_prefix], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The location a body fetch should use: the healthiest one, preferring the anchor among equals.
///
/// The anchor is a tie-break, never a requirement — that is the difference between this model and a
/// primary-plus-record one. A document whose anchor has been deleted at its source still opens, from
/// whichever copy is still there, and goes on tracking that copy's edits.
///
/// An `unreachable` location is still returned when it is the best on offer, deliberately: the
/// caller should attempt the fetch and surface the provider's own error ("your Google sign-in
/// expired"), which is actionable, rather than PM's stored guess. Only `source_missing` — the file
/// really is gone from every place PM knew — has nothing worth trying.
pub fn fetchable(conn: &Connection, document_id: i64) -> Result<Option<Location>> {
    let mut all = list(conn, document_id)?;
    // `list` is already anchor-first, and Rust's sort is stable, so this ranks by health while
    // leaving the anchor ahead of its equals.
    all.sort_by_key(|l| match l.state {
        SourceState::Ok => 0u8,
        SourceState::Unreachable => 1,
        SourceState::SourceMissing => 2,
    });
    Ok(all.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn open() -> Connection {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap()
    }

    /// An index-only document with one anchor location, exactly as the v54 backfill leaves it.
    fn doc(conn: &Connection, source_id: &str) -> i64 {
        conn.execute(
            "INSERT INTO documents (vault_path, title, content_hash, project, source_type, \
                 source_id, source_state, external_ref) \
             VALUES (?1, 'T', ?2, 'Unsorted', 'index_only', ?3, 'ok', ?4)",
            params![
                format!("idx://{source_id}"),
                format!("h-{source_id}"),
                source_id,
                format!("/at/{source_id}")
            ],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO document_locations (document_id, source_id, source_state, external_ref) \
             VALUES (?1, ?2, 'ok', ?3)",
            params![id, source_id, format!("/at/{source_id}")],
        )
        .unwrap();
        id
    }

    fn state_of(conn: &Connection, id: i64) -> String {
        conn.query_row(
            "SELECT source_state FROM documents WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_document_is_as_reachable_as_its_best_location() {
        use SourceState::*;
        assert_eq!(rollup(&[Ok]), Ok);
        assert_eq!(
            rollup(&[SourceMissing, Ok]),
            Ok,
            "one live copy keeps it live"
        );
        assert_eq!(rollup(&[Unreachable, Ok]), Ok);
        // "Ask again later" outranks "it is gone": a transient outage at the surviving copy must
        // not be reported to the user as a deletion.
        assert_eq!(rollup(&[SourceMissing, Unreachable]), Unreachable);
        assert_eq!(rollup(&[SourceMissing, SourceMissing]), SourceMissing);
        // No locations at all — claiming the body is fetchable would surface an error at the reader
        // instead of the honest "only its saved summary is available".
        assert_eq!(rollup(&[]), SourceMissing);
    }

    #[test]
    fn losing_one_location_does_not_lose_the_document() {
        // The property the whole model exists for, and Bobby's reason for choosing it: the file is
        // gone from one Drive account while the copy in a tracked folder is still there and still
        // being edited. A primary-only model would either go stale or reap it.
        let conn = open();
        let id = doc(&conn, "gdrive:a@x.com:f1");
        record(
            &conn,
            id,
            &Location {
                source_id: "local:abc:f9".into(),
                state: SourceState::Ok,
                external_ref: Some("/home/reports/q3.docx".into()),
                source_modified_at: None,
                source_content_hash: None,
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: false,
            },
        )
        .unwrap();

        assert!(set_state(&conn, "gdrive:a@x.com:f1", SourceState::SourceMissing).unwrap());
        assert_eq!(
            state_of(&conn, id),
            "ok",
            "the local copy keeps it reachable"
        );

        // And it opens from the copy that is still there, not the anchor.
        let open_from = fetchable(&conn, id).unwrap().unwrap();
        assert_eq!(open_from.source_id, "local:abc:f9");
        assert!(!open_from.anchor, "the anchor is a tie-break, not a gate");

        // Only when the last one goes does the document read as missing — and then the best location
        // on offer has nothing worth trying, which is what the reader refuses on.
        assert!(set_state(&conn, "local:abc:f9", SourceState::SourceMissing).unwrap());
        assert_eq!(state_of(&conn, id), "source_missing");
        assert_eq!(
            fetchable(&conn, id).unwrap().unwrap().state,
            SourceState::SourceMissing
        );
    }

    #[test]
    fn a_folded_location_stays_known_to_its_connector() {
        // The query #711 depends on. Read off `documents.source_id`, as every connector did before
        // v54, a folded duplicate's id would come back as a brand-new file on the next pass and the
        // duplicate would rebuild itself forever.
        let conn = open();
        let id = doc(&conn, "gdrive:swm:root1:f1");
        record(
            &conn,
            id,
            &Location {
                source_id: "gdrive:b@x.com:f1".into(),
                state: SourceState::Ok,
                external_ref: None,
                source_modified_at: None,
                source_content_hash: None,
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: false,
            },
        )
        .unwrap();
        let known = known_ids(&conn, "gdrive:b@x.com:", Some("gdrive:b@x.com:sd:")).unwrap();
        assert_eq!(known, vec!["gdrive:b@x.com:f1".to_string()]);
        // ...and it stops being known the moment that copy really is gone, so a genuine deletion is
        // still a deletion.
        set_state(&conn, "gdrive:b@x.com:f1", SourceState::SourceMissing).unwrap();
        assert!(known_ids(&conn, "gdrive:b@x.com:", None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_shared_drive_namespace_is_excluded_from_its_accounts_own_set() {
        // My Drive ids and shared-drive ids share the account prefix; the reconcile of one must not
        // see the other's files or it would delete them as absent.
        let conn = open();
        doc(&conn, "gdrive:a@x.com:f1");
        doc(&conn, "gdrive:a@x.com:sd:drive9:f2");
        let mine = known_ids(&conn, "gdrive:a@x.com:", Some("gdrive:a@x.com:sd:")).unwrap();
        assert_eq!(mine, vec!["gdrive:a@x.com:f1".to_string()]);
    }

    #[test]
    fn one_expired_account_does_not_flip_a_file_the_user_also_has_locally() {
        // The fan-out moves LOCATIONS; the document's state is re-derived from all of them. Before
        // v54 this was a single UPDATE over `documents` and the local copy went unreachable too.
        let conn = open();
        let id = doc(&conn, "gdrive:a@x.com:f1");
        record(
            &conn,
            id,
            &Location {
                source_id: "local:abc:f9".into(),
                state: SourceState::Ok,
                external_ref: None,
                source_modified_at: None,
                source_content_hash: None,
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: false,
            },
        )
        .unwrap();
        let other = doc(&conn, "gdrive:a@x.com:f2");

        assert_eq!(
            set_source_state(&conn, "gdrive:a@x.com", SourceState::Unreachable).unwrap(),
            2
        );
        assert_eq!(
            state_of(&conn, id),
            "ok",
            "still readable from the local copy"
        );
        assert_eq!(state_of(&conn, other), "unreachable", "this one really is");
    }

    #[test]
    fn the_mirror_follows_the_anchor_and_never_a_sibling() {
        // `documents`' pointer columns describe the ANCHOR location, because that is what every
        // pre-v54 reader of them assumes. A sibling's ref must not leak into the row and send the
        // reader to the wrong place.
        let conn = open();
        let id = doc(&conn, "gdrive:a@x.com:f1");
        record(
            &conn,
            id,
            &Location {
                source_id: "local:abc:f9".into(),
                state: SourceState::Ok,
                external_ref: Some("/home/q3.docx".into()),
                source_modified_at: Some("2026-08-01T00:00:00Z".into()),
                source_content_hash: Some("sibling-hash".into()),
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: false,
            },
        )
        .unwrap();
        let (eref, hash): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT external_ref, source_content_hash FROM documents WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(eref.as_deref(), Some("/at/gdrive:a@x.com:f1"));
        assert_ne!(hash.as_deref(), Some("sibling-hash"));
    }

    #[test]
    fn a_document_with_no_locations_is_left_entirely_alone() {
        // Vault documents, chats, photos and promoted imports are not described by this table, and
        // blanking their columns would be destructive rather than merely wrong.
        let conn = open();
        conn.execute(
            "INSERT INTO documents (vault_path, title, content_hash, project, source_type, \
                 source_state, external_ref) \
             VALUES ('a.md', 'T', 'h1', 'Unsorted', 'vault', 'ok', '/somewhere')",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        sync_document(&conn, id).unwrap();
        assert_eq!(state_of(&conn, id), "ok");
        let eref: Option<String> = conn
            .query_row(
                "SELECT external_ref FROM documents WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(eref.as_deref(), Some("/somewhere"));
    }

    #[test]
    fn re_observing_a_location_keeps_when_pm_first_saw_it_there() {
        // `first_seen_at` answers "since when has PM known the file was here" — a re-observation
        // does not change that, and the duplicate panel orders siblings by it.
        let conn = open();
        let id = doc(&conn, "gdrive:a@x.com:f1");
        let before: String = conn
            .query_row(
                "SELECT first_seen_at FROM document_locations WHERE source_id = 'gdrive:a@x.com:f1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        record(
            &conn,
            id,
            &Location {
                source_id: "gdrive:a@x.com:f1".into(),
                state: SourceState::Ok,
                external_ref: Some("/moved/here.docx".into()),
                source_modified_at: None,
                source_content_hash: Some("new".into()),
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: true,
            },
        )
        .unwrap();
        let after: String = conn
            .query_row(
                "SELECT first_seen_at FROM document_locations WHERE source_id = 'gdrive:a@x.com:f1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            list(&conn, id).unwrap()[0].external_ref.as_deref(),
            Some("/moved/here.docx"),
            "the rest of the row does update"
        );
    }

    #[test]
    fn silence_from_a_connector_never_unlearns_where_the_file_is() {
        // A re-observation that reports no path has not discovered there isn't one — and
        // `external_ref` is the reader's only way back to a local file, so blanking it would make
        // the document unopenable on the strength of a connector saying nothing.
        let conn = open();
        let id = doc(&conn, "local:abc:f9");
        record(
            &conn,
            id,
            &Location {
                source_id: "local:abc:f9".into(),
                state: SourceState::Ok,
                external_ref: None,
                source_modified_at: None,
                source_content_hash: None,
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: true,
            },
        )
        .unwrap();
        assert_eq!(
            list(&conn, id).unwrap()[0].external_ref.as_deref(),
            Some("/at/local:abc:f9")
        );
    }

    #[test]
    fn an_unreachable_copy_is_still_worth_opening() {
        // Only `source_missing` has nothing to try. An expired sign-in should reach the provider and
        // surface ITS error, which tells the user what to do; PM's stored guess does not.
        let conn = open();
        let id = doc(&conn, "gdrive:a@x.com:f1");
        set_state(&conn, "gdrive:a@x.com:f1", SourceState::Unreachable).unwrap();
        let best = fetchable(&conn, id).unwrap().unwrap();
        assert_eq!(best.state, SourceState::Unreachable);
        assert_eq!(best.source_id, "gdrive:a@x.com:f1");
    }

    #[test]
    fn an_unknown_source_id_is_a_miss_rather_than_a_write() {
        let conn = open();
        assert!(!set_state(&conn, "gdrive:nobody:f1", SourceState::Ok).unwrap());
        assert!(!set_external_ref(&conn, "gdrive:nobody:f1", Some("/x")).unwrap());
        assert!(document_of(&conn, "gdrive:nobody:f1").unwrap().is_none());
    }
}
