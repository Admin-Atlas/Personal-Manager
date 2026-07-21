// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat-stated preference extraction (board card 7F, #145) — the background engine that notices when a
//! user EXPLICITLY states a preference inside a chat ("I always want dates as DD-MM-YYYY") and writes it
//! as a typed [`crate::preferences`] record, `source = 'chat'`, unconfirmed, surfaced in Teach as a
//! "Suggested" preference the user keeps, edits, or dismisses.
//!
//! The shape, and the locked card decisions it honours:
//!
//!   * **Explicit only; gated through review.** We extract ONLY what the user stated in so many words —
//!     never a preference INFERRED from behaviour (that is Stage 5). Each record lands unconfirmed at a
//!     seed confidence, so it is a suggestion in Teach, never a silently-applied rule. The model reply is
//!     untrusted DATA: extracted + validated defensively, then deduped so a re-stated preference is
//!     captured once.
//!   * **Authored user content only.** We feed the model the USER side of the new turns only — never the
//!     assistant's replies or the RAG context assembled into the prompt — the same authored-only
//!     discipline `chat_index::render_authored_segment` enforces for embedding.
//!   * **Cursor-driven, like indexing/summarising.** `chat_sessions.prefs_covers_up_to_turn_id` records
//!     how far extraction has read; each pass looks only at turns past it and advances it only inside the
//!     write that lands the records — so a crash or network failure simply re-reads the same turns next
//!     time, never double-inserts, and never silently skips a batch it failed to process.
//!   * **Background model, best-effort.** Extraction uses the BACKGROUND model role (not the model the
//!     user is talking to), the spend logged under `chat_prefs`. A failure is logged and the chat is
//!     otherwise untouched — extraction is never on the critical path.
//!
//! Triggers mirror `chat_title`: an eager post-reply nudge for the just-active conversation
//! ([`spawn_extract_after_reply`]) plus a launch catch-up ([`spawn_prefs_scheduler`]) for turns added
//! while the app was closed. Both run fully async (the model call is async; DB locks are short and never
//! held across an await, AGENTS rule #4) and share a single-flight guard.

use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::preferences::{self, DraftPreference};
use crate::{chat, entities, secrets, AppState};

/// Seed confidence for a chat-extracted record: an explicit statement, but auto-captured and not yet
/// vouched for by the user — so it lands below 1.0 and unconfirmed, awaiting a "Keep" in Teach.
const SEED_CONFIDENCE: f64 = 0.6;
/// Skip the model call when the new user text is shorter than this — a couple of words can't carry a
/// stated preference, and it saves a call on trivial exchanges the triviality filter didn't already cut.
const MIN_CONTENT_CHARS: usize = 15;
/// Cap how many of the most-recent new turn-pairs one extraction pass feeds the model, bounding the
/// prompt on a large launch catch-up. The eager per-reply nudge keeps live chats current, so this only
/// ever bites a big backlog; when it does, we log what was skipped rather than silently dropping it.
const MAX_EXTRACT_PAIRS: usize = 40;

/// Launch catch-up: wait up to `LAUNCH_WAIT_TICKS × LAUNCH_WAIT_SECS` (~5 min) for the vault to unlock
/// before the catch-up pass (mirrors the title/summary schedulers).
const LAUNCH_WAIT_TICKS: u32 = 60;
const LAUNCH_WAIT_SECS: u64 = 5;

/// The user side of every SUBSTANTIVE new turn-pair (trivial acknowledgements/greetings dropped — they
/// can't carry a stated preference), trimmed and non-empty. The authored user text the extractor scans;
/// pure, so the gate is unit-tested without a model.
fn extractable_user_turns(pairs: &[chat::TurnPair]) -> Vec<String> {
    pairs
        .iter()
        .filter(|p| !matches!(chat::triviality(p), chat::Triviality::Trivial))
        .map(|p| p.user.trim().to_string())
        .filter(|u| !u.is_empty())
        .collect()
}

/// Resolve a draft's project NAME to a canonical entity, WITHOUT creating one — an untrusted chat
/// mention must never mint a new project (that would pollute the canonical list). A project-scoped draft
/// whose name resolves to nothing is downgraded to `global` (the value is still worth suggesting; the
/// user can re-scope it in Teach) rather than inventing an entity or dropping the preference. Returns the
/// `(scope, entity_id)` to store.
fn resolve_draft_target(
    conn: &Connection,
    draft: &DraftPreference,
) -> Result<(String, Option<i64>)> {
    if draft.scope != preferences::SCOPE_PROJECT {
        return Ok((draft.scope.clone(), None));
    }
    let resolved = match draft.project_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => entities::resolve_project(conn, name, false)?,
        _ => None,
    };
    match resolved {
        Some(id) => Ok((preferences::SCOPE_PROJECT.to_string(), Some(id))),
        None => Ok((preferences::SCOPE_GLOBAL.to_string(), None)),
    }
}

/// Persist the extracted drafts: resolve each to its target, skip any that already exist (dedup), and
/// insert the rest as unconfirmed `source='chat'` suggestions — then advance the extraction cursor to
/// `cursor_to`, all in one transaction so the cursor and the records commit together. Returns how many
/// records were newly inserted. Pure DB logic — unit-tested without the model.
fn persist_extraction(
    conn: &mut Connection,
    conversation_id: i64,
    drafts: &[DraftPreference],
    cursor_to: i64,
) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    for draft in drafts {
        let (scope, entity_id) = resolve_draft_target(&tx, draft)?;
        if preferences::pref_exists(
            &tx,
            &scope,
            entity_id,
            draft.condition.as_deref(),
            &draft.value,
        )? {
            continue; // already stored (user re-stated it, or a prior sweep captured it)
        }
        preferences::add_preference(
            &tx,
            &scope,
            entity_id,
            draft.condition.as_deref(),
            &draft.value,
            preferences::SOURCE_CHAT,
            SEED_CONFIDENCE,
            false,
        )?;
        inserted += 1;
    }
    tx.execute(
        "UPDATE chat_sessions SET prefs_covers_up_to_turn_id = ?1 WHERE conversation_id = ?2",
        params![cursor_to, conversation_id],
    )?;
    tx.commit()?;
    Ok(inserted)
}

/// Extract stated preferences from ONE conversation's new turns, if any: short read lock (the cursor +
/// the turns past it; bail cheaply if there is nothing substantive to scan), the background model call
/// OFF the lock, then a short guarded write that lands the records and advances the cursor together.
/// Returns how many records were inserted. Mirrors `chat_title::generate_title`'s lock discipline.
pub(crate) async fn extract_for_session(app: &AppHandle, conversation_id: i64) -> Result<usize> {
    // 1. Short read lock: how far have we read, and what are the new turns?
    let (cursor_to, user_turns) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let cursor: Option<Option<i64>> = conn
            .query_row(
                "SELECT prefs_covers_up_to_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?;
        // No session row (card A hasn't appended a turn-pair yet) ⇒ nothing to extract.
        let Some(cursor) = cursor else {
            return Ok(0);
        };
        let mut pairs = chat::completed_turn_pairs_after(&conn, conversation_id, cursor)?;
        // Bound the work per run on a large backlog, OLDEST-first (pairs are ordered by id): keep the
        // oldest MAX_EXTRACT_PAIRS so the cursor advances only over turns we actually scan — the newer
        // overflow stays unscanned and is picked up next run, instead of being skipped past forever.
        if pairs.len() > MAX_EXTRACT_PAIRS {
            let deferred = pairs.len() - MAX_EXTRACT_PAIRS;
            eprintln!(
                "chat-prefs: session {conversation_id} has a backlog — scanning the {MAX_EXTRACT_PAIRS} \
                 oldest unscanned turns for stated preferences, deferring {deferred} newer ones to the next run"
            );
            pairs.truncate(MAX_EXTRACT_PAIRS);
        }
        let Some(cursor_to) = pairs.iter().map(|p| p.turn_id).max() else {
            return Ok(0); // caught up
        };
        let user_turns = extractable_user_turns(&pairs);
        (cursor_to, user_turns)
    };

    // Nothing substantive to scan ⇒ advance the cursor past it (never re-examined) and stop — no call.
    let content_chars: usize = user_turns.iter().map(|t| t.chars().count()).sum();
    if user_turns.is_empty() || content_chars < MIN_CONTENT_CHARS {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        conn.execute(
            "UPDATE chat_sessions SET prefs_covers_up_to_turn_id = ?1 WHERE conversation_id = ?2",
            params![cursor_to, conversation_id],
        )?;
        return Ok(0);
    }

    // 2. Resolve the background key + models + known projects off the lock; no key ⇒ we cannot extract
    //    (the cursor stays put, so a later launch retries once a key exists).
    let Some(route) = crate::llm_gateway::resolve(app, crate::llm_gateway::Role::Background)?
    else {
        return Ok(0);
    };
    let project_names = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        entities::canonical_project_names(&conn)?
    };

    // 3. Extract (async, no lock held). A model/parse failure propagates so the cursor is NOT advanced
    //    and the turns are retried next time.
    let messages = preferences::render_chat_extract_request(&user_turns, &project_names);
    let completion = crate::llm_gateway::complete(app, &route, &messages, false).await?;
    let drafts = preferences::parse_chat_preferences(&completion.text);

    // 4. Short write lock: land the records (resolve + dedup) and advance the cursor together; log spend.
    let inserted = {
        let state = app.state::<AppState>();
        let mut conn = state.conn()?;
        let inserted = persist_extraction(&mut conn, conversation_id, &drafts, cursor_to)?;
        let model = completion
            .model
            .as_deref()
            .or(Some(route.primary_model_id()));
        let _ = conn.execute(
            "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd) \
             VALUES (?1, 'chat_prefs', ?2, ?3, ?4)",
            params![
                model,
                completion.usage.prompt_tokens,
                completion.usage.completion_tokens,
                completion.usage.cost
            ],
        );
        inserted
    };
    Ok(inserted)
}

/// Extract from every session that has completed turns past its extraction cursor — the launch catch-up
/// for turns added while the app was closed. Best-effort per session: one failure is logged and the
/// reconcile moves on.
async fn reconcile_prefs(app: &AppHandle) {
    let candidates: Vec<i64> = {
        let state = app.state::<AppState>();
        let Ok(conn) = state.conn() else { return };
        let mut stmt = match conn.prepare(
            "SELECT s.conversation_id FROM chat_sessions s \
             WHERE EXISTS (SELECT 1 FROM messages m \
                           WHERE m.conversation_id = s.conversation_id AND m.role = 'assistant' \
                             AND m.id > COALESCE(s.prefs_covers_up_to_turn_id, 0))",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0));
        match rows {
            Ok(rows) => rows.filter_map(std::result::Result::ok).collect(),
            Err(_) => return,
        }
    };
    let mut found = 0usize;
    for conv in candidates {
        match extract_for_session(app, conv).await {
            Ok(n) => found += n,
            Err(e) => eprintln!("chat-prefs: session {conv} failed: {e}"),
        }
    }
    if found > 0 {
        eprintln!("chat-prefs: suggested {found} preference(s) from chat");
    }
}

/// Run a guarded extraction pass under the single-flight flag the eager nudge and the launch catch-up
/// share, so the two never overlap.
async fn run_guarded<F, Fut>(app: &AppHandle, op: F)
where
    F: FnOnce(AppHandle) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let state = app.state::<AppState>();
    let Some(_guard) = crate::BusyGuard::acquire(&state.prefs_busy) else {
        return; // another pass holds the single-flight
    };
    // `_guard` resets the flag on drop — including if `op` panics — so extraction can't wedge.
    op(app.clone()).await;
}

/// Whether the vault is unlocked and an OpenRouter key is set — the minimum to extract (a pure model
/// call, no sidecar).
///
/// The key is the BACKGROUND key, falling back to the primary — the same resolution the call itself
/// uses, so the gate can never say "no" to a setup the work would have run on. It used to demand the
/// PRIMARY key specifically, which meant a background-key-only setup was refused outright: the whole
/// point of the two-key split is that background spend is separable, and this is background work.
fn ready(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    state.conn().is_ok()
        && secrets::get_background_or_primary_key()
            .ok()
            .flatten()
            .is_some()
}

/// Eagerly extract from the just-active conversation after its reply lands (the primary trigger).
/// Fire-and-forget and single-flight, so it never delays the streamed reply and never overlaps the
/// launch catch-up.
pub fn spawn_extract_after_reply(app: AppHandle, conversation_id: i64) {
    tauri::async_runtime::spawn(async move {
        run_guarded(&app, |app| async move {
            if let Err(e) = extract_for_session(&app, conversation_id).await {
                eprintln!("chat-prefs: eager extraction for {conversation_id} skipped ({e})");
            }
        })
        .await;
    });
}

/// The launch catch-up: once the vault is ready, extract from any session with turns added while the app
/// was closed, then stop (the eager nudge handles live sessions).
pub fn spawn_prefs_scheduler(app: AppHandle, launch_stagger: Duration) {
    tauri::async_runtime::spawn(async move {
        let mut ready_now = false;
        for _ in 0..LAUNCH_WAIT_TICKS {
            if ready(&app) {
                ready_now = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(LAUNCH_WAIT_SECS)).await;
        }
        if ready_now {
            // F-54: staggered so the launch passes don't all fire at once on unlock (see lib.rs).
            tokio::time::sleep(launch_stagger).await;
            run_guarded(&app, |app| async move { reconcile_prefs(&app).await }).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn open_db(dir: &std::path::Path) -> Connection {
        crate::db::open(&dir.join("pm.sqlite"), DB_KEY).unwrap()
    }

    fn pair(user: &str, assistant: &str, turn_id: i64) -> chat::TurnPair {
        chat::TurnPair {
            user: user.into(),
            assistant: assistant.into(),
            turn_id,
            at: "2026-07-01T10:00:00.000Z".into(),
        }
    }

    #[test]
    fn extractable_turns_keep_substance_and_drop_chatter() {
        let pairs = vec![
            pair("thanks!", "You're welcome.", 2),
            pair("  I always want dates as DD-MM-YYYY  ", "Noted.", 4),
            pair("ok", "👍", 6),
        ];
        let turns = extractable_user_turns(&pairs);
        assert_eq!(
            turns,
            vec!["I always want dates as DD-MM-YYYY".to_string()],
            "only the substantive user turn survives, trimmed"
        );
    }

    /// A session row with a preset extraction cursor. Returns the conversation id.
    fn session(conn: &Connection, prefs_cursor: Option<i64>) -> i64 {
        conn.execute("INSERT INTO conversations(title) VALUES ('c')", [])
            .unwrap();
        let conv = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope, prefs_covers_up_to_turn_id) \
             VALUES (?1, 'general', ?2)",
            params![conv, prefs_cursor],
        )
        .unwrap();
        conv
    }

    fn draft(scope: &str, project: Option<&str>, value: &str) -> DraftPreference {
        DraftPreference {
            scope: scope.into(),
            entity_id: None,
            project_name: project.map(String::from),
            condition: None,
            value: value.into(),
        }
    }

    #[test]
    fn persist_inserts_dedups_and_advances_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = session(&conn, None);

        let drafts = vec![
            draft("global", None, "Use DD-MM-YYYY dates"),
            draft("global", None, "  use dd-mm-yyyy dates  "), // dup of the first (case/space-insensitive)
        ];
        let inserted = persist_extraction(&mut conn, conv, &drafts, 4).unwrap();
        assert_eq!(inserted, 1, "the duplicate is skipped");

        let (value, source, confirmed): (String, String, bool) = conn
            .query_row(
                "SELECT value, source, user_confirmed FROM preferences",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
            .unwrap();
        assert_eq!(value, "Use DD-MM-YYYY dates");
        assert_eq!(source, "chat", "stored with the chat origin");
        assert!(!confirmed, "surfaces as an unconfirmed suggestion");

        let cursor: Option<i64> = conn
            .query_row(
                "SELECT prefs_covers_up_to_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cursor, Some(4), "cursor advanced to the newest turn");

        // Re-running the SAME drafts inserts nothing (both now dedup against the stored record).
        let again = persist_extraction(&mut conn, conv, &drafts, 4).unwrap();
        assert_eq!(again, 0, "idempotent across sweeps");
    }

    #[test]
    fn unresolved_project_draft_is_downgraded_to_global_not_invented() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(dir.path());
        let conv = session(&conn, None);

        // "Nonesuch" resolves to no existing entity → downgrade to global, don't mint a project.
        persist_extraction(
            &mut conn,
            conv,
            &[draft("project", Some("Nonesuch"), "Keep replies terse")],
            8,
        )
        .unwrap();
        let (scope, entity): (String, Option<i64>) = conn
            .query_row("SELECT scope, entity_id FROM preferences", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(scope, "global");
        assert_eq!(entity, None);
    }
}
