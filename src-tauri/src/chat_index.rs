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

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::ingest::{self, DocMeta, SourceMeta};
use crate::model_gateway::ModelGateway;
use crate::splitter;
use crate::vault::MarkdownCipher;
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

    // 1b. Vault reconcile (F-11). The file is the authoritative truth a Rebuild reads; card A's
    //     `record_turn_pair` writes it best-effort, so a failed append can leave a pair live in `messages`
    //     — and thus about to be indexed from `messages` just below — yet absent from the file, which a
    //     later Rebuild would then silently drop. Re-run the idempotent append for every pair past the
    //     cursor so the file always carries at least what we are about to index. Off the lock (file IO); a
    //     failure aborts the sweep with the cursor untouched, so the pair is retried next sweep.
    reconcile_vault_pairs(state, &plan, &pairs)?;

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
    let (doc_id, chunks, reclassified) = {
        let mut conn = state.conn()?;
        commit_session_index(&mut conn, &plan, &segments, newest.turn_id, &newest.at)?
    };

    // If the append re-evaluated the chat's bucket (card F), mirror the new importance/reviewed back into
    // the vault front-matter so file and row agree — a Rebuild reads the file as truth (card G). Best-effort
    // and OFF the commit: the row is already authoritative; a failure just leaves the file to re-sync on the
    // next re-eval.
    if reclassified {
        if let Some(doc_id) = doc_id {
            if let Err(e) = sync_chat_frontmatter(state, doc_id) {
                eprintln!(
                    "chat_index: front-matter re-sync after re-eval failed (doc {doc_id}): {e}"
                );
            }
        }
    }

    Ok(Outcome::Indexed {
        turns: indexed_turns,
        chunks,
    })
}

/// F-11 — before indexing, ensure every completed pair we are about to index is present in the session's
/// vault file, the truth a Rebuild reads. Card A's [`chat::record_turn_pair`] appends best-effort, so a
/// failed append (locked/full disk, or a crash between the `messages` commit and the file write) can leave
/// a pair indexed from `messages` yet missing from the file — which a later Rebuild would drop.
/// [`chat::append_turn_pair`] is idempotent (keyed on the turn anchor): re-running it for every pair past
/// the cursor is a no-op for the ones already written and a self-heal for any that are missing. Runs off
/// the DB lock; a failure propagates and aborts the sweep with the cursor untouched, so the pair is retried
/// next sweep — the same best-effort contract the index step already keeps.
fn reconcile_vault_pairs(
    state: &AppState,
    plan: &SessionPlan,
    pairs: &[chat::TurnPair],
) -> Result<()> {
    let (vault_dir, cipher) = state.markdown_io()?;
    reconcile_vault_pairs_io(&vault_dir, &cipher, plan, pairs)
}

/// The file-IO core of [`reconcile_vault_pairs`], split out so the reconcile is unit-tested with a real
/// cipher + temp vault and no `AppState`. Appends every pair idempotently, in the caller's (cursor-ascending)
/// order. The front-matter arguments are consulted only when the file must be created — the rare case where
/// even card A's first append never landed — and mirror exactly what `record_turn_pair` writes, so a
/// self-healed file is indistinguishable from a normally-grown one.
fn reconcile_vault_pairs_io(
    vault_dir: &Path,
    cipher: &MarkdownCipher,
    plan: &SessionPlan,
    pairs: &[chat::TurnPair],
) -> Result<()> {
    let project = plan
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("Unsorted");
    for pair in pairs {
        chat::append_turn_pair(
            vault_dir,
            cipher,
            &plan.vault_path,
            &plan.title,
            plan.conversation_id,
            &plan.scope,
            project,
            &plan.created_at,
            &pair.at,
            pair,
        )?;
    }
    Ok(())
}

/// Mirror a re-evaluated chat's classification from the (already-committed) `documents` row into its vault
/// front-matter, so file and row stay consistent for a later Rebuild. Reads the current importance, reviewed
/// flag, and vault path under a short lock, then patches the file off the lock via
/// [`chat::rewrite_chat_classification`], which touches only those two front-matter scalars (the chat
/// identity and body are preserved).
fn sync_chat_frontmatter(state: &AppState, doc_id: i64) -> Result<()> {
    let (vault_path, importance, reviewed) = {
        let conn = state.conn()?;
        conn.query_row(
            "SELECT vault_path, importance, reviewed FROM documents WHERE id = ?1",
            params![doc_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)? != 0,
                ))
            },
        )?
    };
    let (vault, cipher) = state.markdown_io()?;
    chat::rewrite_chat_classification(
        &cipher,
        &vault.join(&vault_path),
        importance.as_deref(),
        reviewed,
    )
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
) -> Result<(Option<i64>, usize, bool)> {
    let tx = conn.transaction()?;

    // Guard against a delete that raced this sweep. `index_session` reads the plan under one lock, RELEASES
    // it to embed off-lock, then re-acquires to commit here — and `delete_conversation` does NOT take the
    // chat-index single-flight guard, so it can delete the conversation (cascading its session row) in that
    // window. If it did, birthing the `(None, false)` documents row below would strand an orphan: a chat
    // document for a conversation that no longer exists, with no FK back to clean it up, retrievable
    // forever. The store is one mutex'd connection, so any racing delete has already committed by the time
    // we hold this transaction's lock — a single existence check closes the window. Nothing survives to
    // index, so abandon the commit (the empty tx rolls back on drop).
    let conversation_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
        params![plan.conversation_id],
        |r| r.get(0),
    )?;
    if !conversation_exists {
        return Ok((None, 0, false));
    }

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
    let mut reclassified = false;
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
            // Card F append re-evaluation: substantive new turns on an ALREADY-EXISTING chat row can turn a
            // throwaway (archived) or already-filed chat into a real discussion — the classification must not
            // be sticky. Only fires on an append (`plan.document_id` was set on entry), never on the birth
            // above (which was just classified). Keys on the appended turns, not the whole history.
            if plan.document_id.is_some() {
                reclassified = reevaluate_on_append(&tx, doc_id, &plan.scope)?;
            }
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
    Ok((doc_id, appended, reclassified))
}

/// The `documents` row for a chat session, born on first index. Card F routing keys on the chat's ORIGIN
/// scope: a **project** chat is born already-filed — linked to its project, HIGH importance (most relevant
/// and recent), and `reviewed: true` so it skips the review queue (already scoped and trusted). A
/// **general** chat takes ingest defaults — `Unsorted`, no importance, `reviewed: false` — so it lands in
/// the review queue for the AI to propose project/tags/importance and the user to approve (card F refiles).
/// This mirrors the vault front-matter [`chat::render_chat_frontmatter`] writes at creation, so file and
/// row agree without a cross-write. The stable `chat:<id>` identity + body-independent `content_hash` are
/// what let an append-growing chat keep one UNIQUE document identity across every re-index.
fn chat_doc_meta(plan: &SessionPlan, last_at: &str) -> DocMeta {
    let is_project = plan.scope == "project";
    let project = if is_project {
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
        importance: is_project.then(|| "high".to_string()),
        reviewed: is_project,
        last_activity: Some(last_at.to_string()),
        source: SourceMeta::chat(chat::source_id(plan.conversation_id)),
    }
}

/// Card F append re-evaluation: when substantive new turns land on a chat's already-existing `documents`
/// row, re-open its classification so a sticky bucket can't quietly bury content (an archived "what's the
/// weather" chat that becomes a real planning session must not stay archived + downranked). Applied to the
/// appended turns only — the caller fires this exactly when the sweep produced new chunks. Pure DB, inside
/// the caller's transaction, so it commits atomically with the chunks + cursor.
///
/// The rules, by the doc's current `(importance, reviewed)` and the chat's ORIGIN `scope`:
/// - **Archived, project chat** → un-archive back to the trusted-scope defaults (`high` / `reviewed`).
/// - **Archived, general chat** → un-archive and re-open the review queue (`importance` cleared,
///   `reviewed = 0`) so the AI re-proposes on the new content.
/// - **Filed (reviewed) general chat** → re-open the queue (`reviewed = 0`) for another review pass, since
///   the appended turns may change its bucket. (Accepted review-queue churn on an actively-used chat.)
/// - **Project chat, not archived** → no-op: it is already `high` / `reviewed` / filed to its project.
///
/// Returns `true` iff it changed the row's classification — the caller uses that to mirror the change back
/// into the vault front-matter (so file and row agree across a Rebuild).
fn reevaluate_on_append(tx: &Connection, doc_id: i64, scope: &str) -> Result<bool> {
    let (importance, reviewed): (Option<String>, bool) = tx.query_row(
        "SELECT importance, reviewed FROM documents WHERE id = ?1",
        params![doc_id],
        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
    )?;
    let archived = importance.as_deref() == Some("archive");
    let is_project = scope == "project";

    if archived && is_project {
        tx.execute(
            "UPDATE documents SET importance = 'high', reviewed = 1 WHERE id = ?1",
            params![doc_id],
        )?;
        Ok(true)
    } else if archived {
        // General chat: clear the archive shelving and send it back through review.
        tx.execute(
            "UPDATE documents SET importance = NULL, reviewed = 0 WHERE id = ?1",
            params![doc_id],
        )?;
        Ok(true)
    } else if !is_project && reviewed {
        // Already-filed general chat: re-open review so the new turns can re-file it.
        tx.execute(
            "UPDATE documents SET reviewed = 0 WHERE id = ?1",
            params![doc_id],
        )?;
        Ok(true)
    } else {
        // Project chat that is not archived is already filed correctly — nothing to do.
        Ok(false)
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
    // F-04: birthing a chat document resolves its scope project with `create_if_new`, which can mint
    // a mirror entity. This sweep runs OUTSIDE `ingest::rebuild` (which syncs at its own end), so push
    // any such mint out to the portable rules file here — otherwise the next session's mirror rebuild
    // (the file is truth) would roll the new entity back. Once per sweep, only when something indexed;
    // best-effort + a byte-identical no-op when nothing was minted.
    if summary.sessions > 0 {
        state.sync_entity_rules();
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
    let Some(_guard) = crate::BusyGuard::acquire(&state.chat_index_busy) else {
        return; // another sweep is already in flight
    };
    // `_guard` resets the flag on drop — including if the sweep panics — so indexing can't wedge.
    let result = reconcile_chat_index(state);
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
    fn commit_abandons_birth_when_the_conversation_was_deleted_mid_sweep() {
        // Card 7G / M4 fix: index_session reads the plan, releases the lock to embed, then commits here.
        // A delete can land in that window (it does not take the chat-index single-flight). If it does we
        // must NOT birth a documents row — it would be an orphan with no conversation and no FK to reap it.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");
        // Plan captured while the conversation still exists (document_id NULL ⇒ the birth branch).
        let p = plan(&conn, conv);
        let segs = vec![segment(
            2,
            "2026-06-28T10:00:01.000Z",
            "**You:** hi\n\n**PM:** hello",
        )];

        // The racing delete: remove the conversation (its chat_sessions row cascades away).
        conn.execute("DELETE FROM conversations WHERE id = ?1", params![conv])
            .unwrap();

        let (doc_id, appended, _) =
            commit_session_index(&mut conn, &p, &segs, 2, "2026-06-28T10:00:01.000Z").unwrap();
        assert_eq!(
            doc_id, None,
            "no document is born for a deleted conversation"
        );
        assert_eq!(appended, 0);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM documents", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "no orphan documents row survives the raced delete",
        );
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
        let (doc_id, appended, _) =
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
        let (doc_id, _, _) = commit_session_index(
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
        let (doc_id_2, appended, _) = commit_session_index(
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
        let (doc_id, appended, _) =
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

    /// A project-scoped session: `conversations.project` set (the ORIGIN), `chat_sessions.scope='project'`.
    fn new_project_session(conn: &Connection, project: &str) -> i64 {
        conn.execute(
            "INSERT INTO conversations(title, project) VALUES ('My chat', ?1)",
            params![project],
        )
        .unwrap();
        let conv = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope, vault_path) VALUES (?1, 'project', ?2)",
            params![conv, format!("vault/chat-{conv}.md")],
        )
        .unwrap();
        conv
    }

    fn doc_org(conn: &Connection, doc_id: i64) -> (String, Option<String>, bool) {
        conn.query_row(
            "SELECT project, importance, reviewed FROM documents WHERE id = ?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
        )
        .unwrap()
    }

    #[test]
    fn general_chat_is_born_unsorted_for_the_review_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");
        let p = plan(&conn, conv);
        let (doc_id, _, _) = commit_session_index(
            &mut conn,
            &p,
            &[segment(2, "2026-06-28T10:00:01.000Z", "hi")],
            2,
            "2026-06-28T10:00:01.000Z",
        )
        .unwrap();
        let (project, importance, reviewed) = doc_org(&conn, doc_id.unwrap());
        assert_eq!(project, "Unsorted");
        assert_eq!(importance, None, "no importance until the user reviews it");
        assert!(!reviewed, "general chat lands in the review queue");
    }

    #[test]
    fn project_chat_is_born_filed_and_skips_the_review_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_project_session(&conn, "Atlas - PM");
        let p = plan(&conn, conv);
        let (doc_id, _, _) = commit_session_index(
            &mut conn,
            &p,
            &[segment(2, "2026-06-28T10:00:01.000Z", "hi")],
            2,
            "2026-06-28T10:00:01.000Z",
        )
        .unwrap();
        let (project, importance, reviewed) = doc_org(&conn, doc_id.unwrap());
        assert_eq!(project, "Atlas - PM", "auto-linked to its origin project");
        assert_eq!(
            importance.as_deref(),
            Some("high"),
            "most relevant + recent"
        );
        assert!(reviewed, "trusted scope skips the review queue");
    }

    /// Card F append re-evaluation. Helper: birth a chat doc, force it into a `(importance, reviewed)`
    /// state, then append one more substantive turn and return the re-evaluated org.
    fn append_after(
        conn: &mut Connection,
        conv: i64,
        set_importance: Option<&str>,
        set_reviewed: bool,
    ) -> (Option<String>, bool) {
        let p = plan(conn, conv);
        let (doc_id, _, _) = commit_session_index(
            conn,
            &p,
            &[segment(2, "2026-06-28T10:00:01.000Z", "first")],
            2,
            "2026-06-28T10:00:01.000Z",
        )
        .unwrap();
        let doc_id = doc_id.unwrap();
        conn.execute(
            "UPDATE documents SET importance = ?1, reviewed = ?2 WHERE id = ?3",
            params![set_importance, set_reviewed as i64, doc_id],
        )
        .unwrap();
        // A later substantive turn on the now-existing row triggers re-evaluation.
        let p2 = plan(conn, conv);
        commit_session_index(
            conn,
            &p2,
            &[segment(
                4,
                "2026-06-28T10:05:00.000Z",
                "a real discussion now",
            )],
            4,
            "2026-06-28T10:05:00.000Z",
        )
        .unwrap();
        let (_, importance, reviewed) = doc_org(conn, doc_id);
        (importance, reviewed)
    }

    #[test]
    fn append_unarchives_and_requeues_a_general_chat() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");
        let (importance, reviewed) = append_after(&mut conn, conv, Some("archive"), false);
        assert_eq!(importance, None, "un-archived");
        assert!(
            !reviewed,
            "re-opened for review so the new content is re-proposed"
        );
    }

    #[test]
    fn append_unarchives_a_project_chat_back_to_high() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_project_session(&conn, "Atlas - PM");
        let (importance, reviewed) = append_after(&mut conn, conv, Some("archive"), false);
        assert_eq!(
            importance.as_deref(),
            Some("high"),
            "trusted scope returns to high"
        );
        assert!(reviewed, "still skips the queue");
    }

    #[test]
    fn append_requeues_an_already_filed_general_chat() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");
        let (importance, reviewed) = append_after(&mut conn, conv, Some("medium"), true);
        assert_eq!(
            importance.as_deref(),
            Some("medium"),
            "importance preserved"
        );
        assert!(!reviewed, "re-opened for another review pass");
    }

    #[test]
    fn append_leaves_an_active_project_chat_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_project_session(&conn, "Atlas - PM");
        let (importance, reviewed) = append_after(&mut conn, conv, Some("high"), true);
        assert_eq!(importance.as_deref(), Some("high"));
        assert!(reviewed, "already filed correctly — a no-op");
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

    /// Create the vault file at exactly the path `delete_conversation_inner` will compute
    /// (`vault_dir.join(stored vault_path)`), and return that path so the test can assert its removal.
    fn materialise_vault_file(
        conn: &Connection,
        vault_dir: &std::path::Path,
        conv: i64,
    ) -> std::path::PathBuf {
        let stored: String = conn
            .query_row(
                "SELECT vault_path FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| r.get(0),
            )
            .unwrap();
        let file = vault_dir.join(stored);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "chat body").unwrap();
        file
    }

    fn count(conn: &Connection, sql: &str, id: i64) -> i64 {
        conn.query_row(sql, params![id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn deleting_an_indexed_chat_purges_document_chunks_vectors_fts_and_vault_file() {
        // The card 7G cascade end-to-end: an indexed chat leaves nothing behind — not the conversation,
        // its messages, its session row, its document, its chunks, the two rowid-keyed mirrors, or its
        // vault file.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = new_session(&conn, "general");
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1,'user','hi'),(?1,'assistant','hello')",
            params![conv],
        )
        .unwrap();

        let segs = vec![segment(
            2,
            "2026-06-28T10:00:01.000Z",
            "**You:** hi\n\n**PM:** hello",
        )];
        let p = plan(&conn, conv);
        let (doc_id, _, _) =
            commit_session_index(&mut conn, &p, &segs, 2, "2026-06-28T10:00:01.000Z").unwrap();
        let doc_id = doc_id.expect("a substantive sweep births the document");

        let vault_dir = dir.path().join("md");
        let file = materialise_vault_file(&conn, &vault_dir, conv);
        assert!(file.exists());

        chat::delete_conversation_inner(&conn, &vault_dir, conv).unwrap();

        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM conversations WHERE id=?1",
                conv
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM messages WHERE conversation_id=?1",
                conv
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chat_sessions WHERE conversation_id=?1",
                conv
            ),
            0
        );
        assert_eq!(
            count(&conn, "SELECT count(*) FROM documents WHERE id=?1", doc_id),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE document_id=?1",
                doc_id
            ),
            0
        );
        let vec_total: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vec", [], |r| r.get(0))
            .unwrap();
        let fts_total: i64 = conn
            .query_row("SELECT count(*) FROM chunks_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vec_total, 0, "vector mirror purged");
        assert_eq!(fts_total, 0, "fts mirror purged");
        assert!(!file.exists(), "vault file removed");
    }

    #[test]
    fn deleting_a_not_yet_indexed_conversation_removes_conversation_and_messages() {
        // A brand-new chat that never recorded a turn-pair has no session row and no document; delete must
        // still clear the conversation + its messages and not error on the absent document/file.
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path());
        conn.execute(
            "INSERT INTO conversations(title, project) VALUES ('Fresh', NULL)",
            [],
        )
        .unwrap();
        let conv = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1,'user','unsent thought')",
            params![conv],
        )
        .unwrap();

        chat::delete_conversation_inner(&conn, &dir.path().join("md"), conv).unwrap();

        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM conversations WHERE id=?1",
                conv
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM messages WHERE conversation_id=?1",
                conv
            ),
            0
        );
    }

    #[test]
    fn deleting_one_chat_leaves_a_second_chats_chunks_intact() {
        // Deleting one indexed chat must not touch another's document/chunks/vectors.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let index_one = |conn: &mut Connection| -> (i64, i64) {
            let conv = new_session(conn, "general");
            let segs = vec![segment(
                2,
                "2026-06-28T10:00:01.000Z",
                "**You:** hi\n\n**PM:** hello",
            )];
            let p = plan(conn, conv);
            let (doc_id, _, _) =
                commit_session_index(conn, &p, &segs, 2, "2026-06-28T10:00:01.000Z").unwrap();
            (conv, doc_id.unwrap())
        };
        let (keep_conv, keep_doc) = index_one(&mut conn);
        let (drop_conv, drop_doc) = index_one(&mut conn);

        chat::delete_conversation_inner(&conn, &dir.path().join("md"), drop_conv).unwrap();

        // The dropped chat is gone...
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM documents WHERE id=?1",
                drop_doc
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE document_id=?1",
                drop_doc
            ),
            0
        );
        // ...the kept chat is fully intact: conversation, document, chunk, and both mirrors.
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM conversations WHERE id=?1",
                keep_conv
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM documents WHERE id=?1",
                keep_doc
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE document_id=?1",
                keep_doc
            ),
            1
        );
        let vec_total: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vec_total, 1, "only the kept chat's vector remains");
    }

    // --- F-11: the launch sweep's vault reconcile ---

    fn reconcile_plan() -> SessionPlan {
        SessionPlan {
            conversation_id: 7,
            document_id: None,
            vault_path: "chat-test-7.md".to_string(),
            scope: "general".to_string(),
            title: "My chat".to_string(),
            project: None,
            created_at: "2026-06-28T10:00:00.000Z".to_string(),
        }
    }

    fn reconcile_pair(turn_id: i64, at: &str, user: &str, assistant: &str) -> chat::TurnPair {
        chat::TurnPair {
            user: user.to_string(),
            assistant: assistant.to_string(),
            turn_id,
            at: at.to_string(),
        }
    }

    #[test]
    fn launch_sweep_reconciles_a_vault_that_trails_its_messages() {
        // F-11: card A appended pair 2 to the vault, but its append for pair 4 failed — pair 4 is live in
        // `messages` and about to be indexed from there. Before F-11 the sweep indexed it yet never wrote it
        // to the file, so a later Rebuild (file = truth) dropped it. The reconcile must re-append the missing
        // pair, and be a pure no-op for the pair already present.
        let dir = tempfile::tempdir().unwrap();
        let cipher = MarkdownCipher::plaintext("v");
        let plan = reconcile_plan();
        let path = dir.path().join(&plan.vault_path);
        let read = |p: &std::path::Path| cipher.decode(&std::fs::read(p).unwrap(), p).unwrap();

        let pairs = vec![
            reconcile_pair(2, "2026-06-28T10:00:01.000Z", "hi", "hello"),
            reconcile_pair(4, "2026-06-28T10:05:00.000Z", "more", "ok"),
        ];

        // Card A's successful append of pair 2 only (pair 4 never made it to the file).
        reconcile_vault_pairs_io(dir.path(), &cipher, &plan, &pairs[..1]).unwrap();
        let before = read(&path);
        assert!(before.contains("<!-- turn 2 ·"));
        assert!(
            !before.contains("<!-- turn 4 ·"),
            "pair 4 is missing from truth — the trailing gap the sweep must close"
        );

        // The launch sweep sees [2, 4] past the cursor and reconciles the file before indexing.
        reconcile_vault_pairs_io(dir.path(), &cipher, &plan, &pairs).unwrap();
        let after = read(&path);
        assert_eq!(
            after.matches("<!-- turn 2 ·").count(),
            1,
            "the already-present pair is not duplicated"
        );
        assert_eq!(
            after.matches("<!-- turn 4 ·").count(),
            1,
            "the missing pair is self-healed into truth exactly once"
        );
        assert!(after.contains("**You:** more") && after.contains("**PM:** ok"));

        // Idempotent: re-running the reconcile appends nothing.
        reconcile_vault_pairs_io(dir.path(), &cipher, &plan, &pairs).unwrap();
        assert_eq!(read(&path), after, "a second reconcile is a pure no-op");
    }

    #[test]
    fn launch_sweep_reconcile_recreates_a_missing_vault_file() {
        // The pathological case: even card A's first append never landed, so there is no file at all. The
        // reconcile must create it with the correct chat front-matter (so a Rebuild reads it as a chat) plus
        // the missing turn — never leave the pair indexed-but-untruthed.
        let dir = tempfile::tempdir().unwrap();
        let cipher = MarkdownCipher::plaintext("v");
        let plan = reconcile_plan();
        let path = dir.path().join(&plan.vault_path);
        assert!(!path.exists(), "no vault file yet");

        let pairs = vec![reconcile_pair(2, "2026-06-28T10:00:01.000Z", "hi", "hello")];
        reconcile_vault_pairs_io(dir.path(), &cipher, &plan, &pairs).unwrap();

        let content = cipher
            .decode(&std::fs::read(&path).unwrap(), &path)
            .unwrap();
        let (fields, body) = ingest::parse_frontmatter(&content).expect("front-matter parses");
        assert_eq!(fields.get("source_type").map(String::as_str), Some("chat"));
        assert_eq!(
            fields.get("chat_scope").map(String::as_str),
            Some("general")
        );
        assert_eq!(
            fields.get("chat_conversation_id").map(String::as_str),
            Some("7")
        );
        assert!(body.contains("**You:** hi") && body.contains("**PM:** hello"));
    }
}
