// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat ingestion foundation (board card 7A, #140) — the data-model + write-discipline substrate that
//! lets PM's own conversations become a first-class ingestion source, modelled like a document.
//!
//! This module owns the parts that are painful to retrofit once real chat data exists, and nothing more:
//!
//!   * **The turn model.** A *turn-pair* is one user message plus its assistant reply; its identity is the
//!     assistant message's `messages.id` (monotonic, append-only). A pair is complete only once the
//!     assistant reply exists. [`completed_turn_pairs_after`] is the read primitive card B/C consume; it
//!     never returns an incomplete trailing user turn. [`assert_user_turn_allowed`] enforces strict
//!     alternation (user → assistant → user) so a turn-pair is always unambiguous.
//!   * **Vault-is-truth write ordering.** A chat session has a real Markdown vault file — flat in the same
//!     `vault/` dir as documents, so a Rebuild glob and the deletion cascade (card G) cover it for free.
//!     [`record_turn_pair`] appends each completed pair to that file **first and authoritatively** (the
//!     embed + cursor-advance is a separate, later step in card B). The append is idempotent, keyed on the
//!     turn id, so a re-run never duplicates a turn — and if the process dies after the vault append but
//!     before card B embeds, the next pass simply re-reads the vault tail past the cursor.
//!   * **The session row.** [`ensure_session`] upserts the thin `chat_sessions` satellite (scope, the
//!     session's vault path, last-active). The two cursors on that row are written by cards B and C; this
//!     card leaves them NULL.
//!
//! Later cards extend this module: embedding/indexing (B) and context assembly + rolling summary (C)
//! live elsewhere, but card G's deletion cascade — [`delete_conversation_inner`] — now lives here, beside
//! the write path it inverts (the Rebuild re-embed sweep it pairs with is `ingest::rebuild`). The
//! `documents` row a chat becomes — carrying project/importance/source_state and the stable
//! `source_id`/`content_hash` reserved here — is created by card B, not this card.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Result};
use crate::ingest;
use crate::vault::MarkdownCipher;
use crate::AppState;

/// The stable identity a chat session's `documents` row will carry (card B). Reused as the vault
/// front-matter `chat_source_id` and as the seed for [`content_hash`], so identity is decoupled from the
/// mutable, append-growing body — the key that makes append-only re-indexing safe at the document level.
pub(crate) fn source_id(conversation_id: i64) -> String {
    format!("chat:{conversation_id}")
}

/// The chat document's stable `content_hash`: a hash of the immutable [`source_id`], NOT of the body.
/// A growing chat must keep one stable, UNIQUE identity across appends, so hashing the body (which
/// changes every turn) would break the dedupe model. Card B writes this onto the `documents` row.
pub(crate) fn content_hash(conversation_id: i64) -> String {
    ingest::hex_digest(source_id(conversation_id).as_bytes())
}

/// A completed turn-pair: one user message and its assistant reply. `turn_id` is the assistant message's
/// id (the pair's monotonic identity); `at` is that message's timestamp.
pub(crate) struct TurnPair {
    pub user: String,
    pub assistant: String,
    pub turn_id: i64,
    pub at: String,
}

/// Refuse a second consecutive user turn — strict alternation, enforced at the write layer. The UI already
/// maintains alternation (input is blocked while a reply streams), so this is an invariant guard: it keeps
/// the turn-pair unit unambiguous and ensures there is never an unpaired user message for the indexer to
/// reason about. Called just before a user message is inserted.
pub(crate) fn assert_user_turn_allowed(conn: &Connection, conversation_id: i64) -> Result<()> {
    let last: Option<String> = conn
        .query_row(
            "SELECT role FROM messages WHERE conversation_id = ?1 ORDER BY id DESC LIMIT 1",
            params![conversation_id],
            |r| r.get(0),
        )
        .optional()?;
    if last.as_deref() == Some("user") {
        return Err(Error::Other(
            "a user turn is already awaiting a reply (strict alternation)".into(),
        ));
    }
    Ok(())
}

/// Discard a trailing reply-less user turn, returning whether one was removed.
///
/// Such a turn is the wreckage of a previous `send_message` that inserted the user row but never
/// recorded a reply — its stream failed (network drop, provider 4xx/5xx, an over-window prompt, or the
/// streaming read-stall) or the process died between persisting the turn and its reply. Left in place it
/// trips [`assert_user_turn_allowed`] on *every* subsequent send, so one transient failure wedges the
/// conversation permanently — the only prior escape was deleting the whole chat (F-02 / B5-1).
///
/// Removing it is safe because this row was never truth: [`record_turn_pair`] appends only a *completed*
/// pair to the vault, and the indexer reads only [`completed_turn_pairs_after`], which excludes an
/// unpaired trailing user turn — so the orphan exists solely in the `messages` table and is neither
/// vault-written nor indexed. The user is simply resending; the failed attempt is dropped. This covers
/// both the stream-error path (cleaned on the user's next send) and an orphan persisted across a crash.
///
/// Deletes by the most-recent row's id, and only when that row is a user turn: the alternation guard
/// prevents a second user row from ever stacking behind a pending one, so at most one dangling turn can
/// exist and it is always the last — a mid-history user message can never be caught. Idempotent: a no-op
/// when the last row is an assistant reply, or the conversation is empty. Relies on the same UI
/// serialization [`assert_user_turn_allowed`] documents (input blocked while a reply streams), so a
/// still-streaming turn is never mistaken for an orphan.
pub(crate) fn discard_dangling_user_turn(conn: &Connection, conversation_id: i64) -> Result<bool> {
    let last: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, role FROM messages WHERE conversation_id = ?1 ORDER BY id DESC LIMIT 1",
            params![conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, role)) = last {
        if role == "user" {
            conn.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Every completed turn-pair in a conversation whose assistant message id is greater than `after_turn_id`
/// (pass `None` for "from the beginning"), in chronological order. A trailing user message with no reply
/// yet is **excluded** — only complete pairs are returned. This is the cursor-driven read card B embeds
/// from and card C summarises from: "is there content past the cursor?" never "is the conversation done?".
pub(crate) fn completed_turn_pairs_after(
    conn: &Connection,
    conversation_id: i64,
    after_turn_id: Option<i64>,
) -> Result<Vec<TurnPair>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, created_at FROM messages \
         WHERE conversation_id = ?1 AND id > ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![conversation_id, after_turn_id.unwrap_or(0)], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut pairs = Vec::new();
    let mut pending_user: Option<String> = None;
    for row in rows {
        let (id, role, content, at) = row?;
        match role.as_str() {
            "user" => pending_user = Some(content),
            "assistant" => {
                if let Some(user) = pending_user.take() {
                    pairs.push(TurnPair {
                        user,
                        assistant: content,
                        turn_id: id,
                        at,
                    });
                }
            }
            // 'system' is never stored in `messages` (system prompts are assembled in memory), but ignore
            // it defensively rather than letting it split a pair.
            _ => {}
        }
    }
    Ok(pairs)
}

/// The session's vault filename: `chat-<DD-MM-YYYY>-<short>.md`, DD-MM per the house date style and
/// `<short>` the first 12 chars of the stable [`content_hash`] (collision-resistant + stable across
/// appends). Analogous to `ingest::vault_filename` for documents.
pub(crate) fn chat_vault_filename(conversation_id: i64, created_at: &str) -> String {
    let date = ddmmyyyy(created_at);
    let short: String = content_hash(conversation_id).chars().take(12).collect();
    format!("chat-{date}-{short}.md")
}

/// Append one completed turn-pair to a session's vault file, **idempotently**. Creates the file with chat
/// front-matter on the first pair; on a re-run for a turn already present (keyed on the turn-id anchor) it
/// is a no-op. The file is the authoritative source of truth — written before any embedding.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_turn_pair(
    vault_dir: &Path,
    cipher: &MarkdownCipher,
    on_disk_name: &str,
    title: &str,
    conversation_id: i64,
    scope: &str,
    project: &str,
    created_at: &str,
    ingested_at: &str,
    pair: &TurnPair,
) -> Result<()> {
    let path = vault_dir.join(on_disk_name);
    let mut content = if path.exists() {
        let bytes = std::fs::read(&path)?;
        cipher.decode(&bytes, &path)?
    } else {
        render_chat_frontmatter(
            title,
            conversation_id,
            scope,
            project,
            created_at,
            ingested_at,
        )
    };
    // Idempotent: skip a turn already present (re-run / crash-recovery re-fire). The anchor carries the
    // turn id, which card B also reads to map the vault tail back to turn ids.
    if content.contains(&turn_anchor(pair.turn_id)) {
        return Ok(());
    }
    // Separate blocks by exactly one blank line.
    if !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.ends_with("\n\n") {
        content.push('\n');
    }
    content.push_str(&render_turn_block(pair));
    cipher.write_to(&path, &content)?;
    Ok(())
}

/// Upsert the `chat_sessions` satellite for a conversation, refreshing its vault path + last-active. Does
/// NOT touch the cursors (`last_indexed_turn_id` / `summary_covers_up_to_turn_id`) — cards B and C own those.
pub(crate) fn ensure_session(
    conn: &Connection,
    conversation_id: i64,
    scope: &str,
    vault_path: &str,
    last_active: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO chat_sessions(conversation_id, scope, vault_path, last_active_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(conversation_id) DO UPDATE SET \
             vault_path = excluded.vault_path, last_active_at = excluded.last_active_at",
        params![conversation_id, scope, vault_path, last_active],
    )?;
    Ok(())
}

/// Record a completed turn-pair: append it to the session's vault file (authoritative truth, first), then
/// upsert the session row. Best-effort by contract — the caller treats a failure as non-fatal because the
/// committed `messages` rows are the backstop: card B's launch sweep reconciles any session whose messages
/// run ahead of its vault, re-running the idempotent [`append_turn_pair`] for every pair past the index
/// cursor before it indexes them (see `chat_index::reconcile_vault_pairs`), so a failed append here is
/// self-healed into the file — truth — rather than silently lost on a later Rebuild. Holds the DB lock only
/// for quick reads/writes, never across the file IO.
pub(crate) fn record_turn_pair(
    state: &AppState,
    conversation_id: i64,
    user: &str,
    assistant: &str,
    turn_id: i64,
) -> Result<()> {
    let (vault_dir, cipher) = state.markdown_io()?;

    // One short lock to gather what the file write needs; dropped before the IO below.
    let (title, project, created_at, msg_at, existing_path) = {
        let conn = state.conn()?;
        let (title, project, created_at): (String, Option<String>, String) = conn.query_row(
            "SELECT title, project, created_at FROM conversations WHERE id = ?1",
            params![conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let msg_at: String = conn.query_row(
            "SELECT created_at FROM messages WHERE id = ?1",
            params![turn_id],
            |r| r.get(0),
        )?;
        let existing_path: Option<String> = conn
            .query_row(
                "SELECT vault_path FROM chat_sessions WHERE conversation_id = ?1",
                params![conversation_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        (title, project, created_at, msg_at, existing_path)
    };

    // Scope is ORIGIN: a chat opened global vs scoped to a project (the `conversations.project` set at
    // creation). A project chat is filed to this origin project at birth; a general chat is later filed
    // by review (card F). `origin_project` is that creation-time project, NOT a post-review destination.
    let origin_project = project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let scope = if origin_project.is_some() {
        "project"
    } else {
        "general"
    };
    let on_disk = existing_path
        .unwrap_or_else(|| cipher.on_disk_name(&chat_vault_filename(conversation_id, &created_at)));
    let pair = TurnPair {
        user: user.to_string(),
        assistant: assistant.to_string(),
        turn_id,
        at: msg_at.clone(),
    };

    // Vault FIRST (authoritative), off the lock.
    append_turn_pair(
        &vault_dir,
        &cipher,
        &on_disk,
        &title,
        conversation_id,
        scope,
        origin_project.unwrap_or("Unsorted"),
        &created_at,
        &msg_at,
        &pair,
    )?;

    // Then record/refresh the session row.
    let conn = state.conn()?;
    ensure_session(&conn, conversation_id, scope, &on_disk, &msg_at)?;
    Ok(())
}

/// Delete a conversation and everything it produced (board card 7G), given an already-locked connection
/// and the resolved vault dir. The DB half runs in one transaction: purge the chat's `documents` row +
/// chunks + vector/FTS mirrors (via [`ingest::delete_document`]) when it was ever indexed, then delete the
/// `conversations` row — which cascades `messages` and the `chat_sessions` satellite (both ON DELETE
/// CASCADE). A brand-new chat with no recorded turn-pair has no session row, so there's nothing on the
/// document side to purge. The vault file is removed AFTER the commit, best-effort: a leftover orphan file
/// is harmless and self-healing (a future Rebuild re-embeds it as a doc with no conversation), whereas
/// removing it before a failed commit would strand a live session pointing at a truth file that's gone —
/// leftover file ≪ dangling live session. Local unlink only, so no network runs under the lock.
pub(crate) fn delete_conversation_inner(
    conn: &Connection,
    vault_dir: &Path,
    conversation_id: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let vault_file = delete_conversation_rows(&tx, conversation_id)?;
    tx.commit()?;

    if let Some(rel) = vault_file {
        let _ = std::fs::remove_file(vault_dir.join(rel));
    }
    Ok(())
}

/// The DB half of a conversation delete, **inside the caller's transaction**. Returns the session's
/// vault path *relative to the vault dir*, for the caller to unlink once the commit is durable.
///
/// Split out of [`delete_conversation_inner`] so a caller that is already inside a transaction can
/// reuse the exact same cascade — SQLite has no nested `BEGIN`, so the wrapper's own
/// `unchecked_transaction` would fail there. Project deletion (#573) deletes many conversations
/// inside one entity-mutation transaction and unlinks their files together after the commit, for
/// the same reason this function doesn't unlink: a leftover file is harmless and self-healing,
/// whereas removing it before a failed commit strands a live session pointing at truth that's gone.
pub(crate) fn delete_conversation_rows(
    tx: &Connection,
    conversation_id: i64,
) -> Result<Option<String>> {
    // The session row exists only once a turn-pair has been appended; a never-indexed chat has none.
    let session: Option<(Option<i64>, Option<String>)> = tx
        .query_row(
            "SELECT document_id, vault_path FROM chat_sessions WHERE conversation_id = ?1",
            params![conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let vault_file = match session {
        Some((doc_id, vault_path)) => {
            if let Some(doc_id) = doc_id {
                crate::ingest::delete_document(tx, doc_id)?;
            }
            vault_path.filter(|p| !p.trim().is_empty())
        }
        None => None,
    };
    // Cascades `messages` and the `chat_sessions` satellite (both ON DELETE CASCADE).
    tx.execute(
        "DELETE FROM conversations WHERE id = ?1",
        params![conversation_id],
    )?;
    Ok(vault_file)
}

// --- rendering ---

/// The HTML-comment turn anchor prefix, carrying the turn id. Idempotency keys on this; card B reads it to
/// map a vault block back to its turn id.
fn turn_anchor(turn_id: i64) -> String {
    format!("<!-- turn {turn_id} ·")
}

/// One turn-pair as Markdown: an id-bearing anchor, then the user text, then the assistant text.
fn render_turn_block(pair: &TurnPair) -> String {
    format!(
        "{} {} -->\n**You:** {}\n\n**PM:** {}\n",
        turn_anchor(pair.turn_id),
        anchor_stamp(&pair.at),
        pair.user.trim(),
        pair.assistant.trim(),
    )
}

/// The chat document's flat front-matter — read back by `ingest::parse_frontmatter` on a Rebuild (card G).
/// `source_type: chat` is the discriminator. Filing (card F): a **project** chat is born already-filed —
/// linked to its origin `project`, high importance, and `reviewed: true` so it skips the review queue —
/// which keeps the front-matter consistent with the DB row [`crate::chat_index::chat_doc_meta`] births,
/// and is Rebuild-safe. A **general** chat takes ingest defaults (`Unsorted` / no importance / unreviewed)
/// until card F files it through the shared `write_document_truth` path. Written by the file's sole writer
/// ([`append_turn_pair`]) at creation, so there is no cross-writer race on the authoritative vault file.
///
/// Note the `last_activity`/`ingested_at` scalars here are a creation-time snapshot and are NOT refreshed as
/// the chat grows — a chat's authoritative recency is per-turn (`chunks.chunk_at`, card B), and a Rebuild
/// re-derives `documents.last_activity` from the indexed turns (`chat_index::index_session`), not from these
/// lines. They exist for parity with a document's front-matter, not as a recency source.
fn render_chat_frontmatter(
    title: &str,
    conversation_id: i64,
    scope: &str,
    project: &str,
    created_at: &str,
    ingested_at: &str,
) -> String {
    // A project chat is filed to its origin project at birth; a general chat is unsorted until review.
    let (filed_project, importance, reviewed) = if scope == "project" {
        (project, "high", "true")
    } else {
        ("Unsorted", "null", "false")
    };
    format!(
        "---\n\
         title: {title}\n\
         content_hash: {hash}\n\
         source_type: chat\n\
         chat_conversation_id: {cid}\n\
         chat_scope: {scope}\n\
         chat_source_id: {sid}\n\
         project: {project}\n\
         linked_projects: []\n\
         tags: []\n\
         importance: {importance}\n\
         reviewed: {reviewed}\n\
         created_at: {created_at}\n\
         ingested_at: {ingested_at}\n\
         last_activity: {ingested_at}\n\
         ---\n\n",
        title = ingest::yaml_quote(title),
        hash = content_hash(conversation_id),
        cid = conversation_id,
        scope = scope,
        sid = source_id(conversation_id),
        project = ingest::yaml_quote(filed_project),
        importance = importance,
        reviewed = reviewed,
        created_at = created_at,
        ingested_at = ingested_at,
    )
}

/// Mirror a chat's re-evaluated classification (card F append re-eval) — and, when the title is
/// regenerated or the chat is renamed (B5-6), its `title:` — back into the vault front-matter, so the
/// file and the `documents` row stay in agreement. A Rebuild reads the file as truth (card G re-embeds
/// each chat `.md`), so a DB-only change would otherwise be silently reverted on the next Rebuild.
/// Patches ONLY the `title:`/`importance:`/`reviewed:` scalars within the leading front-matter fence
/// (and `title:` only when `title` is `Some`) — the chat-identity fields (`source_type`, `chat_*`),
/// the other org fields, and the turn body are left byte-for-byte intact. Best-effort by contract:
/// the `documents` row is already authoritative; this only keeps the file in step.
pub(crate) fn rewrite_chat_classification(
    cipher: &MarkdownCipher,
    vault_file: &Path,
    title: Option<&str>,
    importance: Option<&str>,
    reviewed: bool,
) -> Result<()> {
    let text = cipher.read(vault_file)?;
    let importance_val = importance.unwrap_or("null");
    let mut out = String::with_capacity(text.len());
    // 0 = before the front-matter, 1 = inside it, 2 = past it (body). Only patch scalars while inside.
    let mut fence = 0u8;
    for line in text.split_inclusive('\n') {
        let key = line.trim_end_matches(['\n', '\r']);
        if key == "---" {
            fence = if fence == 0 { 1 } else { 2 };
            out.push_str(line);
        } else if fence == 1 && title.is_some() && key.starts_with("title:") {
            // `title:` is YAML-quoted at birth (`render_chat_frontmatter`); quote it the same way here.
            out.push_str(&format!("title: {}\n", ingest::yaml_quote(title.unwrap())));
        } else if fence == 1 && key.starts_with("importance:") {
            out.push_str(&format!("importance: {importance_val}\n"));
        } else if fence == 1 && key.starts_with("reviewed:") {
            out.push_str(&format!("reviewed: {reviewed}\n"));
        } else {
            out.push_str(line);
        }
    }
    cipher.write_to(vault_file, &out)?;
    Ok(())
}

/// What one [`reconcile_vault_identity`] pass did. Returned, logged and persisted rather than kept
/// silent: this repairs a defect whose whole character was that it left no trace, so "it worked" has
/// to be something you can read, not something you infer from the absence of an error.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ChatIdentityHeal {
    /// Sessions with a vault file that were examined.
    pub scanned: usize,
    /// Files whose front-matter had lost `source_type: chat` and was restamped.
    pub restamped: usize,
    /// `documents` rows a prior Rebuild had already flipped to 'vault', put back to 'chat'.
    pub rows_restored: usize,
    /// Chats whose mis-derived chunks were dropped so the incremental indexer re-indexes them.
    pub reindex_queued: usize,
    /// Sessions whose `document_id` link was re-attached by `vault_path` after a wipe-arm Rebuild.
    pub relinked: usize,
    /// Sessions that could not be repaired (file missing/unreadable), with a short reason each.
    pub unrepaired: Vec<String>,
}

impl ChatIdentityHeal {
    /// Whether this pass changed anything — the gate for logging and for surfacing it to the user.
    pub fn touched_anything(&self) -> bool {
        self.restamped > 0 || self.rows_restored > 0 || self.relinked > 0
    }
}

/// Insert the chat-identity lines back into a stripped front-matter fence, in place.
///
/// Mirrors [`rewrite_chat_classification`]'s discipline — walk the leading fence and touch nothing
/// else — rather than re-rendering the file, because the body is a transcript and everything outside
/// these four lines is already correct. Returns `None` when the file already carries `source_type`
/// (nothing to do) or has no front-matter fence at all (not ours to repair).
fn restamp_chat_identity(text: &str, conversation_id: i64, scope: &str) -> Option<String> {
    let mut fence = 0u8;
    let mut has_source_type = false;
    for line in text.split_inclusive('\n') {
        let key = line.trim_end_matches(['\n', '\r']);
        if key == "---" {
            fence = if fence == 0 { 1 } else { 2 };
            if fence == 2 {
                break;
            }
        } else if fence == 1 && key.starts_with("source_type:") {
            has_source_type = true;
            break;
        }
    }
    if has_source_type || fence == 0 {
        return None;
    }
    // Re-insert immediately after the opening fence. Field order is irrelevant to the flat parser,
    // and the front position matches where `render_chat_frontmatter` writes them at birth.
    let mut out = String::with_capacity(text.len() + 96);
    let mut seen_open = false;
    for line in text.split_inclusive('\n') {
        out.push_str(line);
        if !seen_open && line.trim_end_matches(['\n', '\r']) == "---" {
            seen_open = true;
            out.push_str(&format!(
                "source_type: chat\nchat_conversation_id: {}\nchat_scope: {}\nchat_source_id: {}\n",
                conversation_id,
                scope,
                source_id(conversation_id),
            ));
        }
    }
    Some(out)
}

/// Repair chat vault files that an organisation write stripped of their identity, and any damage a
/// Rebuild has already done on top of that.
///
/// Until 3.81.2 every path that wrote a document's organisation metadata — approving a chat in
/// Review, editing its project, renaming or merging the project that owns it — rebuilt the vault file
/// through `ingest::rewrite_vault_metadata`, which had no chat arm and therefore dropped
/// `source_type: chat` + the `chat_*` lines. That is fixed at the source now; this repairs stores that
/// already took the hit.
///
/// Three states, all handled:
///   1. **Stripped, not yet rebuilt** — restamp the file from `chat_sessions`. Nothing else is wrong
///      yet, so this is a pure, lossless repair.
///   2. **Stripped and since rebuilt** — the file was re-ingested as an ordinary document, so the row
///      says `source_type = 'vault'` and its chunks were re-derived from the WHOLE transcript
///      (including PM's own answers, which the chat indexer deliberately never indexes) with NULL turn
///      pointers. Restore the row, drop those chunks and reset `last_indexed_turn_id` so the
///      incremental chat indexer re-indexes the session properly on its next sweep.
///      `clear_document_chunks` is safe: chunks are derived data, re-derivable from the transcript.
///   3. **Wipe-arm Rebuild in between** — a vector-width change DELETEs every `documents` row, and
///      `chat_sessions.document_id` is `ON DELETE SET NULL`, so the link dangles while a stray 'vault'
///      row now owns the `vault_path`. Re-attach by `vault_path` before doing (2), otherwise the
///      re-index would collide on that UNIQUE column.
///
/// Idempotent and cheap — one front-matter read per session with a vault file, and writes only where
/// something is actually wrong — so it is safe to call on every vault open AND as a precondition of
/// Rebuild. That pairing is deliberate: it means there is no window in which a user who updates and
/// immediately rebuilds can make the damage permanent.
///
/// Never touches `conversations` or `messages`: the authored turns are the truth and survive all three
/// states untouched. Best-effort per session — one unreadable file is recorded in `unrepaired` and the
/// pass continues.
pub fn reconcile_vault_identity(
    conn: &mut Connection,
    vault: &Path,
    cipher: &MarkdownCipher,
) -> Result<ChatIdentityHeal> {
    let mut report = ChatIdentityHeal::default();
    let sessions: Vec<(i64, String, String, Option<i64>)> = {
        let mut stmt = conn.prepare(
            "SELECT conversation_id, vault_path, scope, document_id FROM chat_sessions \
             WHERE vault_path IS NOT NULL AND vault_path <> ''",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    for (conversation_id, vault_path, scope, document_id) in sessions {
        report.scanned += 1;
        let file = vault.join(&vault_path);
        let text = match cipher.read(&file) {
            Ok(t) => t,
            Err(e) => {
                // A missing file is ordinary (the chat was deleted, or never had substance); anything
                // else is worth naming. Either way the pass continues.
                report
                    .unrepaired
                    .push(format!("conversation {conversation_id}: {e}"));
                continue;
            }
        };

        let restamped = match restamp_chat_identity(&text, conversation_id, &scope) {
            Some(fixed) => {
                cipher.write_to(&file, &fixed)?;
                report.restamped += 1;
                true
            }
            None => false,
        };

        // Re-attach a link a wipe-arm Rebuild severed, so the row work below has a row to act on.
        let doc_id = match document_id {
            Some(id) => Some(id),
            None => {
                let found: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM documents WHERE vault_path = ?1",
                        params![vault_path],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = found {
                    conn.execute(
                        "UPDATE chat_sessions SET document_id = ?2 WHERE conversation_id = ?1",
                        params![conversation_id, id],
                    )?;
                    report.relinked += 1;
                }
                found
            }
        };

        let Some(doc_id) = doc_id else { continue };
        let source_type: Option<String> = conn
            .query_row(
                "SELECT source_type FROM documents WHERE id = ?1",
                params![doc_id],
                |r| r.get(0),
            )
            .optional()?;
        // Only act when the row actually disagrees. A healthy chat (the overwhelming common case)
        // costs one front-matter read and one indexed lookup, and writes nothing at all.
        if source_type.as_deref() == Some(ingest::SOURCE_TYPE_CHAT) {
            continue;
        }
        if !restamped && source_type.is_none() {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE documents SET source_type = ?2, source_id = ?3 WHERE id = ?1",
            params![doc_id, ingest::SOURCE_TYPE_CHAT, source_id(conversation_id)],
        )?;
        // The chunks were derived from the wrong body by the wrong splitter; drop them and rewind the
        // cursor so `chat_index` re-indexes authored turns only, with turn pointers restored.
        ingest::clear_document_chunks(&tx, doc_id)?;
        tx.execute(
            "UPDATE chat_sessions SET last_indexed_turn_id = NULL WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        tx.commit()?;
        report.rows_restored += 1;
        report.reindex_queued += 1;
    }

    Ok(report)
}

/// `YYYY-MM-DD…` → `DD-MM-YYYY` (house date style). Defensive: any non-conforming head yields `unknown`
/// rather than panicking — PM's own timestamps always conform, so this only guards malformed input.
fn ddmmyyyy(iso: &str) -> String {
    let head = iso.get(0..10).unwrap_or("");
    if let [y, m, d] = head.split('-').collect::<Vec<_>>().as_slice() {
        if y.len() == 4 && m.len() == 2 && d.len() == 2 {
            return format!("{d}-{m}-{y}");
        }
    }
    "unknown".to_string()
}

/// A human stamp for a turn anchor: `DD-MM-YYYY HH:MM` (the time dropped if absent).
fn anchor_stamp(at: &str) -> String {
    let date = ddmmyyyy(at);
    match at.get(11..16) {
        Some(time) if !time.is_empty() => format!("{date} {time}"),
        _ => date,
    }
}

// --- triviality gate (board card 7B, PR2) ---

/// Whether a completed turn-pair carries indexable substance, or is throwaway chatter that would only
/// pollute retrieval and bloat the store ("thanks", "ok", a bare greeting).
pub(crate) enum Triviality {
    /// Pure acknowledgement / greeting on both sides — skipped from the index (the cursor still advances
    /// past it, so it is *handled*, not reconsidered forever). The vault keeps the turn, so card F can
    /// revisit it.
    Trivial,
    /// Carries content (a decision, a fact, a real question/answer) — indexed normally.
    Substantive,
}

/// The lean, deterministic importance gate card B uses to keep the chat firehose out of the index — NO
/// model call (running the document-inbox LLM scorer per turn-pair in a background job would be a cost
/// firehose, and the full importance/archive routing is card F's job). Conservative by construction: a
/// pair is `Trivial` only when **both** sides are short AND the user turn is a pure acknowledgement or
/// greeting — so a terse question with a substantive answer, or any longer exchange, is always kept. This
/// decides only whether a pair is *embedded*; the document-level archive outcome lives in card F.
pub(crate) fn triviality(pair: &TurnPair) -> Triviality {
    // A substantive answer (long, or multi-line) means the exchange carried content even if the user's
    // prompt was terse, so the length gate alone keeps "explain X?" → "<long answer>".
    const SHORT_CHARS: usize = 64;
    if pair.user.trim().chars().count() > SHORT_CHARS
        || pair.assistant.trim().chars().count() > SHORT_CHARS
    {
        return Triviality::Substantive;
    }
    if is_pure_chatter(&pair.user) {
        Triviality::Trivial
    } else {
        Triviality::Substantive
    }
}

/// Reduce a message to its bare alphanumerics of **any** script, lowercased (punctuation, emoji, and
/// whitespace dropped), then test it against a tight allow-list of acknowledgements/greetings. Kept
/// deliberately small: a false *Substantive* only means a trivial pair is indexed (mild), while a false
/// *Trivial* would drop real content (bad) — so when in doubt this returns false. Using `is_alphanumeric`
/// (not `is_ascii_alphanumeric`) is what keeps a short non-Latin message — a CJK request, a Cyrillic note
/// — from normalising to empty and being misread as chatter, then skipped by both the chat index and the
/// preference extractor with the cursor advanced past it forever (audit F-31).
fn is_pure_chatter(text: &str) -> bool {
    let normalized: String = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    // A message that was punctuation/emoji only (e.g. "👍") normalises to empty — also chatter.
    if normalized.is_empty() {
        return !text.trim().is_empty();
    }
    matches!(
        normalized.as_str(),
        "thanks"
            | "thank"
            | "thankyou"
            | "thanksalot"
            | "thankssomuch"
            | "ty"
            | "tysm"
            | "thx"
            | "ok"
            | "okay"
            | "okthanks"
            | "k"
            | "kk"
            | "gotit"
            | "got"
            | "cool"
            | "nice"
            | "great"
            | "perfect"
            | "awesome"
            | "amazing"
            | "soundsgood"
            | "sounds"
            | "sg"
            | "yes"
            | "yep"
            | "yeah"
            | "yup"
            | "no"
            | "nope"
            | "sure"
            | "alright"
            | "gotcha"
            | "makessense"
            | "willdo"
            | "np"
            | "yw"
            | "hi"
            | "hello"
            | "hey"
            | "heythere"
            | "hiya"
            | "yo"
            | "goodmorning"
            | "gm"
            | "goodnight"
            | "gn"
            | "bye"
            | "cheers"
            | "lol"
            | "haha"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn open_db(dir: &Path) -> Connection {
        crate::db::open(&dir.join("pm.sqlite"), DB_KEY).unwrap()
    }

    fn new_conversation(conn: &Connection) -> i64 {
        conn.execute("INSERT INTO conversations DEFAULT VALUES", [])
            .unwrap();
        conn.last_insert_rowid()
    }

    fn add_message(conn: &Connection, conv: i64, role: &str, content: &str) -> i64 {
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1, ?2, ?3)",
            params![conv, role, content],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn alternation_guard_rejects_stacked_user_turns() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path());
        let conv = new_conversation(&conn);

        // Empty conversation: a user turn is allowed.
        assert!(assert_user_turn_allowed(&conn, conv).is_ok());
        add_message(&conn, conv, "user", "hi");
        // Last message is a user turn: a second one is refused.
        assert!(assert_user_turn_allowed(&conn, conv).is_err());
        add_message(&conn, conv, "assistant", "hello");
        // Reply landed: a new user turn is allowed again.
        assert!(assert_user_turn_allowed(&conn, conv).is_ok());
    }

    #[test]
    fn discard_dangling_user_turn_unwedges_a_failed_send() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path());
        let conv = new_conversation(&conn);

        let count = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT count(*) FROM messages WHERE conversation_id = ?1",
                params![conv],
                |r| r.get(0),
            )
            .unwrap()
        };

        // Empty conversation: nothing to discard.
        assert!(!discard_dangling_user_turn(&conn, conv).unwrap());

        // A completed pair, then a user turn whose reply stream failed — the F-02 wedge: the next send
        // is now refused by the alternation guard.
        add_message(&conn, conv, "user", "a");
        let b = add_message(&conn, conv, "assistant", "b");
        add_message(&conn, conv, "user", "orphan"); // reply never landed
        assert!(assert_user_turn_allowed(&conn, conv).is_err(), "wedged");

        // Discarding the orphan removes exactly that one row and unwedges the conversation.
        assert!(discard_dangling_user_turn(&conn, conv).unwrap());
        assert_eq!(count(&conn), 2, "only the orphan is gone");
        assert!(assert_user_turn_allowed(&conn, conv).is_ok(), "unwedged");
        // The completed pair beneath it is untouched — a mid-history user row is never caught.
        let pairs = completed_turn_pairs_after(&conn, conv, None).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            (
                pairs[0].user.as_str(),
                pairs[0].assistant.as_str(),
                pairs[0].turn_id
            ),
            ("a", "b", b)
        );

        // Idempotent: with a reply now the trailing row, there is nothing to discard.
        assert!(!discard_dangling_user_turn(&conn, conv).unwrap());
    }

    #[test]
    fn completed_turn_pairs_pairs_and_drops_incomplete_tail() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(dir.path());
        let conv = new_conversation(&conn);
        add_message(&conn, conv, "user", "a");
        let b = add_message(&conn, conv, "assistant", "b");
        add_message(&conn, conv, "user", "c");
        let d = add_message(&conn, conv, "assistant", "d");
        add_message(&conn, conv, "user", "e"); // incomplete trailing user turn

        let all = completed_turn_pairs_after(&conn, conv, None).unwrap();
        assert_eq!(
            all.len(),
            2,
            "two complete pairs; the trailing user turn is excluded"
        );
        assert_eq!(
            (all[0].user.as_str(), all[0].assistant.as_str()),
            ("a", "b")
        );
        assert_eq!(all[0].turn_id, b, "turn id is the assistant message id");
        assert_eq!(all[1].turn_id, d);

        // Past the first pair's cursor, only the second pair remains; past the last, nothing.
        assert_eq!(
            completed_turn_pairs_after(&conn, conv, Some(b))
                .unwrap()
                .len(),
            1
        );
        assert!(completed_turn_pairs_after(&conn, conv, Some(d))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn chat_vault_filename_is_ddmmyyyy_and_stable() {
        let f = chat_vault_filename(7, "2026-06-28T10:00:00.000Z");
        assert!(
            f.starts_with("chat-28-06-2026-"),
            "DD-MM-YYYY date head: {f}"
        );
        assert!(f.ends_with(".md"));
        assert_eq!(
            f,
            chat_vault_filename(7, "2026-06-28T10:00:00.000Z"),
            "stable"
        );
        assert_ne!(
            f,
            chat_vault_filename(8, "2026-06-28T10:00:00.000Z"),
            "differs by session"
        );
    }

    fn append_round_trip(cipher: &MarkdownCipher) {
        let dir = tempfile::tempdir().unwrap();
        let name = cipher.on_disk_name(&chat_vault_filename(7, "2026-06-28T10:00:00.000Z"));
        let read = |n: &str| {
            let p = dir.path().join(n);
            cipher.decode(&std::fs::read(&p).unwrap(), &p).unwrap()
        };
        let p1 = TurnPair {
            user: "hi".into(),
            assistant: "hello".into(),
            turn_id: 2,
            at: "2026-06-28 10:00:01".into(),
        };
        let args = (
            &name,
            "My chat",
            7i64,
            "general",
            "2026-06-28T10:00:00.000Z",
            "2026-06-28T10:00:01.000Z",
        );
        append_turn_pair(
            dir.path(),
            cipher,
            args.0,
            args.1,
            args.2,
            args.3,
            "Unsorted",
            args.4,
            args.5,
            &p1,
        )
        .unwrap();
        // Re-firing the same turn is a no-op (idempotent).
        append_turn_pair(
            dir.path(),
            cipher,
            args.0,
            args.1,
            args.2,
            args.3,
            "Unsorted",
            args.4,
            args.5,
            &p1,
        )
        .unwrap();

        let content = read(&name);
        assert_eq!(
            content.matches("<!-- turn 2 ·").count(),
            1,
            "turn 2 written exactly once"
        );
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
        assert_eq!(
            fields.get("chat_source_id").map(String::as_str),
            Some("chat:7")
        );
        assert!(body.contains("**You:** hi") && body.contains("**PM:** hello"));

        // A new, different turn appends alongside the first.
        let p2 = TurnPair {
            user: "more".into(),
            assistant: "ok".into(),
            turn_id: 4,
            at: "2026-06-28 10:05:00".into(),
        };
        append_turn_pair(
            dir.path(),
            cipher,
            args.0,
            args.1,
            args.2,
            args.3,
            "Unsorted",
            args.4,
            args.5,
            &p2,
        )
        .unwrap();
        let content = read(&name);
        assert!(content.contains("<!-- turn 2 ·") && content.contains("<!-- turn 4 ·"));
    }

    #[test]
    fn append_round_trips_plaintext() {
        append_round_trip(&MarkdownCipher::plaintext("v"));
    }

    #[test]
    fn append_round_trips_encrypted() {
        let cipher = MarkdownCipher::for_test_encrypted("v");
        let name = cipher.on_disk_name(&chat_vault_filename(7, "2026-06-28T10:00:00.000Z"));
        assert!(
            name.ends_with(".md.pmenc"),
            "encrypted on-disk name carries .pmenc"
        );
        append_round_trip(&cipher);
    }

    #[test]
    fn rewrite_chat_classification_patches_only_org_scalars() {
        // Card 7F / M3: an append re-eval mirrors its new importance/reviewed into the vault front-matter so
        // a Rebuild (file-as-truth) preserves it. It must patch ONLY those scalars — never the chat identity
        // fields or the turn body.
        let cipher = MarkdownCipher::plaintext("v");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat-28-06-2026-abc.md");
        let original = "---\n\
             title: Hi\n\
             content_hash: chat:7\n\
             source_type: chat\n\
             chat_conversation_id: 7\n\
             chat_scope: general\n\
             chat_source_id: chat:7\n\
             project: Unsorted\n\
             tags: []\n\
             importance: null\n\
             reviewed: false\n\
             created_at: 2026-06-28T10:00:00.000Z\n\
             ingested_at: 2026-06-28T10:00:00.000Z\n\
             last_activity: 2026-06-28T10:00:00.000Z\n\
             ---\n\n\
             <!-- turn 2 · 2026-06-28 10:00 -->\n**You:** hi\n\n**PM:** hello\n";
        cipher.write_to(&file, original).unwrap();

        // Re-file it (e.g. an already-filed general chat that got archived, now re-opened): importance set,
        // reviewed flipped.
        rewrite_chat_classification(&cipher, &file, None, Some("archive"), true).unwrap();
        let content = cipher.read(&file).unwrap();
        let (fields, body) =
            ingest::parse_frontmatter(&content).expect("front-matter still parses");
        assert_eq!(
            fields.get("importance").map(String::as_str),
            Some("archive")
        );
        assert_eq!(fields.get("reviewed").map(String::as_str), Some("true"));
        // `title: None` leaves the title scalar exactly as it was.
        assert_eq!(fields.get("title").map(String::as_str), Some("Hi"));
        // Identity + body untouched.
        assert_eq!(fields.get("source_type").map(String::as_str), Some("chat"));
        assert_eq!(
            fields.get("chat_conversation_id").map(String::as_str),
            Some("7")
        );
        assert_eq!(
            fields.get("chat_source_id").map(String::as_str),
            Some("chat:7")
        );
        assert_eq!(fields.get("project").map(String::as_str), Some("Unsorted"));
        assert!(body.contains("**You:** hi") && body.contains("**PM:** hello"));

        // Clearing importance renders `null`, and the turn body is still intact.
        rewrite_chat_classification(&cipher, &file, None, None, false).unwrap();
        let content = cipher.read(&file).unwrap();
        let (fields, _) = ingest::parse_frontmatter(&content).expect("parses");
        assert_eq!(fields.get("importance").map(String::as_str), Some("null"));
        assert_eq!(fields.get("reviewed").map(String::as_str), Some("false"));
    }

    #[test]
    fn rewrite_chat_classification_patches_the_title_scalar() {
        // B5-6: a generated/renamed title is mirrored into the vault front-matter so a Rebuild (which
        // reads the file as truth) keeps it — and a colon-bearing title must be YAML-quoted, or the
        // front-matter would no longer parse.
        let cipher = MarkdownCipher::plaintext("v");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat-28-06-2026-abc.md");
        let original = "---\n\
             title: \"first message placeholder…\"\n\
             content_hash: chat:7\n\
             source_type: chat\n\
             chat_conversation_id: 7\n\
             chat_scope: general\n\
             chat_source_id: chat:7\n\
             project: Unsorted\n\
             tags: []\n\
             importance: null\n\
             reviewed: false\n\
             created_at: 2026-06-28T10:00:00.000Z\n\
             ingested_at: 2026-06-28T10:00:00.000Z\n\
             last_activity: 2026-06-28T10:00:00.000Z\n\
             ---\n\n\
             <!-- turn 2 · 2026-06-28 10:00 -->\n**You:** hi\n\n**PM:** hello\n";
        cipher.write_to(&file, original).unwrap();

        // A colon in the title exercises the yaml-quote path.
        rewrite_chat_classification(&cipher, &file, Some("Q3: budget review"), None, false)
            .unwrap();
        let content = cipher.read(&file).unwrap();
        let (fields, body) =
            ingest::parse_frontmatter(&content).expect("front-matter still parses");
        assert_eq!(
            fields.get("title").map(String::as_str),
            Some("Q3: budget review"),
            "the new title lands and survives a re-parse (was YAML-quoted)"
        );
        // Everything else is byte-preserved: org scalars, identity, and the turn body.
        assert_eq!(fields.get("importance").map(String::as_str), Some("null"));
        assert_eq!(fields.get("reviewed").map(String::as_str), Some("false"));
        assert_eq!(fields.get("source_type").map(String::as_str), Some("chat"));
        assert_eq!(fields.get("project").map(String::as_str), Some("Unsorted"));
        assert!(body.contains("**You:** hi") && body.contains("**PM:** hello"));
    }

    /// Card F routing: a general chat is born unsorted/unreviewed (heads to the review queue), a project
    /// chat is born filed to its origin project with high importance and `reviewed: true` (skips the
    /// queue) — the front-matter matching the `documents` row `chat_index::chat_doc_meta` births.
    #[test]
    fn front_matter_routes_general_to_queue_and_files_project_chats() {
        let cipher = MarkdownCipher::plaintext("v");
        let dir = tempfile::tempdir().unwrap();
        let read = |n: &str| {
            let p = dir.path().join(n);
            cipher.decode(&std::fs::read(&p).unwrap(), &p).unwrap()
        };
        let fields_of = |scope: &str, project: &str, cid: i64| {
            let name = cipher.on_disk_name(&chat_vault_filename(cid, "2026-06-28T10:00:00.000Z"));
            append_turn_pair(
                dir.path(),
                &cipher,
                &name,
                "My chat",
                cid,
                scope,
                project,
                "2026-06-28T10:00:00.000Z",
                "2026-06-28T10:00:01.000Z",
                &pair(
                    "what's the plan for Q3?",
                    "Here is a detailed plan for the quarter ahead.",
                ),
            )
            .unwrap();
            ingest::parse_frontmatter(&read(&name))
                .expect("front-matter parses")
                .0
        };

        let general = fields_of("general", "Unsorted", 7);
        assert_eq!(general.get("project").map(String::as_str), Some("Unsorted"));
        assert_eq!(general.get("importance").map(String::as_str), Some("null"));
        assert_eq!(general.get("reviewed").map(String::as_str), Some("false"));

        let project = fields_of("project", "Atlas - PM", 8);
        assert_eq!(
            project.get("project").map(String::as_str),
            Some("Atlas - PM")
        );
        assert_eq!(project.get("importance").map(String::as_str), Some("high"));
        assert_eq!(project.get("reviewed").map(String::as_str), Some("true"));
    }

    fn pair(user: &str, assistant: &str) -> TurnPair {
        TurnPair {
            user: user.into(),
            assistant: assistant.into(),
            turn_id: 2,
            at: "2026-06-28T10:00:00.000Z".into(),
        }
    }

    #[test]
    fn triviality_skips_chatter_but_keeps_substance() {
        let trivial = |u: &str, a: &str| matches!(triviality(&pair(u, a)), Triviality::Trivial);

        // Pure acknowledgement / greeting on both sides → trivial (skipped from the index).
        assert!(trivial("thanks!", "You're welcome."));
        assert!(trivial("ok", "👍"));
        assert!(trivial("got it", "Great."));
        assert!(trivial("👍", "👍"));
        assert!(trivial("Hey there", "Hello!"));

        // F-31: a short non-Latin exchange carrying a real fact must be KEPT. Before the any-script
        // fix these normalised to empty and were misread as chatter — dropped from the index and the
        // preference scan, cursor advanced past them forever. Both sides short so the length gate
        // doesn't mask the classification (a CJK scheduling note, a Cyrillic decision).
        assert!(!trivial("下周三下午三点开会", "好的，已记录。"));
        assert!(!trivial("встреча в среду в три", "хорошо"));
        // A pure-emoji exchange is still trivial — the empty-normalisation branch stays correct.
        assert!(trivial("🎉🎉", "👍"));

        // Anything stating a decision/fact/preference, or a real Q&A, is kept — even when terse.
        assert!(!trivial(
            "Let's call the org Atlas.",
            "Noted — Atlas it is."
        ));
        assert!(!trivial(
            "thanks",
            "Before you go: remember the launch is on 1 July and the demo needs the new build."
        ));
        assert!(!trivial(
            "What's our deadline?",
            "The pitch milestone is due 15 August."
        ));
        // A bare acknowledgement that is actually long enough to carry content is kept (length gate).
        assert!(!trivial(
            "ok but actually I changed my mind, let's ship on Friday instead of Monday",
            "Sure."
        ));
    }

    // --- chat vault identity: the strip repair (3.81.2) ---------------------------------------

    /// Seed a session whose vault file exists, plus the `documents` row card B would have created.
    fn seed_chat_session(
        conn: &Connection,
        dir: &Path,
        cipher: &MarkdownCipher,
        conv: i64,
        scope: &str,
        source_type: &str,
    ) -> (i64, String) {
        let vault_path = format!("chat-01-01-2026-{conv}.md");
        let file = dir.join(&vault_path);
        cipher
            .write_to(
                &file,
                &render_chat_frontmatter(
                    "A chat",
                    conv,
                    scope,
                    "Atlas",
                    "2026-01-01T00:00:00Z",
                    "2026-01-01T00:00:00Z",
                ),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id) \
             VALUES (?1, 'A chat', ?2, ?3, ?4)",
            params![vault_path, content_hash(conv), source_type, source_id(conv)],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, document_id, scope, vault_path, last_indexed_turn_id) \
             VALUES (?1, ?2, ?3, ?4, 99)",
            params![conv, doc_id, scope, vault_path],
        )
        .unwrap();
        (doc_id, vault_path)
    }

    /// Reproduce the bug: drop every identity line the way the generic front-matter rewriter used to.
    fn strip_identity(cipher: &MarkdownCipher, file: &Path) {
        let text = cipher.read(file).unwrap();
        let stripped: String = text
            .lines()
            .filter(|l| !l.starts_with("source_type:") && !l.starts_with("chat_"))
            .map(|l| format!("{l}\n"))
            .collect();
        cipher.write_to(file, &stripped).unwrap();
    }

    #[test]
    fn restamp_puts_back_exactly_the_identity_lines_a_strip_removed() {
        let born = render_chat_frontmatter("T", 7, "project", "Atlas", "2026-01-01", "2026-01-01");
        let stripped: String = born
            .lines()
            .filter(|l| !l.starts_with("source_type:") && !l.starts_with("chat_"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(!stripped.contains("source_type: chat"));

        let fixed = restamp_chat_identity(&stripped, 7, "project").expect("a strip is repairable");
        assert!(fixed.contains("source_type: chat"));
        assert!(fixed.contains("chat_conversation_id: 7"));
        assert!(fixed.contains("chat_scope: project"));
        assert!(fixed.contains(&format!("chat_source_id: {}", source_id(7))));
        // Everything else survives untouched — this repairs identity, it does not re-render the file.
        assert!(fixed.contains(r#"title: "T""#));
        assert!(fixed.contains(&format!("content_hash: {}", content_hash(7))));
    }

    #[test]
    fn restamp_is_a_no_op_on_a_healthy_file_and_on_a_non_frontmatter_file() {
        let born =
            render_chat_frontmatter("T", 7, "general", "Unsorted", "2026-01-01", "2026-01-01");
        assert!(restamp_chat_identity(&born, 7, "general").is_none());
        // No fence at all: not ours to repair, and must never be mangled into one.
        assert!(restamp_chat_identity("just a body, no front-matter\n", 7, "general").is_none());
    }

    #[test]
    fn heal_restamps_a_stripped_file_and_leaves_a_healthy_store_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let cipher = MarkdownCipher::plaintext("vault-test");
        let conv = new_conversation(&conn);
        let (_doc, vault_path) =
            seed_chat_session(&conn, dir.path(), &cipher, conv, "project", "chat");

        // Healthy store: the pass changes nothing.
        let clean = reconcile_vault_identity(&mut conn, dir.path(), &cipher).unwrap();
        assert_eq!(clean.scanned, 1);
        assert_eq!(clean.restamped, 0);
        assert!(!clean.touched_anything());

        // State 1 — stripped, not yet rebuilt.
        strip_identity(&cipher, &dir.path().join(&vault_path));
        let healed = reconcile_vault_identity(&mut conn, dir.path(), &cipher).unwrap();
        assert_eq!(healed.restamped, 1);
        assert!(healed.touched_anything());
        let text = cipher.read(&dir.path().join(&vault_path)).unwrap();
        assert!(text.contains("source_type: chat"));
        assert!(text.contains(&format!("chat_conversation_id: {conv}")));

        // Idempotent: a second pass finds nothing left to do.
        let again = reconcile_vault_identity(&mut conn, dir.path(), &cipher).unwrap();
        assert_eq!(again.restamped, 0);
        assert!(!again.touched_anything());
    }

    #[test]
    fn heal_restores_a_row_a_rebuild_already_demoted_and_rewinds_the_index_cursor() {
        // State 2 — the strip happened, then a Rebuild re-ingested the chat as an ordinary document:
        // the row says 'vault', and its chunks were derived from the whole transcript (PM's own
        // answers included) with NULL turn pointers.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let cipher = MarkdownCipher::plaintext("vault-test");
        let conv = new_conversation(&conn);
        let (doc_id, vault_path) =
            seed_chat_session(&conn, dir.path(), &cipher, conv, "general", "vault");
        strip_identity(&cipher, &dir.path().join(&vault_path));
        conn.execute(
            "INSERT INTO chunks(document_id, ordinal, content, char_count) \n             VALUES (?1, 0, 'PM: a hallucinated answer', 26)",
            params![doc_id],
        )
        .unwrap();

        let healed = reconcile_vault_identity(&mut conn, dir.path(), &cipher).unwrap();
        assert_eq!(healed.restamped, 1);
        assert_eq!(healed.rows_restored, 1);
        assert_eq!(healed.reindex_queued, 1);

        let (st, sid): (String, Option<String>) = conn
            .query_row(
                "SELECT source_type, source_id FROM documents WHERE id = ?1",
                params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(st, ingest::SOURCE_TYPE_CHAT);
        assert_eq!(sid.as_deref(), Some(source_id(conv).as_str()));

        // The mis-derived chunks are gone and the cursor is rewound, so the chat indexer re-indexes
        // authored turns only, with turn pointers restored.
        let chunks: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE document_id = ?1",
                params![doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chunks, 0);
        let cursor: Option<i64> = conn
            .query_row(
                "SELECT last_indexed_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cursor, None);

        // The authored truth is never touched — that is what makes every state recoverable.
        let convs: i64 = conn
            .query_row("SELECT count(*) FROM conversations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(convs, 1);
    }

    #[test]
    fn heal_relinks_a_session_a_wipe_arm_rebuild_unlinked() {
        // State 3 — a vector-width Rebuild DELETEd every `documents` row, so `document_id` was set
        // NULL by the FK and a stray row now owns the vault_path. Re-attach before repairing, or the
        // re-index collides on that UNIQUE column.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let cipher = MarkdownCipher::plaintext("vault-test");
        let conv = new_conversation(&conn);
        let (doc_id, vault_path) =
            seed_chat_session(&conn, dir.path(), &cipher, conv, "general", "vault");
        strip_identity(&cipher, &dir.path().join(&vault_path));
        conn.execute(
            "UPDATE chat_sessions SET document_id = NULL WHERE conversation_id = ?1",
            params![conv],
        )
        .unwrap();

        let healed = reconcile_vault_identity(&mut conn, dir.path(), &cipher).unwrap();
        assert_eq!(healed.relinked, 1);
        assert_eq!(healed.rows_restored, 1);
        let linked: Option<i64> = conn
            .query_row(
                "SELECT document_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(linked, Some(doc_id));
    }

    #[test]
    fn heal_records_an_unreadable_session_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let cipher = MarkdownCipher::plaintext("vault-test");
        let gone = new_conversation(&conn);
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope, vault_path) \
             VALUES (?1, 'general', 'missing.md')",
            params![gone],
        )
        .unwrap();
        let conv = new_conversation(&conn);
        let (_d, vault_path) =
            seed_chat_session(&conn, dir.path(), &cipher, conv, "project", "chat");
        strip_identity(&cipher, &dir.path().join(&vault_path));

        let healed = reconcile_vault_identity(&mut conn, dir.path(), &cipher).unwrap();
        assert_eq!(healed.scanned, 2);
        assert_eq!(
            healed.unrepaired.len(),
            1,
            "the missing file is reported, not swallowed"
        );
        assert_eq!(
            healed.restamped, 1,
            "the readable session is still repaired"
        );
    }
}
