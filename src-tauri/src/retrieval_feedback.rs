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

use rusqlite::{params, Connection};
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
/// `None` (an ungrounded answer) writes NULL, which stays distinguishable from an empty array: the
/// first means nothing was retrieved, the second that retrieval ran and found nothing.
pub fn record_grounding(conn: &Connection, message_id: i64, chunk_ids: &[i64]) -> Result<()> {
    let json = serde_json::to_string(chunk_ids).map_err(|e| Error::Other(e.to_string()))?;
    conn.execute(
        "UPDATE messages SET retrieved_chunk_ids = ?2 WHERE id = ?1",
        params![message_id, json],
    )?;
    Ok(())
}

/// The asking turn and the grounding for `message_id` — the user message immediately preceding it
/// in the same conversation, and the chunk ids recorded at answer time.
///
/// Returns `None` when the answer has no recorded grounding, which is the correct outcome rather
/// than an error: an ungrounded answer has no retrieval to judge, so there is nothing to log and a
/// stray thumb on one is simply dropped.
fn grounding_for(conn: &Connection, message_id: i64) -> Result<Option<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT (SELECT content FROM messages u \
                  WHERE u.conversation_id = m.conversation_id \
                    AND u.role = 'user' AND u.id < m.id \
                  ORDER BY u.id DESC LIMIT 1), \
                m.retrieved_chunk_ids \
           FROM messages m WHERE m.id = ?1",
    )?;
    let row = stmt
        .query_row(params![message_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        })
        .ok();
    Ok(match row {
        Some((Some(query), Some(chunks))) => Some((query, chunks)),
        _ => None,
    })
}

/// The retrieval configuration in force, as an opaque stamp string.
///
/// Recorded for the same reason v43 stamps the filing pipeline: signal gathered under one chunking
/// and embedding regime is not comparable with signal gathered under another, and history that was
/// never labelled cannot be separated afterwards. Best-effort — a stamp that can't be resolved
/// leaves NULL rather than failing the user's click.
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
    let Some((query, chunk_ids)) = grounding_for(conn, message_id)? else {
        return Ok(false);
    };
    conn.execute(
        "INSERT INTO retrieval_feedback(message_id, query, chunk_ids, signal, config_stamp) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            message_id,
            query,
            chunk_ids,
            rating.as_signal(),
            config_stamp(conn)
        ],
    )?;
    Ok(true)
}

/// Log that the user opened one of the sources cited by an answer.
///
/// Idempotent per (answer, document): the unique index makes a repeat click a no-op rather than a
/// duplicate row, so a corpus count stays a count of judgements and not of curiosity.
pub fn record_citation_click(conn: &Connection, message_id: i64, document_id: i64) -> Result<bool> {
    let Some((query, chunk_ids)) = grounding_for(conn, message_id)? else {
        return Ok(false);
    };
    conn.execute(
        "INSERT OR IGNORE INTO retrieval_feedback\
             (message_id, query, chunk_ids, signal, document_id, config_stamp) \
         VALUES (?1, ?2, ?3, 'citation_click', ?4, ?5)",
        params![
            message_id,
            query,
            chunk_ids,
            document_id,
            config_stamp(conn)
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
