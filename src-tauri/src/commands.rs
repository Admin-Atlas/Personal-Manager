// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command surface exposed to the frontend. DB access locks the shared
//! connection only for quick synchronous work — never across an `.await` — so
//! the streaming chat command stays responsive.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::calendar::{self, CalendarEvent, CalendarInfo, IcsFeedInfo};
use crate::error::{Error, Result};
use crate::google;
use crate::ingest::{self, Document, IngestEvent};
use crate::projects::{self, ProjectOverview, ProjectProposalEvent};
use crate::retrieval::{self, Citation, RetrievedChunk};
use crate::review::{self, ReviewDecision, ReviewEvent};
use crate::sidecar::SidecarStatus;
use crate::{briefing, clock, cost, db, learning, openrouter, paths, secrets, AppState};

/// Fallback model when the user hasn't chosen one. Swappable in Settings and
/// stored as a plain string (spec §6 — never locked into a model).
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4.6";

/// Recommended model presets offered in Settings — the "sensible defaults" half of
/// the Cost Logger (spec §17.1): cheap/fast for high-volume background work, stronger
/// for chat. Consumed ONLY as the "use recommended" suggestion (and applied by the
/// user, who then Saves) — never by `effective_models`, which always reads the user's
/// stored list (spec §6: nothing hardcoded into the engine). `DEFAULT_CHAT_MODELS[0]`
/// stays == `DEFAULT_MODEL`. These are swappable strings, not pinned ids.
const DEFAULT_CHAT_MODELS: &[&str] = &["anthropic/claude-sonnet-4.6", "anthropic/claude-haiku-4.5"];
const DEFAULT_BACKGROUND_MODELS: &[&str] = &["anthropic/claude-haiku-4.5", "anthropic/claude-sonnet-4.6"];

/// Settings keys for the two model roles. Each holds a JSON array of model ids
/// (ordered, first = primary); the `*_AUTO_SWITCH` keys hold "true"/"false".
const CHAT_MODELS_KEY: &str = "chat_models";
const BACKGROUND_MODELS_KEY: &str = "background_models";
const CHAT_AUTO_SWITCH_KEY: &str = "chat_auto_switch";
const BACKGROUND_AUTO_SWITCH_KEY: &str = "background_auto_switch";

/// The user's IANA time-zone name (e.g. "America/New_York"), supplied by the
/// frontend via `Intl.DateTimeFormat().resolvedOptions().timeZone`. Empty/unset →
/// the backend reasons in UTC (see `resolve_zone`).
const TIME_ZONE_KEY: &str = "time_zone";

/// Caps for chat: the most we'll store for a single message, and how many prior
/// turns we replay into a request. A long conversation or one giant pasted message
/// would otherwise inflate every call (the spend lands on the user's own key).
/// Both are generous — far beyond any normal chat turn or history depth.
const MAX_MESSAGE_CHARS: usize = 100_000;
const MAX_HISTORY_MESSAGES: usize = 40;

#[derive(Serialize)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    /// The project this chat is scoped to (Step 5), or `None` for a global chat.
    /// A scoped chat's retrieval is confined to this project's documents.
    pub project: Option<String>,
}

#[derive(Serialize)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub created_at: String,
    /// Source documents this answer drew from (assistant turns only).
    pub citations: Option<Vec<Citation>>,
}

#[derive(Serialize)]
pub struct Settings {
    /// Ordered preferred models for chat (user-facing) and background work
    /// (sorting proposals + Learning You). First = primary; the rest are
    /// auto-switch fallbacks when the matching toggle is on.
    pub chat_models: Vec<String>,
    pub background_models: Vec<String>,
    pub chat_auto_switch: bool,
    pub background_auto_switch: bool,
    /// When on, the UI shows an explanation panel for whatever section the user
    /// hovers — a learn-the-app affordance (Step 4b). Defaults off.
    pub help_mode: bool,
    /// The user's IANA time zone (e.g. "Europe/London"), or "" when not yet set —
    /// the focus-view day boundaries and the briefing/agenda "now" reason in it.
    pub time_zone: String,
}

/// Streamed back to the UI over a Tauri channel as the assistant replies.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Token { text: String },
    Done {
        message_id: i64,
        content: String,
        citations: Vec<Citation>,
    },
    Error { message: String },
}

// --- secrets ---

#[tauri::command]
pub fn has_openrouter_key() -> Result<bool> {
    Ok(secrets::get_openrouter_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    secrets::set_openrouter_key(key)
}

#[tauri::command]
pub fn has_openrouter_background_key() -> Result<bool> {
    Ok(secrets::get_openrouter_background_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_background_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    secrets::set_openrouter_background_key(key)
}

// --- settings ---

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    let conn = state
        .db
        .lock()
        .map_err(|_| Error::Other("database lock poisoned".into()))?;
    Ok(Settings {
        chat_models: models_for(&conn, CHAT_MODELS_KEY)?,
        background_models: models_for(&conn, BACKGROUND_MODELS_KEY)?,
        chat_auto_switch: db::get_setting(&conn, CHAT_AUTO_SWITCH_KEY)?.as_deref() == Some("true"),
        background_auto_switch: db::get_setting(&conn, BACKGROUND_AUTO_SWITCH_KEY)?.as_deref()
            == Some("true"),
        help_mode: db::get_setting(&conn, "help_mode")?.as_deref() == Some("true"),
        time_zone: db::get_setting(&conn, TIME_ZONE_KEY)?.unwrap_or_default(),
    })
}

#[tauri::command]
pub fn set_chat_models(state: State<'_, AppState>, models: Vec<String>) -> Result<()> {
    let conn = state.db.lock().unwrap();
    save_models(&conn, CHAT_MODELS_KEY, models)
}

#[tauri::command]
pub fn set_background_models(state: State<'_, AppState>, models: Vec<String>) -> Result<()> {
    let conn = state.db.lock().unwrap();
    save_models(&conn, BACKGROUND_MODELS_KEY, models)
}

#[tauri::command]
pub fn set_chat_auto_switch(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.db.lock().unwrap();
    db::set_setting(&conn, CHAT_AUTO_SWITCH_KEY, if enabled { "true" } else { "false" })
}

#[tauri::command]
pub fn set_background_auto_switch(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.db.lock().unwrap();
    db::set_setting(&conn, BACKGROUND_AUTO_SWITCH_KEY, if enabled { "true" } else { "false" })
}

/// Toggle the UI help/explain mode (Step 4b). Stored in `settings` so it persists.
#[tauri::command]
pub fn set_help_mode(state: State<'_, AppState>, enabled: bool) -> Result<()> {
    let conn = state.db.lock().unwrap();
    db::set_setting(&conn, "help_mode", if enabled { "true" } else { "false" })
}

/// The stored IANA time zone (empty string = none set; the backend then uses UTC).
#[tauri::command]
pub fn get_time_zone(state: State<'_, AppState>) -> Result<String> {
    let conn = state.db.lock().unwrap();
    Ok(db::get_setting(&conn, TIME_ZONE_KEY)?.unwrap_or_default())
}

/// Persist the IANA zone the frontend resolved via `Intl`. Validated against the
/// chrono-tz database so a garbage string can't be stored; an empty value clears it
/// (the backend falls back to UTC). This is correctness state, not appearance, so it
/// lives in the backend `settings` table where the SQL/date logic reads it.
#[tauri::command]
pub fn set_time_zone(state: State<'_, AppState>, zone: String) -> Result<()> {
    use std::str::FromStr;
    let zone = zone.trim();
    if !zone.is_empty() && chrono_tz::Tz::from_str(zone).is_err() {
        return Err(Error::Other(format!("unrecognised time zone: {zone}")));
    }
    let conn = state.db.lock().unwrap();
    db::set_setting(&conn, TIME_ZONE_KEY, zone)
}

/// The OpenRouter model catalogue (public endpoint, no key needed) so the user can
/// browse, search, and pick a model with pricing in Settings (spec §6 — any model,
/// swappable).
#[tauri::command]
pub async fn list_models() -> Result<Vec<openrouter::ModelInfo>> {
    openrouter::list_models().await
}

// --- conversations & messages ---

#[tauri::command]
pub fn list_conversations(state: State<'_, AppState>) -> Result<Vec<Conversation>> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, project FROM conversations \
         ORDER BY updated_at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([], row_to_conversation)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Start a conversation. `project` scopes it to one project (Step 5) — the
/// per-project view passes it so the chat's retrieval narrows to that project;
/// `None` is a normal global chat.
#[tauri::command]
pub fn create_conversation(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<Conversation> {
    let project = project.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT INTO conversations(project) VALUES (?1)",
        params![project],
    )?;
    let id = conn.last_insert_rowid();
    Ok(conn.query_row(
        "SELECT id, title, created_at, updated_at, project FROM conversations WHERE id = ?1",
        params![id],
        row_to_conversation,
    )?)
}

#[tauri::command]
pub fn get_messages(state: State<'_, AppState>, conversation_id: i64) -> Result<Vec<Message>> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, conversation_id, role, content, model, created_at, citations \
         FROM messages WHERE conversation_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![conversation_id], row_to_message)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Persist the user's turn, stream the assistant's reply from OpenRouter (tokens
/// pushed over `on_event`), then persist the assistant's turn.
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: i64,
    content: String,
    on_event: Channel<ChatEvent>,
) -> Result<()> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(Error::Other("message is empty".into()));
    }
    // Cap the stored/sent message so one multi-MB paste can't bloat the store and
    // every following request.
    let content: String = content.chars().take(MAX_MESSAGE_CHARS).collect();

    let api_key = secrets::get_openrouter_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    // Save the user turn and gather history + models + the learned profile + the
    // conversation's project scope. Scope the lock so the guard is dropped before
    // the network await below.
    let (history, models, profile, scope, agenda) = {
        let conn = state.db.lock().unwrap();

        let prior: i64 = conn.query_row(
            "SELECT count(*) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;

        // A project-scoped chat (Step 5) confines retrieval to that project's docs.
        let scope: Option<String> = conn.query_row(
            "SELECT project FROM conversations WHERE id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;

        conn.execute(
            "INSERT INTO messages(conversation_id, role, content) VALUES (?1, 'user', ?2)",
            params![conversation_id, content],
        )?;

        // Name a fresh conversation after its first message.
        if prior == 0 {
            let title: String = content.chars().take(48).collect();
            conn.execute(
                "UPDATE conversations SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![title, conversation_id],
            )?;
        }

        let models = effective_models(&conn, CHAT_MODELS_KEY, CHAT_AUTO_SWITCH_KEY)?;
        // Replay only the most recent turns (newest N by id, then back into
        // chronological order) so a long conversation can't grow every request.
        let mut stmt = conn.prepare(
            "SELECT role, content FROM \
                 (SELECT id, role, content FROM messages WHERE conversation_id = ?1 \
                  ORDER BY id DESC LIMIT ?2) \
             ORDER BY id",
        )?;
        let history = stmt
            .query_map(params![conversation_id, MAX_HISTORY_MESSAGES as i64], |row| {
                Ok(openrouter::ChatMessage {
                    role: row.get(0)?,
                    content: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let profile = learning::profile_preamble(&conn)?;
        // Give a global (unscoped) chat the user's upcoming agenda so it can answer
        // "what's on at 3pm?" (Step 6). A project-scoped chat stays on its documents.
        let agenda = if scope.is_none() {
            calendar::agenda_preamble(&conn, 7, resolve_zone(&conn))?
        } else {
            None
        };
        (history, models, profile, scope, agenda)
    };

    // Ground the answer in the user's files (best-effort): retrieve the most
    // relevant chunks and prepend them as a system message the model must cite.
    // If retrieval yields nothing (no docs / engine not ready), chat proceeds
    // exactly as before. A scoped chat draws only from its project.
    let retrieved = retrieve_grounding(&app, content.clone(), scope).await;
    let citations = retrieval::citations_from(&retrieved);

    let mut messages = Vec::with_capacity(history.len() + 2);
    // The learned profile goes first so the model carries the user's habits into
    // every answer (Step 4b, spec §4.5); then the grounding sources, then history.
    if let Some(profile) = &profile {
        messages.push(openrouter::ChatMessage {
            role: "system".into(),
            content: profile.clone(),
        });
    }
    if let Some(agenda) = &agenda {
        messages.push(openrouter::ChatMessage {
            role: "system".into(),
            content: agenda.clone(),
        });
    }
    if !retrieved.is_empty() {
        messages.push(openrouter::ChatMessage {
            role: "system".into(),
            content: retrieval::grounding_prompt(&retrieved),
        });
    }
    messages.extend(history);

    // Stream the reply, forwarding each token to the UI.
    let result = openrouter::stream_chat(&api_key, &models, &messages, |token| {
        let _ = on_event.send(ChatEvent::Token {
            text: token.to_string(),
        });
    })
    .await;

    let completion = match result {
        Ok(c) => c,
        Err(e) => {
            let _ = on_event.send(ChatEvent::Error {
                message: e.to_string(),
            });
            return Err(e);
        }
    };
    let reply = completion.text;
    let usage = completion.usage;
    // Record the model that actually answered — the served one (so a fallback is
    // reflected), falling back to the requested primary if it wasn't reported.
    let used_model = completion
        .model
        .unwrap_or_else(|| models.first().cloned().unwrap_or_default());

    // Persist the assistant turn with the documents it cited (JSON, or NULL).
    let citations_json = if citations.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&citations).map_err(|e| Error::Other(e.to_string()))?)
    };
    let message_id = {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO messages(conversation_id, role, content, model, citations) \
             VALUES (?1, 'assistant', ?2, ?3, ?4)",
            params![conversation_id, reply, used_model, citations_json],
        )?;
        let id = conn.last_insert_rowid();
        log_usage(&conn, "chat", Some(&used_model), &usage);
        conn.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            params![conversation_id],
        )?;
        id
    };

    let _ = on_event.send(ChatEvent::Done {
        message_id,
        content: reply,
        citations,
    });
    Ok(())
}

/// Retrieve grounding chunks for a chat query — best-effort. Returns an empty
/// list (so chat falls back to ungrounded answering) if there are no documents
/// or the document engine isn't ready yet; never errors out the chat. Runs the
/// blocking embed + search off the async runtime, and never holds the DB lock
/// across the sidecar embed call (AGENTS rule #4).
async fn retrieve_grounding(
    app: &AppHandle,
    query: String,
    project: Option<String>,
) -> Vec<RetrievedChunk> {
    let app = app.clone();
    let task = tokio::task::spawn_blocking(move || -> Result<Vec<RetrievedChunk>> {
        let state = app.state::<AppState>();

        // Nothing to ground on?
        let has_docs: bool = {
            let conn = state.db.lock().unwrap();
            conn.query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))?
        };
        if !has_docs {
            return Ok(Vec::new());
        }
        // Don't trigger a slow first-run install mid-chat — only embed if ready.
        if !matches!(state.sidecar.status(), SidecarStatus::Ready) {
            return Ok(Vec::new());
        }

        let embeddings = state.sidecar.embed(&[query.clone()])?;
        let Some(query_vec) = embeddings.into_iter().next() else {
            return Ok(Vec::new());
        };

        let conn = state.db.lock().unwrap();
        retrieval::hybrid_search(&conn, &query, &query_vec, retrieval::DEFAULT_TOP_K, project.as_deref())
    })
    .await;

    match task {
        Ok(Ok(chunks)) => chunks,
        _ => Vec::new(),
    }
}

// --- archivist: documents ---

/// Where the document engine (Python sidecar) is in its lifecycle, so the UI can
/// show first-run setup.
#[tauri::command]
pub fn sidecar_status(state: State<'_, AppState>) -> SidecarStatus {
    state.sidecar.status()
}

/// Provision the managed venv if needed (slow on first run). Run off the async
/// runtime so the UI stays responsive.
#[tauri::command]
pub async fn ensure_sidecar(app: AppHandle) -> Result<()> {
    tokio::task::spawn_blocking(move || app.state::<AppState>().sidecar.ensure_installed())
        .await
        .map_err(|e| Error::Other(format!("setup task panicked: {e}")))?
}

/// Ingest files/folders: convert → chunk → embed → index. Progress streams over
/// `on_event`. The whole pipeline is blocking, so it runs on a blocking thread.
///
/// `paths` are raw filesystem paths, so this is effectively an arbitrary-file-read
/// primitive — deliberately trusted: the only caller is PM's own webview, and the
/// paths come from the user's drag-drop / file-dialog (the same reach the dialog
/// already grants). It is not exposed to any external/untrusted caller.
#[tauri::command]
pub async fn ingest_paths(
    app: AppHandle,
    paths: Vec<String>,
    on_event: Channel<IngestEvent>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || ingest::run(&app, paths, on_event))
        .await
        .map_err(|e| Error::Other(format!("ingest task panicked: {e}")))?
}

/// Drop the index and rebuild it from the Markdown vault (spec §3 acceptance).
#[tauri::command]
pub async fn rebuild_index(app: AppHandle, on_event: Channel<IngestEvent>) -> Result<()> {
    tokio::task::spawn_blocking(move || ingest::rebuild(&app, on_event))
        .await
        .map_err(|e| Error::Other(format!("rebuild task panicked: {e}")))?
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> Result<Vec<Document>> {
    let conn = state.db.lock().unwrap();
    ingest::list_documents(&conn)
}

/// Direct hybrid search over the store — the retrieval loop exposed on its own
/// (a search surface, and the way to verify exact-term recall independently of
/// chat). Embeds the query via the sidecar, so it ensures the engine is set up.
#[tauri::command]
pub async fn search_documents(
    app: AppHandle,
    query: String,
    k: Option<usize>,
) -> Result<Vec<RetrievedChunk>> {
    const MAX_K: usize = 50;
    let k = k.unwrap_or(retrieval::DEFAULT_TOP_K).min(MAX_K);
    tokio::task::spawn_blocking(move || -> Result<Vec<RetrievedChunk>> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;

        let embeddings = state.sidecar.embed(&[query.clone()])?;
        let query_vec = embeddings.into_iter().next().unwrap_or_default();

        let conn = state.db.lock().unwrap();
        retrieval::hybrid_search(&conn, &query, &query_vec, k, None)
    })
    .await
    .map_err(|e| Error::Other(format!("search task panicked: {e}")))?
}

/// Transcribe a recorded voice clip to text for the chat box (spec §4 P1 — voice
/// input). The webview records the clip and sends it base64-encoded; we decode it
/// to a temp file inside the data dir, transcribe it locally via the sidecar's
/// Whisper model, and delete the file. An explicit user action, so it ensures the
/// engine is installed first (mirrors `search_documents`). Fully on-device — the
/// audio never leaves the machine. All blocking, so it runs off the async runtime.
#[tauri::command]
pub async fn transcribe_audio(app: AppHandle, audio_base64: String) -> Result<String> {
    use base64::Engine;

    tokio::task::spawn_blocking(move || -> Result<String> {
        // Bound the untrusted webview payload before allocating the decode buffer
        // (every other webview input is capped). ~32 MiB of base64 ≈ 24 MiB of
        // audio — far more than a dictation clip, but it stops a hostile/oversized
        // string from ballooning memory on a low-RAM machine.
        const MAX_AUDIO_B64_CHARS: usize = 32 * 1024 * 1024;
        let b64 = audio_base64.trim();
        if b64.len() > MAX_AUDIO_B64_CHARS {
            return Err(Error::Other("the recording is too large to transcribe".into()));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::Other(format!("could not decode the recording: {e}")))?;
        if bytes.is_empty() {
            return Ok(String::new());
        }

        // Keep the clip inside PM's data dir (not the system temp) so it shares the
        // user's at-rest disk encryption. A random-named NamedTempFile deletes
        // itself on drop (RAII), so even a crash mid-transcribe can't leave the raw
        // audio behind under a predictable name.
        use std::io::Write;
        let tmp_dir = paths::data_dir(&app)?.join("runtime").join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let mut clip = tempfile::Builder::new()
            .prefix("voice-")
            .suffix(".webm")
            .tempfile_in(&tmp_dir)?;
        clip.write_all(&bytes)?;
        clip.flush()?;

        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
        let text = state.sidecar.transcribe(clip.path());

        // `clip` drops at end of scope, deleting the temp file on success or error.
        text
    })
    .await
    .map_err(|e| Error::Other(format!("transcription task panicked: {e}")))?
}

// --- archivist: sorting review & organisation (Step 4) ---

/// Distinct project labels across all documents — feeds the review project picker
/// and biases the AI proposal toward projects that already exist.
#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<String>> {
    let conn = state.db.lock().unwrap();
    db::distinct_projects(&conn)
}

/// Documents still awaiting the sorting review (`reviewed = 0`).
#[tauri::command]
pub fn review_queue(state: State<'_, AppState>) -> Result<Vec<Document>> {
    let conn = state.db.lock().unwrap();
    ingest::review_queue(&conn)
}

/// Propose project/tags/importance for the unreviewed documents, on demand (so a
/// big folder import doesn't auto-fire model calls). Proposals stream back over
/// `on_event`; they're transient — the user confirms them via `commit_review`.
/// Runs on the background API key; never holds the DB lock across a model call.
#[tauri::command]
pub async fn propose_metadata(
    app: AppHandle,
    document_ids: Option<Vec<i64>>,
    on_event: Channel<ReviewEvent>,
) -> Result<()> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    // Bound the (untrusted webview) id list: it expands to one SQL placeholder
    // each, so an unbounded list would blow SQLITE_MAX_VARIABLE_NUMBER. Far above
    // any real review selection.
    const MAX_PROPOSE_IDS: usize = 10_000;
    if document_ids.as_ref().is_some_and(|ids| ids.len() > MAX_PROPOSE_IDS) {
        return Err(Error::Other("too many documents selected at once".into()));
    }

    struct Pending {
        id: i64,
        title: String,
        body: String,
    }

    // Gather the documents + existing projects + learned profile under a short
    // lock, then drop it before any network call (rule #4).
    let (pending, projects, models, profile) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let profile = learning::profile_preamble(&conn)?;
        let projects = db::distinct_projects(&conn)?;
        let pending = {
            let base_sql = "SELECT d.id, d.title, \
                    COALESCE((SELECT content FROM chunks c WHERE c.document_id = d.id ORDER BY ordinal LIMIT 1), '') \
             FROM documents d WHERE d.reviewed = 0";

            let pending_sql = if let Some(ids) = document_ids.as_ref() {
                if ids.is_empty() {
                    format!("{base_sql} AND 1=0 ORDER BY d.ingested_at DESC, d.id DESC")
                } else {
                    let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(", ");
                    format!("{base_sql} AND d.id IN ({placeholders}) ORDER BY d.ingested_at DESC, d.id DESC")
                }
            } else {
                format!("{base_sql} ORDER BY d.ingested_at DESC, d.id DESC")
            };

            let mut stmt = conn.prepare(&pending_sql)?;
            if let Some(ids) = document_ids.as_ref().filter(|ids| !ids.is_empty()) {
                stmt.query_map(
                    rusqlite::params_from_iter(ids),
                    |r| Ok(Pending { id: r.get(0)?, title: r.get(1)?, body: r.get(2)? }),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
            } else {
                stmt.query_map([], |r| Ok(Pending { id: r.get(0)?, title: r.get(1)?, body: r.get(2)? }))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        (pending, projects, models, profile)
    };

    let mut proposed = 0;
    let mut usage_rows: Vec<(Option<String>, openrouter::Usage)> = Vec::new();
    for p in pending {
        let (proposal, usage_info) =
            review::propose(&api_key, &models, &p.title, &p.body, &projects, profile.as_deref()).await;
        if let Some((usage, served)) = usage_info {
            usage_rows.push((served, usage));
        }
        let _ = on_event.send(ReviewEvent::Proposed { document_id: p.id, proposal });
        proposed += 1;
    }
    log_background_usage(&app, &models, &usage_rows);
    let _ = on_event.send(ReviewEvent::Finished { proposed });
    Ok(())
}

/// Commit a review pass: for each decision, log the fields the user changed from
/// the AI proposal, then write the confirmed metadata to the vault + DB and mark
/// the document reviewed. Blocking (file rewrites), so it runs off the runtime.
#[tauri::command]
pub async fn commit_review(app: AppHandle, decisions: Vec<ReviewDecision>) -> Result<()> {
    let vault = paths::vault_dir(&app)?;
    let blocking_app = app.clone();
    let logged = tokio::task::spawn_blocking(move || -> Result<usize> {
        let state = blocking_app.state::<AppState>();
        let now = iso_now(&state)?;

        // The whole pass is all-or-nothing: corrections, vault rewrites, and the
        // `reviewed` flags commit together, or the DB transaction rolls back and
        // every vault file we touched is restored. Otherwise a failure partway
        // through would leave earlier docs marked reviewed (dropped from the queue
        // on retry, their corrections never re-logged) and mid-batch vault/DB drift.
        let mut conn = state.db.lock().unwrap();
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, String)> = Vec::new();

        let result: Result<usize> = (|| {
            let mut logged = 0usize;
            for d in &decisions {
                let title: String = tx
                    .query_row("SELECT title FROM documents WHERE id = ?1", params![d.document_id], |r| r.get(0))
                    .unwrap_or_default();
                logged += review::log_corrections(&tx, d, &title)?;
                let importance = review::normalize_importance(d.importance.clone());
                let w = ingest::rewrite_vault_metadata(
                    &tx, &vault, d.document_id, &d.project, &d.tags, importance.as_deref(), true, &now,
                )?;
                written.push(w);
            }
            Ok(logged)
        })();

        match result {
            Ok(logged) => match tx.commit() {
                Ok(()) => Ok(logged),
                Err(e) => {
                    ingest::restore_vault_files(written);
                    Err(e.into())
                }
            },
            Err(e) => {
                drop(tx); // roll back the DB side
                ingest::restore_vault_files(written);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| Error::Other(format!("commit task panicked: {e}")))??;

    // If the user corrected anything, refresh the Learning-You profile in the
    // background — one model call per review pass (not per document), best-effort
    // and non-blocking so the UI returns immediately (Step 4b, spec §4.5).
    if logged > 0 {
        tauri::async_runtime::spawn(async move {
            let _ = run_profile_refresh(app).await;
        });
    }
    Ok(())
}

/// Edit one already-reviewed document's metadata (the after-the-fact "this is
/// Project 2, not 3"). Logs the change against the currently stored values.
#[tauri::command]
pub async fn set_document_metadata(
    app: AppHandle,
    document_id: i64,
    project: String,
    tags: Vec<String>,
    importance: Option<String>,
) -> Result<Document> {
    let vault = paths::vault_dir(&app)?;
    let importance = review::normalize_importance(importance);
    tokio::task::spawn_blocking(move || -> Result<Document> {
        let state = app.state::<AppState>();
        let now = iso_now(&state)?;

        // Log the correction + rewrite the vault file + update the row atomically,
        // restoring the vault file if the DB side fails (the file write lands first).
        let mut conn = state.db.lock().unwrap();
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, String)> = Vec::new();

        let work = (|| -> Result<()> {
            let (cur_project, cur_tags_json, cur_importance, title): (String, String, Option<String>, String) = tx
                .query_row(
                    "SELECT project, tags, importance, title FROM documents WHERE id = ?1",
                    params![document_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
            let decision = ReviewDecision {
                document_id,
                project: project.clone(),
                tags: tags.clone(),
                importance: importance.clone(),
                proposed_project: cur_project,
                proposed_tags: serde_json::from_str(&cur_tags_json).unwrap_or_default(),
                proposed_importance: cur_importance,
            };
            review::log_corrections(&tx, &decision, &title)?;
            written.push(ingest::rewrite_vault_metadata(
                &tx, &vault, document_id, &project, &tags, importance.as_deref(), true, &now,
            )?);
            Ok(())
        })();

        if let Err(e) = work {
            drop(tx);
            ingest::restore_vault_files(written);
            return Err(e);
        }
        if let Err(e) = tx.commit() {
            ingest::restore_vault_files(written);
            return Err(e.into());
        }
        ingest::load_document(&conn, document_id)
    })
    .await
    .map_err(|e| Error::Other(format!("update task panicked: {e}")))?
}

// --- personal assistant: projects & focus view (Step 5) ---

/// Every active project with its triage metadata and one derived status — the
/// focus view's data (spec §4.1).
#[tauri::command]
pub fn list_project_overviews(state: State<'_, AppState>) -> Result<Vec<ProjectOverview>> {
    let conn = state.db.lock().unwrap();
    let today = clock::today_sql_in(resolve_zone(&conn));
    projects::list_overviews(&conn, &today)
}

/// Set (or update) a project's triage metadata — the user confirming/correcting an
/// AI proposal, or editing by hand in the focus/project view. Creates the row on
/// first set; blanks clear a field.
#[tauri::command]
pub fn set_project_metadata(
    state: State<'_, AppState>,
    name: String,
    deadline: Option<String>,
    size: Option<String>,
    blocked_by: Option<String>,
    parent: Option<String>,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Other("project name is empty".into()));
    }
    let conn = state.db.lock().unwrap();
    projects::set_metadata(&conn, name, deadline, size, blocked_by, parent)
}

/// Propose triage metadata (size/parent/blocked-by/deadline) for projects, on
/// demand — the AI-proposes-you-confirm half of the focus view, mirroring
/// `propose_metadata`. `names` limits it to specific projects (default: all).
/// Proposals stream over `on_event`; the user confirms via `set_project_metadata`.
/// Runs on the background API key; never holds the DB lock across a model call.
#[tauri::command]
pub async fn propose_project_metadata(
    app: AppHandle,
    names: Option<Vec<String>>,
    on_event: Channel<ProjectProposalEvent>,
) -> Result<()> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    // Bound the (untrusted webview) name list — one model call per name, so this
    // also caps runaway spend. Far above any real project count.
    const MAX_PROPOSE_NAMES: usize = 2_000;
    if names.as_ref().is_some_and(|n| n.len() > MAX_PROPOSE_NAMES) {
        return Err(Error::Other("too many projects selected at once".into()));
    }

    struct Target {
        name: String,
        samples: Vec<String>,
    }

    // Gather targets + their document samples + the full project list (for picking
    // a real parent/blocker) + models under a short lock, then drop it (rule #4).
    let (targets, all_projects, models) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let all_projects: Vec<String> = db::distinct_projects(&conn)?;
        let target_names = match names {
            Some(n) if !n.is_empty() => n,
            _ => all_projects.clone(),
        };
        let mut targets = Vec::new();
        for name in target_names {
            let samples = projects::document_samples(&conn, &name)?;
            targets.push(Target { name, samples });
        }
        (targets, all_projects, models)
    };

    let mut proposed = 0;
    let mut usage_rows: Vec<(Option<String>, openrouter::Usage)> = Vec::new();
    for t in targets {
        let others: Vec<String> = all_projects.iter().filter(|p| **p != t.name).cloned().collect();
        let (proposal, usage_info) =
            projects::propose(&api_key, &models, &t.name, &t.samples, &others).await;
        if let Some((usage, served)) = usage_info {
            usage_rows.push((served, usage));
        }
        let _ = on_event.send(ProjectProposalEvent::Proposed {
            project: t.name,
            proposal,
        });
        proposed += 1;
    }
    log_background_usage(&app, &models, &usage_rows);
    let _ = on_event.send(ProjectProposalEvent::Finished { proposed });
    Ok(())
}

// --- personal assistant: calendar (Step 6) ---

/// The calendar connector's state, for the Settings panel. Covers both paths: the
/// simple .ics feeds and the advanced Google OAuth sign-in.
#[derive(Serialize)]
pub struct CalendarStatus {
    /// How many .ics feeds are subscribed (the no-OAuth path).
    pub ics_feeds: usize,
    /// The user has pasted a Google client id + secret.
    pub oauth_client_configured: bool,
    /// A Google OAuth token is stored (sign-in completed).
    pub oauth_connected: bool,
    /// How many Google calendars are selected to sync.
    pub calendars_selected: usize,
    /// ISO timestamp of the last successful sync, if any.
    pub last_sync: Option<String>,
    /// How far ahead PM mirrors events (and the agenda horizon), in days.
    pub window_days: i64,
}

#[tauri::command]
pub fn calendar_status(state: State<'_, AppState>) -> Result<CalendarStatus> {
    let conn = state.db.lock().unwrap();
    Ok(CalendarStatus {
        ics_feeds: calendar::load_feeds()?.len(),
        oauth_client_configured: google::has_client()?,
        oauth_connected: google::is_connected()?,
        calendars_selected: calendar::selected_calendar_ids(&conn)?.len(),
        last_sync: calendar::last_sync(&conn)?,
        window_days: calendar::AGENDA_DAYS,
    })
}

// .ics feeds — the no-OAuth path (works under Advanced Protection).

/// Subscribed feeds without their secret URLs, for Settings.
#[tauri::command]
pub fn list_ics_feeds() -> Result<Vec<IcsFeedInfo>> {
    calendar::feed_infos()
}

/// Add an .ics feed and sync it immediately so its events appear. If it can't be
/// fetched/parsed, it's rolled back so a broken feed isn't left behind.
#[tauri::command]
pub async fn add_ics_feed(app: AppHandle, label: String, url: String) -> Result<()> {
    let feed = calendar::add_feed(&label, &url)?;
    // Resolve the user's zone (for floating/all-day ICS times) under a short lock,
    // then drop it before the network sync (rule #4).
    let tz = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        resolve_zone(&conn)
    };
    match calendar::sync_feed(&feed, tz).await {
        Ok(events) => {
            let state = app.state::<AppState>();
            let conn = state.db.lock().unwrap();
            calendar::replace_events(&conn, &feed.id, &events)?;
            calendar::set_last_sync(&conn)?;
            Ok(())
        }
        Err(e) => {
            let state = app.state::<AppState>();
            let conn = state.db.lock().unwrap();
            let _ = calendar::remove_feed(&conn, &feed.id);
            Err(e)
        }
    }
}

/// Remove a feed and its mirrored events.
#[tauri::command]
pub fn remove_ics_feed(state: State<'_, AppState>, id: String) -> Result<()> {
    let conn = state.db.lock().unwrap();
    calendar::remove_feed(&conn, &id)
}

/// Store the user's BYO Google "Desktop app" client credentials (keychain only).
#[tauri::command]
pub fn set_google_client(client_id: String, client_secret: String) -> Result<()> {
    let id = client_id.trim();
    let secret = client_secret.trim();
    if id.is_empty() || secret.is_empty() {
        return Err(Error::Other("Both the Client ID and Client secret are required.".into()));
    }
    secrets::set_google_client(id, secret)
}

/// Forget the client credentials (also disconnects + clears the mirror, since the
/// token belongs to that client).
#[tauri::command]
pub fn clear_google_client(state: State<'_, AppState>) -> Result<()> {
    secrets::clear_google_token().ok();
    secrets::clear_google_client()?;
    let conn = state.db.lock().unwrap();
    calendar::clear_all_events(&conn)
}

/// Run the OAuth consent flow (opens the system browser; resolves once the user
/// signs in or it times out).
#[tauri::command]
pub async fn connect_google() -> Result<()> {
    google::connect().await
}

/// Sign out: forget the token and clear the mirrored events. Client credentials stay
/// so the user can reconnect without re-entering them.
#[tauri::command]
pub fn disconnect_google(state: State<'_, AppState>) -> Result<()> {
    secrets::clear_google_token()?;
    let conn = state.db.lock().unwrap();
    calendar::clear_all_events(&conn)
}

/// The user's calendars, with PM's current selection applied (for the picker).
#[tauri::command]
pub async fn list_google_calendars(app: AppHandle) -> Result<Vec<CalendarInfo>> {
    let raw = calendar::fetch_calendar_list().await?;
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let selected = calendar::selected_calendar_ids(&conn)?;
    Ok(calendar::to_calendar_infos(raw, &selected))
}

/// Choose which calendars to sync.
#[tauri::command]
pub fn set_google_calendar_ids(state: State<'_, AppState>, ids: Vec<String>) -> Result<()> {
    let conn = state.db.lock().unwrap();
    calendar::set_selected_calendar_ids(&conn, &ids)
}

/// Pull events from both sources (subscribed .ics feeds + selected Google calendars)
/// into the local mirror. Returns the number of events synced. Best-effort per source
/// and never holds the DB lock across a fetch (rule #4); surfaces an error only if
/// every source failed (so a transient miss keeps the last-good events).
#[tauri::command]
pub async fn sync_calendar(app: AppHandle) -> Result<usize> {
    let (oauth_ids, feeds, time_min, time_max, tz) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        let oauth_ids = if google::is_connected()? {
            calendar::selected_calendar_ids(&conn)?
        } else {
            Vec::new()
        };
        let feeds = calendar::load_feeds()?;
        let (min, max) = calendar::time_window(&conn)?;
        (oauth_ids, feeds, min, max, resolve_zone(&conn))
    };

    // Every calendar/feed we intend to keep events for — anything else is pruned.
    let mut active: Vec<String> = oauth_ids.clone();
    active.extend(feeds.iter().map(|f| f.id.clone()));

    if active.is_empty() {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        calendar::clear_all_events(&conn)?;
        calendar::set_last_sync(&conn)?;
        return Ok(0);
    }

    let mut total = 0usize;
    let mut last_err: Option<Error> = None;

    for id in &oauth_ids {
        match calendar::fetch_events(id, &time_min, &time_max).await {
            Ok(events) => {
                let state = app.state::<AppState>();
                let conn = state.db.lock().unwrap();
                calendar::replace_events(&conn, id, &events)?;
                total += events.len();
            }
            Err(e) => last_err = Some(e),
        }
    }
    for feed in &feeds {
        match calendar::sync_feed(feed, tz).await {
            Ok(events) => {
                let state = app.state::<AppState>();
                let conn = state.db.lock().unwrap();
                calendar::replace_events(&conn, &feed.id, &events)?;
                total += events.len();
            }
            Err(e) => last_err = Some(e),
        }
    }

    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        // Reconcile deselected calendars. A source that failed *this* round keeps
        // its last-good events (standard cache behaviour) rather than being blanked.
        calendar::prune_unselected(&conn, &active)?;
        // Only record a clean sync when every selected source refreshed — a partial
        // failure must not hide behind a fresh "last synced" timestamp.
        if last_err.is_none() {
            calendar::set_last_sync(&conn)?;
        }
    }

    // Surface any source failure (auth/expired, or a bad feed URL) even when other
    // sources succeeded; the successful ones are already committed above.
    if let Some(e) = last_err {
        return Err(e);
    }
    Ok(total)
}

/// The upcoming events in the mirror, for the focus-view agenda.
#[tauri::command]
pub fn list_calendar_events(state: State<'_, AppState>) -> Result<Vec<CalendarEvent>> {
    let conn = state.db.lock().unwrap();
    calendar::list_upcoming(&conn, calendar::AGENDA_DAYS)
}

// --- learning you (Step 4b) ---

/// The distilled Learning-You profile + when it was last updated and how many
/// corrections back it, for display in Settings.
#[tauri::command]
pub fn get_learning_profile(state: State<'_, AppState>) -> Result<learning::LearningProfile> {
    let conn = state.db.lock().unwrap();
    learning::get_profile(&conn)
}

/// Re-distil the Learning-You profile from the logged corrections, on demand
/// (the "Refresh now" button). Returns the refreshed profile.
#[tauri::command]
pub async fn refresh_learning_profile(app: AppHandle) -> Result<learning::LearningProfile> {
    run_profile_refresh(app.clone()).await?;
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    learning::get_profile(&conn)
}

/// Gather corrections + the current profile, distil an updated profile via the
/// background model, and persist it. Background work: runs on the background API
/// key and never holds the DB lock across the model call (rule #4). A no-op when
/// there are no corrections to learn from yet.
async fn run_profile_refresh(app: AppHandle) -> Result<()> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    let (current, corrections, models) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let current = learning::get_profile(&conn)?.profile;
        let corrections = learning::recent_corrections(&conn, learning::MAX_CORRECTIONS)?;
        (current, corrections, models)
    };

    if corrections.is_empty() {
        return Ok(());
    }

    let (updated, usage, served) = learning::distill(&api_key, &models, &current, &corrections).await?;

    let state = app.state::<AppState>();
    let now = iso_now(&state)?;
    let conn = state.db.lock().unwrap();
    log_usage(&conn, "background", served.as_deref().or_else(|| models.first().map(String::as_str)), &usage);
    learning::save_profile(&conn, &updated, &now)
}

// --- daily briefing (Step 7, spec §4 P1) ---

/// The stored "here's your picture today" briefing + whether it's due a refresh, for
/// the focus view. Read-only — no model call, so it's cheap on every mount.
#[tauri::command]
pub fn get_daily_briefing(state: State<'_, AppState>) -> Result<briefing::DailyBriefing> {
    let conn = state.db.lock().unwrap();
    briefing::get_briefing(&conn)
}

/// Regenerate the daily briefing from the current focus-view state (the "Refresh"
/// button, and the focus view's once-a-day auto-refresh when stale). Returns the new
/// briefing. Background work: runs on the background API key, never holds the DB lock
/// across the model call (rule #4), and is a no-op (returns the stored value) when
/// there's nothing to summarise.
#[tauri::command]
pub async fn refresh_daily_briefing(app: AppHandle) -> Result<briefing::DailyBriefing> {
    let api_key = secrets::get_background_or_primary_key()?
        .ok_or_else(|| Error::Other("No OpenRouter API key set. Add one in Settings.".into()))?;

    let (snapshot, profile, models) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        let models = effective_models(&conn, BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY)?;
        let zone = resolve_zone(&conn);
        let now = clock::now_local_iso(zone);
        let today = clock::today_sql_in(zone);
        let projects = projects::list_overviews(&conn, &today)?;
        let events = calendar::list_upcoming(&conn, briefing::BRIEFING_AGENDA_DAYS)?;
        let snapshot = briefing::build_snapshot(&projects, &events, &now, zone);
        let profile = learning::profile_preamble(&conn)?;
        (snapshot, profile, models)
    };

    // Nothing to brief on yet — leave any prior briefing in place.
    let Some(snapshot) = snapshot else {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        return briefing::get_briefing(&conn);
    };

    let (text, usage, served) =
        briefing::generate(&api_key, &models, &snapshot, profile.as_deref()).await?;

    let state = app.state::<AppState>();
    let now = iso_now(&state)?;
    let conn = state.db.lock().unwrap();
    log_usage(&conn, "background", served.as_deref().or_else(|| models.first().map(String::as_str)), &usage);
    briefing::save_briefing(&conn, &text, &now)?;
    briefing::get_briefing(&conn)
}

// --- cost logger (spec §11.2 / §17.1) ---

/// Spend for one model over a window. `cost_usd` is `None` when the model isn't in
/// the price cache yet — surfaced as "—", never an understated $0.
#[derive(Serialize)]
pub struct ModelSpend {
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Option<f64>,
}

/// The Settings "Usage & cost" payload: per-model spend over two windows + totals,
/// plus when the cached pricing was last refreshed.
#[derive(Serialize)]
pub struct CostSummary {
    pub last_30d: Vec<ModelSpend>,
    pub all_time: Vec<ModelSpend>,
    pub total_30d_usd: Option<f64>,
    pub total_all_time_usd: Option<f64>,
    pub pricing_updated_at: Option<String>,
}

/// Per-model spend (trailing 30 days + all time) joined against the cached OpenRouter
/// prices. CHECK-ON-READ: if the price cache is empty or older than a day, refresh it
/// from the public catalogue first (no key, no model call, no scheduler — mirrors the
/// briefing's staleness rule). Read-mostly; safe on every Settings open.
#[tauri::command]
pub async fn cost_summary(app: AppHandle) -> Result<CostSummary> {
    // Best-effort refresh: if it fails (offline, etc.) still return the summary —
    // token counts come from the local log and need no network; only the priced
    // costs fall back to "unknown". The explicit "Refresh prices" button surfaces
    // the error instead.
    let _ = ensure_pricing_fresh(&app).await;
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    build_cost_summary(&conn)
}

/// Force a re-pull of OpenRouter's public pricing into the cache, then return the
/// refreshed summary (the Settings "Refresh prices" action).
#[tauri::command]
pub async fn refresh_pricing(app: AppHandle) -> Result<CostSummary> {
    refresh_pricing_now(&app).await?;
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    build_cost_summary(&conn)
}

/// The recommended model preset for a role ("chat" | "background") — the cost
/// logger's "sensible defaults". The UI pre-fills the model editor with these and the
/// user Saves, so nothing is applied without confirmation (spec §6: swappable).
#[tauri::command]
pub fn recommended_models(role: String) -> Result<Vec<String>> {
    let list = match role.as_str() {
        "chat" => DEFAULT_CHAT_MODELS,
        "background" => DEFAULT_BACKGROUND_MODELS,
        _ => return Err(Error::Other(format!("unknown model role: {role}"))),
    };
    Ok(list.iter().map(|s| s.to_string()).collect())
}

/// Append a `usage_log` row — best-effort: cost logging must never fail a model call,
/// so errors are swallowed. `model = None` is allowed (an unreported served model).
fn log_usage(conn: &Connection, kind: &str, model: Option<&str>, usage: &openrouter::Usage) {
    let _ = conn.execute(
        "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens) VALUES (?1, ?2, ?3, ?4)",
        params![model, kind, usage.prompt_tokens, usage.completion_tokens],
    );
}

/// Write collected background usage rows under one short lock (best-effort), each
/// attributed to its served model, or the requested primary when none was reported.
fn log_background_usage(app: &AppHandle, models: &[String], rows: &[(Option<String>, openrouter::Usage)]) {
    if rows.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let Ok(conn) = state.db.lock() else { return };
    for (served, usage) in rows {
        let model = served.as_deref().or_else(|| models.first().map(String::as_str));
        log_usage(&conn, "background", model, usage);
    }
}

/// Refresh the cached pricing when it's stale (check-on-read). Resolves staleness
/// under a short lock, then does the network fetch + upsert without holding it (rule #4).
async fn ensure_pricing_fresh(app: &AppHandle) -> Result<()> {
    let stale = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        let hours: Option<f64> = conn
            .query_row(
                "SELECT (julianday('now') - julianday(replace(MAX(fetched_at),'Z',''))) * 24.0 FROM model_pricing",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        cost::pricing_is_stale(hours)
    };
    if stale {
        refresh_pricing_now(app).await?;
    }
    Ok(())
}

/// Pull the public OpenRouter catalogue (no key) and upsert every model's prices into
/// the cache. Never holds the DB lock across the network call (rule #4).
async fn refresh_pricing_now(app: &AppHandle) -> Result<()> {
    let models = openrouter::list_models().await?;
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let tx = conn.unchecked_transaction()?;
    for m in &models {
        tx.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now')) \
             ON CONFLICT(model) DO UPDATE SET \
                prompt_price = ?2, completion_price = ?3, fetched_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            params![m.id, m.prompt_price, m.completion_price],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Assemble the cost summary from `usage_log` × the cached `model_pricing`.
fn build_cost_summary(conn: &Connection) -> Result<CostSummary> {
    let last_30d = spend_rows(conn, true)?;
    let all_time = spend_rows(conn, false)?;
    let total_30d_usd = total_cost(&last_30d);
    let total_all_time_usd = total_cost(&all_time);
    let pricing_updated_at: Option<String> = conn
        .query_row("SELECT MAX(fetched_at) FROM model_pricing", [], |r| r.get(0))
        .ok()
        .flatten();
    Ok(CostSummary {
        last_30d,
        all_time,
        total_30d_usd,
        total_all_time_usd,
        pricing_updated_at,
    })
}

/// Per-model token sums + request counts (optionally only the last 30 days), priced
/// from the cache; ordered by request count desc. Rows with a NULL model are excluded.
fn spend_rows(conn: &Connection, last_30d: bool) -> Result<Vec<ModelSpend>> {
    let window = if last_30d {
        "AND u.created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT u.model, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price \
         FROM usage_log u LEFT JOIN model_pricing p ON p.model = u.model \
         WHERE u.model IS NOT NULL {window} \
         GROUP BY u.model \
         ORDER BY COUNT(*) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map([], |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let completion_tokens: i64 = r.get(2)?;
            let prompt_price: Option<f64> = r.get(4)?;
            let completion_price: Option<f64> = r.get(5)?;
            Ok(ModelSpend {
                model: r.get(0)?,
                prompt_tokens,
                completion_tokens,
                request_count: r.get(3)?,
                cost_usd: cost::call_cost(
                    Some(prompt_tokens),
                    Some(completion_tokens),
                    prompt_price,
                    completion_price,
                ),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // Rank by cost (most expensive first); unpriced models (unknown cost) sort last,
    // then by request count — so the breakdown reads as a spend ranking.
    rows.sort_by(|a, b| {
        let ak = a.cost_usd.unwrap_or(f64::NEG_INFINITY);
        let bk = b.cost_usd.unwrap_or(f64::NEG_INFINITY);
        bk.partial_cmp(&ak)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.request_count.cmp(&a.request_count))
    });
    Ok(rows)
}

/// Total spend across rows: `Some(0)` with no usage, `None` when there's usage but no
/// model is priced yet, else the sum of the priced rows (unpriced models shown "—").
fn total_cost(rows: &[ModelSpend]) -> Option<f64> {
    if rows.is_empty() {
        return Some(0.0);
    }
    let known: Vec<f64> = rows.iter().filter_map(|r| r.cost_usd).collect();
    if known.is_empty() {
        return None;
    }
    Some(known.iter().sum())
}

// --- helpers ---

/// Current UTC time in the store's ISO8601 format (matches ingest timestamps).
fn iso_now(state: &AppState) -> Result<String> {
    let conn = state.db.lock().unwrap();
    Ok(conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| r.get(0))?)
}

/// Resolve the user's stored IANA zone to a `chrono_tz::Tz`. Falls back to UTC when
/// the key is unset, empty, or unparseable — chrono `Local` only yields an offset
/// (no IANA name, DST-unstable), so the canonical zone is supplied by the frontend
/// (`Intl`) and stored; UTC is the stable default matching every `strftime('now')`.
/// Infallible by design (worst case UTC) so call sites stay one-liners.
fn resolve_zone(conn: &Connection) -> chrono_tz::Tz {
    use std::str::FromStr;
    db::get_setting(conn, TIME_ZONE_KEY)
        .ok()
        .flatten()
        .and_then(|s| chrono_tz::Tz::from_str(s.trim()).ok())
        .unwrap_or(chrono_tz::Tz::UTC)
}

fn current_model(conn: &Connection) -> Result<String> {
    Ok(db::get_setting(conn, "default_model")?.unwrap_or_else(|| DEFAULT_MODEL.to_string()))
}

/// The ordered model list stored for a role, parsed from its JSON setting. Falls
/// back to the single legacy `default_model` (or `DEFAULT_MODEL`) when unset or
/// empty, so existing installs keep working. Never returns an empty list.
fn models_for(conn: &Connection, key: &str) -> Result<Vec<String>> {
    if let Some(raw) = db::get_setting(conn, key)? {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
            let list: Vec<String> = list
                .into_iter()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect();
            if !list.is_empty() {
                return Ok(list);
            }
        }
    }
    Ok(vec![current_model(conn)?])
}

/// The effective list to send for a role: the full ordered list when auto-switch
/// is on (so OpenRouter can fall through to the next model on a rate-limit/quota
/// error), otherwise just the primary. Never empty.
fn effective_models(conn: &Connection, models_key: &str, auto_key: &str) -> Result<Vec<String>> {
    let models = models_for(conn, models_key)?;
    let auto = db::get_setting(conn, auto_key)?.as_deref() == Some("true");
    if auto {
        Ok(models)
    } else {
        Ok(vec![models
            .into_iter()
            .next()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())])
    }
}

/// Clean (trim, drop empties + over-long ids, de-dup preserving order, cap count)
/// and persist a role's model list as a JSON array. An empty result is stored as
/// `[]`; `models_for` then falls back to the default, so the role always resolves
/// to a usable model. The caps bound a frontend-supplied list that's persisted.
fn save_models(conn: &Connection, key: &str, models: Vec<String>) -> Result<()> {
    const MAX_MODELS: usize = 50;
    const MAX_MODEL_ID_CHARS: usize = 200;
    let mut seen = std::collections::HashSet::new();
    let cleaned: Vec<String> = models
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m.chars().count() <= MAX_MODEL_ID_CHARS)
        .filter(|m| seen.insert(m.clone()))
        .take(MAX_MODELS)
        .collect();
    let json = serde_json::to_string(&cleaned).map_err(|e| Error::Other(e.to_string()))?;
    db::set_setting(conn, key, &json)
}

fn row_to_conversation(row: &rusqlite::Row) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        project: row.get(4)?,
    })
}

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
    // Citations are stored as a JSON array string; tolerate NULL / malformed.
    let citations_raw: Option<String> = row.get(6)?;
    let citations = citations_raw.and_then(|s| serde_json::from_str(&s).ok());
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        model: row.get(4)?,
        created_at: row.get(5)?,
        citations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway encrypted store (also exercises the migration-in-transaction
    /// path in `db::open`).
    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, key).unwrap();
        (dir, conn)
    }

    #[test]
    fn save_models_caps_count_and_id_length_and_dedups() {
        let (_dir, conn) = temp_db();
        let mut models: Vec<String> = (0..100).map(|i| format!("vendor/model-{i}")).collect();
        models.push("vendor/model-0".into()); // duplicate of the first
        models.push("x".repeat(500)); // an absurdly long id
        save_models(&conn, CHAT_MODELS_KEY, models).unwrap();

        let stored = models_for(&conn, CHAT_MODELS_KEY).unwrap();
        assert!(stored.len() <= 50, "model count is capped");
        assert!(stored.iter().all(|m| m.chars().count() <= 200), "over-long id dropped");
        assert_eq!(stored.iter().filter(|m| *m == "vendor/model-0").count(), 1, "de-duped");
    }
}
