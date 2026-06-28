// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Incremental chat indexing engine (board card 7B, #141) — the engine that turns completed chat
//! turn-pairs into indexed chunks, **incrementally, idempotently, and append-only**, off card A's
//! `chat_sessions.last_indexed_turn_id` cursor, so a chat flows through the same chunk → embed → index
//! → hybrid-retrieval pipeline as a document.
//!
//! The shape, and why it diverges from document ingest:
//!
//!   * **Append-only.** A chat session is one `documents` row (born here on first index, carrying the
//!     stable `chat:<id>` identity reserved by card A). Each completed turn-pair past the cursor is
//!     split on its own and its chunks are *appended* to that row, continuing the ordinal sequence —
//!     old chunks are never re-split or re-embedded. Document ingest re-splits the whole file on every
//!     change; chat can't afford that (it grows forever), and append-only is what makes per-chunk
//!     timestamps natural.
//!   * **Authored content only.** We embed exactly what the user wrote and what the model wrote, read
//!     straight from the `messages` table — never the RAG context that was assembled into the prompt.
//!     Indexing the assembled prompt would re-embed copies of other documents' chunks as chat sources
//!     (near-duplicate retrieval poisoning + unbounded bloat). See [`render_authored_segment`].
//!   * **Cursor-driven idempotency.** The cursor advances only inside the same transaction that lands
//!     the chunks ([`commit_session_index`]). A crash mid-sweep leaves the cursor where it was, so the
//!     next run simply re-reads the same turn-pairs — never a double-insert. The trigger is always "is
//!     there content past the cursor?", never "is the conversation done?".
//!
//! Triggers in this card: an app-launch reconcile sweep ([`spawn_launch_sweep`]). The idle-cadence
//! background job and the triviality gate are card 7B's second PR. Context assembly (C), the
//! navigation pointer's UI (E), and learning-loop routing (F) are later cards.

use std::sync::atomic::Ordering;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::ingest::{self, DocMeta, SourceMeta};
use crate::model_gateway::ModelGateway;
use crate::splitter;
use crate::{chat, registry, AppState};

/// How long the launch sweep waits for the vault to unlock + the engine to be provisioned before
/// giving up (a passphrase vault opens only once the user unlocks). `TICKS × SECS` ≈ 5 minutes; if the
/// session never unlocks this run, the next launch — or the PR2 idle job — catches up.
const LAUNCH_WAIT_TICKS: u32 = 60;
const LAUNCH_WAIT_SECS: u64 = 5;

/// Idle scheduler cadence: check every `IDLE_TICK_SECS`, and run a background sweep once the user has
/// been idle for at least `IDLE_THRESHOLD_SECS` (≈15 min) so indexing never competes with active use.
const IDLE_TICK_SECS: u64 = 300;
const IDLE_THRESHOLD_SECS: u64 = 900;

/// The result of indexing one session.
pub(crate) enum Outcome {
    /// No completed turn-pairs past the cursor (or no session/vault yet) — nothing done.
    UpToDate,
    /// Indexed `turns` new turn-pairs into `chunks` appended chunks.
    Indexed { turns: usize, chunks: usize },
}

/// A reconcile sweep's tally.
#[derive(Default)]
pub(crate) struct Summary {
    pub sessions: usize,
    pub turns: usize,
    pub chunks: usize,
    pub failed: usize,
}

/// One turn-pair already split + embedded, ready to land. Kept separate from the embed step so the
/// transaction half ([`commit_session_index`]) is pure DB logic — unit-testable without the sidecar.
pub(crate) struct IndexedSegment {
    pub turn_id: i64,
    pub at: String,
    pub chunks: Vec<splitter::Chunk>,
    pub embeddings: Vec<Vec<f32>>,
}

/// The raw `chat_sessions` row, as read before validating the vault path is present.
struct SessionRow {
    document_id: Option<i64>,
    last_indexed: Option<i64>,
    vault_path: Option<String>,
    scope: String,
}

/// The per-session context the commit needs (read once under a short lock).
struct SessionPlan {
    conversation_id: i64,
    /// The chat's `documents` row, or `None` until this is the first index (then the row is born).
    document_id: Option<i64>,
    vault_path: String,
    scope: String,
    title: String,
    project: Option<String>,
    created_at: String,
}

/// Index every completed turn-pair of one conversation that is past its index cursor. Reads the
/// session + new pairs under a short lock, chunks/embeds each pair off the lock, then lands them all in
/// one transaction. A best-effort unit — a failure leaves the cursor untouched, so the next sweep
/// retries from exactly where this one stopped.
pub(crate) fn index_session(state: &AppState, conversation_id: i64) -> Result<Outcome> {
    // 1. Short lock: the session row, its conversation context, and the pairs past the cursor.
    let (plan, pairs) = {
        let conn = state.conn()?;
        let row: Option<SessionRow> = conn
            .query_row(
                "SELECT document_id, last_indexed_turn_id, vault_path, scope \
                 FROM chat_sessions WHERE conversation_id = ?1",
                params![conversation_id],
                |r| {
                    Ok(SessionRow {
                        document_id: r.get(0)?,
                        last_indexed: r.get(1)?,
                        vault_path: r.get(2)?,
                        scope: r.get(3)?,
                    })
                },
            )
            .optional()?;
        // No session row (card A hasn't appended a turn-pair) or no vault file yet ⇒ nothing to index.
        let Some(SessionRow {
            document_id,
            last_indexed,
            vault_path: Some(vault_path),
            scope,
        }) = row
        else {
            return Ok(Outcome::UpToDate);
        };
        let (title, project, created_at): (String, Option<String>, String) = conn.query_row(
            "SELECT title, project, created_at FROM conversations WHERE id = ?1",
            params![conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let pairs = chat::completed_turn_pairs_after(&conn, conversation_id, last_indexed)?;
        (
            SessionPlan {
                conversation_id,
                document_id,
                vault_path,
                scope,
                title,
                project,
                created_at,
            },
            pairs,
        )
    };

    if pairs.is_empty() {
        return Ok(Outcome::UpToDate);
    }

    // 2. Resolve the vault's embedder + build the gateway (off the lock). Refuse a width the live index
    //    can't hold (a mid-switch vault re-indexes via Rebuild, not here).
    let (embedder, embed_batch) = {
        let conn = state.conn()?;
        let embedder = crate::db::selected_embedder(&conn)?;
        ingest::guard_dimension(&conn, &embedder)?;
        (embedder, crate::db::indexing_embed_batch(&conn))
    };
    let gateway = ModelGateway::new(
        &state.sidecar,
        embedder.clone(),
        registry::reranker_for(&embedder),
    )
    .with_embed_batch(embed_batch);

    // 3. Chunk + embed each SUBSTANTIVE new pair. The triviality gate (card B's lean firehose filter)
    //    skips pure-acknowledgement/greeting pairs from the index — they are never chunked, so they can't
    //    pollute retrieval — while the cursor still advances past them below (they are handled, not
    //    reconsidered). The per-pair content-hash seed keeps leaf UIDs unique across turns (two
    //    single-paragraph turns would otherwise collide) yet stable on a rebuild.
    let chat_hash = chat::content_hash(conversation_id);
    let mut segments: Vec<IndexedSegment> = Vec::with_capacity(pairs.len());
    for pair in &pairs {
        if matches!(chat::triviality(pair), chat::Triviality::Trivial) {
            continue;
        }
        let body = render_authored_segment(pair);
        let seed = format!("{chat_hash}:{}", pair.turn_id);
        let chunks = ingest::split_document(&gateway, &body, &plan.title, &seed)?;
        let texts = ingest::leaf_embed_texts(&chunks);
        let embeddings = gateway.embed_documents(&texts)?;
        ingest::check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;
        segments.push(IndexedSegment {
            turn_id: pair.turn_id,
            at: pair.at.clone(),
            chunks,
            embeddings,
        });
    }

    // 4. Land it all in one transaction. The cursor advances to the newest pair PROCESSED — including any
    //    trivial pairs that were skipped — so a stretch of small talk is never re-examined every sweep.
    let newest = pairs
        .iter()
        .max_by_key(|p| p.turn_id)
        .expect("pairs is non-empty here");
    let indexed_turns = segments.len();
    let mut conn = state.conn()?;
    let (_doc_id, chunks) =
        commit_session_index(&mut conn, &plan, &segments, newest.turn_id, &newest.at)?;
    Ok(Outcome::Indexed {
        turns: indexed_turns,
        chunks,
    })
}

/// Land a session's pre-split, pre-embedded segments: birth the `documents` row on the first index that
/// has substance (linking it back onto the session), append every segment's chunks continuing the
/// document's ordinal sequence, and advance the index cursor to `cursor_to` — all atomically. Returns
/// `(document_id_or_none, appended_chunks)`. `cursor_to`/`cursor_at` are the newest turn-pair *processed*
/// (which may be a skipped trivial pair), so the cursor always moves past everything examined; with no
/// substantive `segments` this advances the cursor only and births nothing (a chat that has only ever
/// exchanged small talk gets no empty document). Pure DB logic (no sidecar), so the append-only
/// invariants are unit-tested directly. Caller owns the connection; this opens and commits its own
/// transaction.
fn commit_session_index(
    conn: &mut Connection,
    plan: &SessionPlan,
    segments: &[IndexedSegment],
    cursor_to: i64,
    cursor_at: &str,
) -> Result<(Option<i64>, usize)> {
    let tx = conn.transaction()?;

    // Birth the documents row only when there is substance to append (never for a trivial-only sweep).
    let doc_id = match (plan.document_id, segments.is_empty()) {
        (existing, true) => existing,
        (Some(id), false) => Some(id),
        (None, false) => {
            let meta = chat_doc_meta(plan, cursor_at);
            let id = ingest::insert_document_row(&tx, &meta)?;
            tx.execute(
                "UPDATE chat_sessions SET document_id = ?1 WHERE conversation_id = ?2",
                params![id, plan.conversation_id],
            )?;
            Some(id)
        }
    };

    let mut appended = 0usize;
    if let Some(doc_id) = doc_id {
        if !segments.is_empty() {
            // Append-only: continue after the document's highest ordinal — never renumber old chunks.
            let mut ordinal: i64 = tx.query_row(
                "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM chunks WHERE document_id = ?1",
                params![doc_id],
                |r| r.get(0),
            )?;
            for seg in segments {
                let before = ordinal;
                ordinal = ingest::append_chat_chunks(
                    &tx,
                    doc_id,
                    ordinal,
                    &seg.chunks,
                    &seg.embeddings,
                    seg.turn_id,
                    &seg.at,
                )?;
                appended += (ordinal - before) as usize;
            }
            tx.execute(
                "UPDATE documents SET last_activity = ?1 WHERE id = ?2",
                params![cursor_at, doc_id],
            )?;
        }
    }

    // Advance the index cursor past everything processed. The cursor and the chunks commit together —
    // that is the crash-safety guarantee.
    tx.execute(
        "UPDATE chat_sessions SET last_indexed_turn_id = ?1, last_active_at = ?2 \
         WHERE conversation_id = ?3",
        params![cursor_to, cursor_at, plan.conversation_id],
    )?;

    tx.commit()?;
    Ok((doc_id, appended))
}

/// The `documents` row for a chat session, born on first index. The org fields take ingest defaults
/// (the chat enters the system like a fresh document); `project` is the chat's ORIGIN — a project-scoped
/// chat belongs to its project, a general chat lands in `Unsorted` (card F may refile either). The stable
/// `chat:<id>` identity + body-independent `content_hash` are what let an append-growing chat keep one
/// UNIQUE document identity across every re-index.
fn chat_doc_meta(plan: &SessionPlan, last_at: &str) -> DocMeta {
    let project = if plan.scope == "project" {
        plan.project
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or("Unsorted")
            .to_string()
    } else {
        "Unsorted".to_string()
    };
    DocMeta {
        source_path: None,
        vault_path: plan.vault_path.clone(),
        title: plan.title.clone(),
        content_hash: chat::content_hash(plan.conversation_id),
        ext: Some("md".into()),
        byte_size: None,
        created_at: Some(plan.created_at.clone()),
        ingested_at: last_at.to_string(),
        project,
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        last_activity: Some(last_at.to_string()),
        source: SourceMeta::chat(chat::source_id(plan.conversation_id)),
    }
}

/// One turn-pair as the text we embed: **authored content only** — exactly what the user wrote and what
/// the model wrote, taken from the `messages` rows (the [`chat::TurnPair`]). The RAG context that was
/// assembled into the live prompt is structurally absent here because we never read the prompt — only
/// the authored messages. This is card B's one-line "index the authored content, never the retrieved
/// context" rule, enforced by construction.
fn render_authored_segment(pair: &chat::TurnPair) -> String {
    format!(
        "**You:** {}\n\n**PM:** {}",
        pair.user.trim(),
        pair.assistant.trim()
    )
}

/// Index every chat session that has completed turn-pairs past its cursor. The launch/idle sweep. A
/// cheap EXISTS guard skips already-caught-up chats so we never build a gateway for nothing. Best-effort
/// per session: one failure is logged and the sweep moves on.
pub(crate) fn reconcile_chat_index(state: &AppState) -> Result<Summary> {
    let candidates: Vec<i64> = {
        let conn = state.conn()?;
        let mut stmt = conn.prepare(
            "SELECT s.conversation_id FROM chat_sessions s \
             WHERE EXISTS (SELECT 1 FROM messages m \
                           WHERE m.conversation_id = s.conversation_id AND m.role = 'assistant' \
                             AND m.id > COALESCE(s.last_indexed_turn_id, 0))",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    let mut summary = Summary::default();
    for conv in candidates {
        match index_session(state, conv) {
            Ok(Outcome::Indexed { turns, chunks }) => {
                summary.sessions += 1;
                summary.turns += turns;
                summary.chunks += chunks;
            }
            Ok(Outcome::UpToDate) => {}
            Err(e) => {
                summary.failed += 1;
                eprintln!("chat-index: session {conv} failed: {e}");
            }
        }
    }
    Ok(summary)
}

/// The pure idle-gate decision, factored out so it is unit-tested without a wall-clock and so the future
/// screen-capture subsystem can share the same gate: run a background sweep only when the user has been
/// idle past `threshold`, no Drive/OneDrive sync is using the engine, and no sweep is already in flight.
pub(crate) fn should_run_now(
    idle_for: Duration,
    threshold: Duration,
    sync_active: bool,
    already_running: bool,
) -> bool {
    idle_for >= threshold && !sync_active && !already_running
}

/// Run a reconcile sweep under the single-flight guard the launch sweep and the idle loop share, so the
/// two never overlap. Blocking (it embeds) — only ever called from a blocking context.
fn run_sweep_guarded(state: &AppState, label: &str) {
    if state
        .chat_index_busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // another sweep is already in flight
    }
    let result = reconcile_chat_index(state);
    state.chat_index_busy.store(false, Ordering::SeqCst);
    match result {
        Ok(s) if s.sessions > 0 || s.failed > 0 => eprintln!(
            "chat-index: {label} indexed {} turn(s) across {} session(s) ({} failed)",
            s.turns, s.sessions, s.failed
        ),
        Ok(_) => {}
        Err(e) => eprintln!("chat-index: {label} skipped ({e})"),
    }
}

/// Poll (bounded) until the vault is unlocked and the engine is provisioned, without ever triggering a
/// first-run build. Returns false if neither happened within the launch window (the idle loop / next
/// launch will catch up).
async fn wait_until_ready(app: &AppHandle) -> bool {
    for _ in 0..LAUNCH_WAIT_TICKS {
        {
            let state = app.state::<AppState>();
            if state.conn().is_ok() && state.sidecar.is_ready() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_secs(LAUNCH_WAIT_SECS)).await;
    }
    false
}

/// Fire-and-forget the app-launch reconcile sweep: catches up any chat whose turns ran ahead of the
/// index while the app was closed (or whose live append landed but the index step never ran). Background
/// and best-effort; the heavy embed runs on a blocking thread. Modelled on
/// `commands::spawn_preferences_migration`.
pub fn spawn_launch_sweep(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if !wait_until_ready(&app).await {
            return;
        }
        let _ = tauri::async_runtime::spawn_blocking(move || {
            run_sweep_guarded(&app.state::<AppState>(), "launch sweep");
        })
        .await;
    });
}

/// Fire-and-forget the idle-time background indexer: on an idle cadence it sweeps any chat with content
/// past its cursor, so a long live session is *progressively* indexed and nothing waits for the next
/// launch. It only ever touches *completed* turn-pairs, so it is safe even on the currently-open session.
/// Never competes with active use ([`should_run_now`] gates on idle + no active sync) and shares the
/// launch sweep's single-flight guard. The minimal scheduler this card builds (there is none to hook
/// into yet); `should_run_now` is the reusable seam the screen-capture subsystem can later share.
/// Modelled on `lock_session::spawn_watcher`.
pub fn spawn_idle_indexer(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let threshold = Duration::from_secs(IDLE_THRESHOLD_SECS);
        loop {
            tokio::time::sleep(Duration::from_secs(IDLE_TICK_SECS)).await;
            let (idle, sync_active, busy, ready) = {
                let state = app.state::<AppState>();
                let ready = state.conn().is_ok() && state.sidecar.is_ready();
                (
                    state.idle_for(),
                    state.sync_active(),
                    state.chat_index_busy.load(Ordering::SeqCst),
                    ready,
                )
            };
            if !ready || !should_run_now(idle, threshold, sync_active, busy) {
                continue;
            }
            let app2 = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                run_sweep_guarded(&app2.state::<AppState>(), "idle sweep");
            })
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    /// The default `chunk_vec` width a fresh migrated store carries (v2 creates `float[384]`); our
    /// dummy embeddings must match it or the vec0 insert rejects them.
    const DIM: usize = 384;

    fn open_db(dir: &std::path::Path) -> Connection {
        crate::db::open(&dir.join("pm.sqlite"), DB_KEY).unwrap()
    }

    fn new_session(conn: &Connection, scope: &str) -> i64 {
        conn.execute(
            "INSERT INTO conversations(title, project) VALUES ('My chat', NULL)",
            [],
        )
        .unwrap();
        let conv = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope, vault_path) VALUES (?1, ?2, ?3)",
            params![conv, scope, format!("vault/chat-{conv}.md")],
        )
        .unwrap();
        conv
    }

    /// A one-leaf segment with a dummy embedding — stands in for the split+embed step so the commit's
    /// DB invariants can be exercised without the sidecar.
    fn segment(turn_id: i64, at: &str, body: &str) -> IndexedSegment {
        let chunk = splitter::Chunk {
            uid: format!("uid-{turn_id}"),
            parent_uid: None,
            kind: splitter::ChunkKind::Leaf,
            heading: None,
            display_content: body.to_string(),
            embed_content: body.to_string(),
            start_offset: 0,
            end_offset: body.len(),
        };
        IndexedSegment {
            turn_id,
            at: at.to_string(),
            chunks: vec![chunk],
            embeddings: vec![vec![0.1f32; DIM]],
        }
    }

    fn plan(conn: &Connection, conv: i64) -> SessionPlan {
        let (document_id, vault_path, scope): (Option<i64>, String, String) = conn
            .query_row(
                "SELECT document_id, vault_path, scope FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let (title, project, created_at): (String, Option<String>, String) = conn
            .query_row(
                "SELECT title, project, created_at FROM conversations WHERE id = ?1",
                params![conv],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        SessionPlan {
            conversation_id: conv,
            document_id,
            vault_path,
            scope,
            title,
            project,
            created_at,
        }
    }

    #[test]
    fn first_index_births_chat_document_and_advances_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");

        let segs = vec![segment(
            2,
            "2026-06-28T10:00:01.000Z",
            "**You:** hi\n\n**PM:** hello",
        )];
        let p = plan(&conn, conv);
        let (doc_id, appended) =
            commit_session_index(&mut conn, &p, &segs, 2, "2026-06-28T10:00:01.000Z").unwrap();
        let doc_id = doc_id.expect("a substantive sweep births the document");
        assert_eq!(appended, 1, "one chunk appended");

        // The documents row is born with the chat discriminator + stable identity.
        let (stype, sid, hash): (String, Option<String>, String) = conn
            .query_row(
                "SELECT source_type, source_id, content_hash FROM documents WHERE id = ?1",
                params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(stype, "chat");
        assert_eq!(sid, Some(chat::source_id(conv)));
        assert_eq!(hash, chat::content_hash(conv));

        // The session is linked to its document and the cursor advanced to the assistant message id.
        let (linked, cursor): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT document_id, last_indexed_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(linked, Some(doc_id));
        assert_eq!(cursor, Some(2), "cursor = newest indexed turn id");

        // The chunk carries its turn pointer + per-chunk timestamp, and is vector + FTS indexed.
        let (turn, at): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT chat_turn_id, chunk_at FROM chunks WHERE document_id = ?1",
                params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(turn, Some(2));
        assert_eq!(at.as_deref(), Some("2026-06-28T10:00:01.000Z"));
        let leaf_id: i64 = conn
            .query_row(
                "SELECT id FROM chunks WHERE document_id = ?1",
                params![doc_id],
                |r| r.get(0),
            )
            .unwrap();
        let vecs: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunk_vec WHERE rowid = ?1",
                params![leaf_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(vecs, 1, "leaf is vector-indexed");
    }

    #[test]
    fn second_sweep_appends_without_touching_old_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");

        let p = plan(&conn, conv);
        let (doc_id, _) = commit_session_index(
            &mut conn,
            &p,
            &[segment(2, "2026-06-28T10:00:01.000Z", "first")],
            2,
            "2026-06-28T10:00:01.000Z",
        )
        .unwrap();
        let doc_id = doc_id.unwrap();
        let first_chunk_id: i64 = conn
            .query_row(
                "SELECT id FROM chunks WHERE document_id = ?1 AND ordinal = 0",
                params![doc_id],
                |r| r.get(0),
            )
            .unwrap();

        // A later sweep with a new turn-pair appends to the SAME document, continuing ordinals.
        let p2 = plan(&conn, conv); // document_id is now set, so no second row is born
        let (doc_id_2, appended) = commit_session_index(
            &mut conn,
            &p2,
            &[segment(4, "2026-06-28T10:05:00.000Z", "second")],
            4,
            "2026-06-28T10:05:00.000Z",
        )
        .unwrap();
        assert_eq!(doc_id_2, Some(doc_id), "same document, not a new one");
        assert_eq!(appended, 1);

        let docs: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(docs, 1, "no duplicate chat document");

        // The original chunk is untouched (same id, ordinal 0); the new one continues at ordinal 1.
        let (ordinals, ids): (i64, i64) = conn
            .query_row(
                "SELECT count(*), min(id) FROM chunks WHERE document_id = ?1",
                params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ordinals, 2, "both turn-pairs' chunks present");
        assert_eq!(ids, first_chunk_id, "the first chunk's row is preserved");
        let new_ordinal: i64 = conn
            .query_row(
                "SELECT ordinal FROM chunks WHERE document_id = ?1 AND chat_turn_id = 4",
                params![doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_ordinal, 1, "appended after the existing chunk");

        // The cursor moved to the newest turn.
        let cursor: Option<i64> = conn
            .query_row(
                "SELECT last_indexed_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cursor, Some(4));
    }

    #[test]
    fn trivial_only_sweep_advances_cursor_without_birthing_a_document() {
        // When every new pair is trivial (skipped from embedding), `index_session` calls commit with an
        // empty segment list but a cursor past the small talk. The cursor must advance (so the chatter is
        // never re-examined) yet no empty document is born.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");

        let p = plan(&conn, conv);
        let (doc_id, appended) =
            commit_session_index(&mut conn, &p, &[], 6, "2026-06-28T10:10:00.000Z").unwrap();
        assert_eq!(doc_id, None, "no document born for a trivial-only sweep");
        assert_eq!(appended, 0);

        let docs: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(docs, 0, "no empty chat document created");

        let (linked, cursor): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT document_id, last_indexed_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(linked, None, "still unlinked");
        assert_eq!(cursor, Some(6), "but the cursor advanced past the chatter");
    }

    #[test]
    fn should_run_now_gates_on_idle_sync_and_single_flight() {
        let threshold = Duration::from_secs(900);
        // Idle long enough, nothing else running → go.
        assert!(should_run_now(
            Duration::from_secs(1000),
            threshold,
            false,
            false
        ));
        // Still active (under threshold) → wait.
        assert!(!should_run_now(
            Duration::from_secs(60),
            threshold,
            false,
            false
        ));
        // A sync is running → defer to it.
        assert!(!should_run_now(
            Duration::from_secs(1000),
            threshold,
            true,
            false
        ));
        // A sweep is already in flight → single-flight.
        assert!(!should_run_now(
            Duration::from_secs(1000),
            threshold,
            false,
            true
        ));
    }

    #[test]
    fn authored_segment_holds_only_the_two_messages() {
        let pair = chat::TurnPair {
            user: "  what should I name the org?  ".into(),
            assistant: "Atlas.".into(),
            turn_id: 2,
            at: "2026-06-28T10:00:01.000Z".into(),
        };
        let seg = render_authored_segment(&pair);
        assert_eq!(
            seg,
            "**You:** what should I name the org?\n\n**PM:** Atlas."
        );
    }
}
