// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Retrieval-relevance feedback capture (Stage-4 card 10) — the corpus a learned reranker would
//! one day train on.
//!
//! **This module collects; it does not learn.** No scoring, no ranking, no model reads any of it
//! yet. It exists early and deliberately: a query-time cross-encoder can only be trained on
//! judgements of the form *did this CHUNK answer this QUERY*, and PM records that nowhere. The
//! `corrections` table is the wrong shape — it logs FILING decisions (this document belongs to that
//! project), which say nothing about query-time relevance. So the reranker's gate cannot open for
//! want of a corpus rather than for want of a model, and a corpus only accrues if capture is already
//! shipped while people are using the thing.
//!
//! Two signals, both cheap and both honest:
//!
//! * **A rating** on an answer — an explicit thumb. One per answer; a later thumb replaces the
//!   earlier one, because a user changing their mind is a correction, not a second opinion.
//! * **A citation click** — the user opened one of the sources PM offered. Weaker than a thumb and
//!   noisier (curiosity looks like relevance), but it is unprompted and costs the user nothing, so
//!   it accrues at a rate explicit ratings never will. Deduped per document so re-opening the same
//!   source doesn't inflate the corpus with copies of one judgement.
//!
//! Every row snapshots the query text and the grounding chunk ids rather than joining back to them,
//! so it stands alone as a training example. It still cascades from `messages`: deleting a
//! conversation takes its feedback with it, because PM does not keep a shadow copy of what someone
//! asked after they have deleted the asking.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{Error, Result};

/// An explicit judgement on a grounded answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rating {
    Up,
    Down,
}

impl Rating {
    fn as_signal(self) -> &'static str {
        match self {
            Rating::Up => "up",
            Rating::Down => "down",
        }
    }

    /// Parse the frontend's string form, rejecting anything else before it reaches SQL.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "up" => Ok(Rating::Up),
            "down" => Ok(Rating::Down),
            other => Err(Error::Other(format!(
                "unknown answer rating {other:?} (expected \"up\" or \"down\")"
            ))),
        }
    }
}

/// What the UI needs to render an answer's feedback controls: the rating already given, if any.
#[derive(Clone, Debug, Default, Serialize)]
pub struct AnswerFeedback {
    /// `"up"`, `"down"`, or `None` when the answer hasn't been rated.
    pub rating: Option<String>,
}

/// Record what grounded an answer, so a later reaction can name the chunks it is judging.
///
/// Stored as JSON on the message rather than in a side table because it is per-answer data with the
/// same lifetime as the answer, and because at the moment the user reacts the frontend knows only
/// which message it is — the retrieved set is long out of scope by then.
///
/// Banks three things, all of which are only knowable NOW (v49):
///
/// * the raw `chunks.id` set, as before — an honest record of what was retrieved at the time;
/// * each chunk's **stable uid**, which survives a Rebuild. Row ids do not: a Rebuild deletes and
///   re-creates every chunk, and SQLite reuses those integers, so a judgement stored by id alone
///   comes to name unrelated text rather than merely going stale;
/// * the **retrieval config in force at answer time**. Resolving it when the user later clicks
///   labelled the judgement with whatever regime happened to be current then, which for a thumb
///   given after a re-embed is precisely the wrong one.
///
/// An ungrounded answer records nothing, which stays distinguishable from an empty array: the first
/// means nothing was retrieved, the second that retrieval ran and found nothing.
pub fn record_grounding(conn: &Connection, message_id: i64, chunk_ids: &[i64]) -> Result<()> {
    let json = serde_json::to_string(chunk_ids).map_err(|e| Error::Other(e.to_string()))?;
    let uids = serde_json::to_string(&stable_uids(conn, chunk_ids)?)
        .map_err(|e| Error::Other(e.to_string()))?;
    conn.execute(
        "UPDATE messages SET retrieved_chunk_ids = ?2, retrieved_chunk_uids = ?3, \
                             retrieved_config_stamp = ?4 WHERE id = ?1",
        params![message_id, json, uids, config_stamp(conn)],
    )?;
    Ok(())
}

/// The Rebuild-stable `chunks.uid` for each id, in the SAME order, so uid `n` is the identity of
/// chunk id `n`. A chunk with no uid (pre-v16 rows, which predate stable uids) contributes `null`
/// rather than shifting every later entry — a training example must never silently re-pair a query
/// with a different chunk's identity.
fn stable_uids(conn: &Connection, chunk_ids: &[i64]) -> Result<Vec<Option<String>>> {
    let mut stmt = conn.prepare("SELECT uid FROM chunks WHERE id = ?1")?;
    chunk_ids
        .iter()
        .map(|id| {
            stmt.query_row(params![id], |r| r.get::<_, Option<String>>(0))
                .optional()
                .map(Option::flatten)
                .map_err(Error::from)
        })
        .collect()
}

/// Everything a reaction needs about the answer it judges, all of it snapshotted at ANSWER time.
struct Grounding {
    /// The asking turn — the user message immediately preceding the answer.
    query: String,
    /// JSON array of `chunks.id` as retrieved. A Rebuild invalidates these; kept as the honest
    /// record of the moment, with [`Grounding::chunk_uids`] carrying the durable identity.
    chunk_ids: String,
    /// JSON array of stable `chunks.uid`, parallel to `chunk_ids`. `None` for answers produced
    /// before v49 banked them.
    chunk_uids: Option<String>,
    /// The retrieval configuration in force when the answer was produced. `None` for pre-v49
    /// answers, which is the correct value: it was never recorded, and resolving it now would
    /// label the judgement with a regime it was not formed under.
    config_stamp: Option<String>,
}

/// The grounding banked for `message_id`.
///
/// Returns `None` when the answer has no recorded grounding, which is the correct outcome rather
/// than an error: an ungrounded answer has no retrieval to judge, so there is nothing to log and a
/// stray thumb on one is simply dropped.
fn grounding_for(conn: &Connection, message_id: i64) -> Result<Option<Grounding>> {
    let mut stmt = conn.prepare(
        "SELECT (SELECT content FROM messages u \
                  WHERE u.conversation_id = m.conversation_id \
                    AND u.role = 'user' AND u.id < m.id \
                  ORDER BY u.id DESC LIMIT 1), \
                m.retrieved_chunk_ids, m.retrieved_chunk_uids, m.retrieved_config_stamp \
           FROM messages m WHERE m.id = ?1",
    )?;
    let row = stmt
        .query_row(params![message_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .ok();
    Ok(match row {
        Some((Some(query), Some(chunk_ids), chunk_uids, config_stamp)) => Some(Grounding {
            query,
            chunk_ids,
            chunk_uids,
            config_stamp,
        }),
        _ => None,
    })
}

/// The retrieval configuration in force, as an opaque stamp string.
///
/// Recorded for the same reason v43 stamps the filing pipeline: signal gathered under one chunking
/// and embedding regime is not comparable with signal gathered under another, and history that was
/// never labelled cannot be separated afterwards. Best-effort — a stamp that can't be resolved
/// leaves NULL rather than failing the user's write.
///
/// Called at ANSWER time only (from [`record_grounding`]); a reaction reads the banked stamp. A
/// thumb can be given days later, across a re-embed or a Rebuild, so resolving it at reaction time
/// labelled judgements with a regime they were not formed under — the precise confusion the stamp
/// exists to prevent.
fn config_stamp(conn: &Connection) -> Option<String> {
    let embedder = crate::db::selected_embedder(conn).ok()?;
    let cfg = crate::retrieval_config::RetrievalConfig::current_for(&embedder);
    serde_json::to_string(&cfg).ok()
}

/// Rate a grounded answer, replacing any rating already given for it. `None` clears the rating.
///
/// Returns whether anything was stored — `false` when the answer had no grounding to judge.
pub fn set_rating(conn: &Connection, message_id: i64, rating: Option<Rating>) -> Result<bool> {
    // Clearing and re-rating both start by removing the previous verdict: the unique index admits
    // one rating per answer, and a changed mind supersedes rather than accumulates.
    conn.execute(
        "DELETE FROM retrieval_feedback WHERE message_id = ?1 AND signal IN ('up','down')",
        params![message_id],
    )?;
    let Some(rating) = rating else {
        return Ok(true);
    };
    let Some(g) = grounding_for(conn, message_id)? else {
        return Ok(false);
    };
    conn.execute(
        "INSERT INTO retrieval_feedback\
             (message_id, query, chunk_ids, chunk_uids, signal, config_stamp) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            message_id,
            g.query,
            g.chunk_ids,
            g.chunk_uids,
            rating.as_signal(),
            g.config_stamp
        ],
    )?;
    Ok(true)
}

/// Log that the user opened one of the sources cited by an answer.
///
/// Idempotent per (answer, document): the unique index makes a repeat click a no-op rather than a
/// duplicate row, so a corpus count stays a count of judgements and not of curiosity.
pub fn record_citation_click(conn: &Connection, message_id: i64, document_id: i64) -> Result<bool> {
    let Some(g) = grounding_for(conn, message_id)? else {
        return Ok(false);
    };
    conn.execute(
        "INSERT OR IGNORE INTO retrieval_feedback\
             (message_id, query, chunk_ids, chunk_uids, signal, document_id, config_stamp) \
         VALUES (?1, ?2, ?3, ?4, 'citation_click', ?5, ?6)",
        params![
            message_id,
            g.query,
            g.chunk_ids,
            g.chunk_uids,
            document_id,
            g.config_stamp
        ],
    )?;
    Ok(true)
}

/// The rating already recorded for an answer, for rendering its controls.
pub fn feedback_for(conn: &Connection, message_id: i64) -> Result<AnswerFeedback> {
    let rating: Option<String> = conn
        .query_row(
            "SELECT signal FROM retrieval_feedback \
              WHERE message_id = ?1 AND signal IN ('up','down') LIMIT 1",
            params![message_id],
            |r| r.get(0),
        )
        .ok();
    Ok(AnswerFeedback { rating })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// A conversation with one asking turn and one grounded answer; returns the answer's id.
    fn seed(conn: &Connection, chunk_ids: Option<&[i64]>) -> i64 {
        conn.execute("INSERT INTO conversations(title) VALUES ('c')", [])
            .unwrap();
        let cid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1,'user','where are my taxes?')",
            params![cid],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1,'assistant','here')",
            params![cid],
        )
        .unwrap();
        let mid = conn.last_insert_rowid();
        if let Some(ids) = chunk_ids {
            record_grounding(conn, mid, ids).unwrap();
        }
        mid
    }

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM retrieval_feedback", [], |r| r.get(0))
            .unwrap()
    }

    /// A rating snapshots the query and the grounding, so the row stands alone as training data.
    #[test]
    fn a_rating_snapshots_the_query_and_the_chunks() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[7, 9]));
        assert!(set_rating(&conn, mid, Some(Rating::Up)).unwrap());

        let (q, c, s): (String, String, String) = conn
            .query_row(
                "SELECT query, chunk_ids, signal FROM retrieval_feedback WHERE message_id = ?1",
                params![mid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(q, "where are my taxes?");
        assert_eq!(c, "[7,9]");
        assert_eq!(s, "up");
    }

    /// A document with two chunks carrying stable uids — the identities that survive a Rebuild.
    fn seed_chunks(conn: &Connection) -> (i64, i64) {
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('v/a.md','A','h-a')",
            [],
        )
        .unwrap();
        let doc = conn.last_insert_rowid();
        let mut ids = Vec::new();
        for (ordinal, uid) in [(0, "uid-aaa"), (1, "uid-bbb")] {
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count, uid) \
                 VALUES (?1, ?2, 'body', 4, ?3)",
                params![doc, ordinal, uid],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        (ids[0], ids[1])
    }

    /// Row ids do not survive a Rebuild; the stable uid does. A judgement stored by id alone doesn't
    /// merely go stale — SQLite reuses those integers, so it comes to name unrelated text.
    #[test]
    fn grounding_banks_rebuild_stable_uids_beside_the_row_ids() {
        let (_d, conn) = store();
        let (c1, c2) = seed_chunks(&conn);
        let mid = seed(&conn, Some(&[c1, c2]));

        let (ids, uids): (String, Option<String>) = conn
            .query_row(
                "SELECT retrieved_chunk_ids, retrieved_chunk_uids FROM messages WHERE id = ?1",
                params![mid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ids, format!("[{c1},{c2}]"));
        assert_eq!(uids.as_deref(), Some(r#"["uid-aaa","uid-bbb"]"#));

        // The uids ride onto the judgement itself, so a training example stands alone.
        assert!(set_rating(&conn, mid, Some(Rating::Up)).unwrap());
        let stored: Option<String> = conn
            .query_row(
                "SELECT chunk_uids FROM retrieval_feedback WHERE message_id = ?1",
                params![mid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some(r#"["uid-aaa","uid-bbb"]"#));
    }

    /// A chunk with no uid contributes `null` rather than being skipped — dropping it would shift
    /// every later entry and silently re-pair the query with a different chunk's identity.
    #[test]
    fn a_chunk_without_a_uid_holds_its_place_as_null() {
        let (_d, conn) = store();
        let (c1, _c2) = seed_chunks(&conn);
        conn.execute("UPDATE chunks SET uid = NULL WHERE id = ?1", params![c1])
            .unwrap();
        let mid = seed(&conn, Some(&[c1, c1 + 1]));

        let uids: Option<String> = conn
            .query_row(
                "SELECT retrieved_chunk_uids FROM messages WHERE id = ?1",
                params![mid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uids.as_deref(), Some(r#"[null,"uid-bbb"]"#));
    }

    /// The config stamp is banked with the ANSWER. Resolving it when the user later clicks labelled
    /// the judgement with whatever regime was current THEN — for a thumb given after a re-embed,
    /// precisely the one it was not formed under.
    #[test]
    fn the_config_stamp_is_the_one_in_force_at_answer_time() {
        let (_d, conn) = store();
        let (c1, _) = seed_chunks(&conn);
        let mid = seed(&conn, Some(&[c1]));

        let at_answer: Option<String> = conn
            .query_row(
                "SELECT retrieved_config_stamp FROM messages WHERE id = ?1",
                params![mid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(at_answer.is_some(), "the answer banks a stamp");

        // The vault re-embeds under a different model between the answer and the reaction.
        crate::db::set_setting(&conn, "embedding_model", "intfloat/multilingual-e5-large").unwrap();
        let now = config_stamp(&conn);
        assert_ne!(
            now, at_answer,
            "the fixture must actually change the live stamp, or this test proves nothing"
        );

        set_rating(&conn, mid, Some(Rating::Up)).unwrap();
        let stored: Option<String> = conn
            .query_row(
                "SELECT config_stamp FROM retrieval_feedback WHERE message_id = ?1",
                params![mid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, at_answer,
            "the judgement carries the regime it was formed under, not the current one"
        );
    }

    /// An answer produced before v49 banked these keeps NULL rather than acquiring a stamp resolved
    /// now — a value that was never recorded must not be invented after the fact.
    #[test]
    fn a_pre_v49_answer_records_no_stamp_rather_than_a_wrong_one() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[1]));
        conn.execute(
            "UPDATE messages SET retrieved_chunk_uids = NULL, retrieved_config_stamp = NULL \
             WHERE id = ?1",
            params![mid],
        )
        .unwrap();

        assert!(set_rating(&conn, mid, Some(Rating::Up)).unwrap());
        let (uids, stamp): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT chunk_uids, config_stamp FROM retrieval_feedback WHERE message_id = ?1",
                params![mid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(uids, None);
        assert_eq!(stamp, None);
    }

    /// A changed mind supersedes: one rating per answer, never two contradicting each other.
    #[test]
    fn re_rating_replaces_rather_than_accumulates() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[1]));
        set_rating(&conn, mid, Some(Rating::Up)).unwrap();
        set_rating(&conn, mid, Some(Rating::Down)).unwrap();
        assert_eq!(rows(&conn), 1);
        assert_eq!(
            feedback_for(&conn, mid).unwrap().rating.as_deref(),
            Some("down")
        );

        set_rating(&conn, mid, None).unwrap();
        assert_eq!(rows(&conn), 0, "clearing removes the judgement entirely");
        assert_eq!(feedback_for(&conn, mid).unwrap().rating, None);
    }

    /// Re-opening the same source is one judgement, not many.
    #[test]
    fn citation_clicks_dedupe_per_document_but_not_across_them() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[1, 2]));
        assert!(record_citation_click(&conn, mid, 42).unwrap());
        assert!(record_citation_click(&conn, mid, 42).unwrap());
        assert_eq!(rows(&conn), 1, "the repeat click is a no-op");
        record_citation_click(&conn, mid, 43).unwrap();
        assert_eq!(
            rows(&conn),
            2,
            "a different source is a different judgement"
        );
    }

    /// A rating and a click are independent signals and coexist on one answer.
    #[test]
    fn a_rating_and_a_click_coexist() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[1]));
        set_rating(&conn, mid, Some(Rating::Up)).unwrap();
        record_citation_click(&conn, mid, 5).unwrap();
        assert_eq!(rows(&conn), 2);
        // Replacing the rating must not disturb the click.
        set_rating(&conn, mid, Some(Rating::Down)).unwrap();
        assert_eq!(rows(&conn), 2);
    }

    /// An answer that retrieved nothing has no relevance judgement to make; a stray thumb is
    /// dropped rather than stored against an empty grounding.
    #[test]
    fn an_ungrounded_answer_records_nothing() {
        let (_d, conn) = store();
        let mid = seed(&conn, None);
        assert!(!set_rating(&conn, mid, Some(Rating::Up)).unwrap());
        assert!(!record_citation_click(&conn, mid, 1).unwrap());
        assert_eq!(rows(&conn), 0);
    }

    /// Deleting the conversation takes the feedback with it — no shadow copy of a deleted question.
    #[test]
    fn deleting_the_conversation_deletes_its_feedback() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[1]));
        set_rating(&conn, mid, Some(Rating::Up)).unwrap();
        record_citation_click(&conn, mid, 5).unwrap();
        assert_eq!(rows(&conn), 2);

        let cid: i64 = conn
            .query_row(
                "SELECT conversation_id FROM messages WHERE id = ?1",
                params![mid],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conn.execute("DELETE FROM conversations WHERE id = ?1", params![cid])
            .unwrap();
        assert_eq!(rows(&conn), 0, "feedback cascaded away with the chat");
    }

    /// An empty retrieval is not the same as no retrieval, and the distinction has to survive.
    #[test]
    fn empty_grounding_is_distinct_from_absent_grounding() {
        let (_d, conn) = store();
        let mid = seed(&conn, Some(&[]));
        let stored: Option<String> = conn
            .query_row(
                "SELECT retrieved_chunk_ids FROM messages WHERE id = ?1",
                params![mid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some("[]"));
    }

    #[test]
    fn an_unknown_rating_is_rejected() {
        assert!(Rating::parse("sideways").is_err());
        assert_eq!(Rating::parse("up").unwrap(), Rating::Up);
    }
}
