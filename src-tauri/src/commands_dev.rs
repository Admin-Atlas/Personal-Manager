// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Developer-mode inspection commands — the STRICTLY READ-ONLY backend for the runtime
//! Developer mode (issue #78). Every command here reads internal state and returns a
//! redacted, allow-listed view of it; none mutates the store or the schema.
//!
//! Security model — the feature ships to ALL users from a PUBLIC repo, so the real risk is
//! outward leakage (a screenshot or a pasted bug report), not on-device viewing:
//!  * Redaction happens HERE, in the backend, so a withheld value never even crosses IPC.
//!  * The raw-table browser uses a fixed `(table → projected columns)` allow-list — never
//!    `SELECT *`, never an interpolated table name — so a column added by a future migration
//!    can never auto-appear; surfacing one is a deliberate edit to [`TABLES`] in this file.
//!  * Free-text/personal columns are truncated or shown only as a length; the `settings`
//!    grab-bag (which also holds the archived `learning_profile` and the UI blobs) shows
//!    key + type + length, with only a small safe-list of operational scalar keys rendered.
//!  * No keychain read and no portable-file (`*.pmrules` / `*.pmindex`) decrypt lives here —
//!    OAuth tokens, feed URLs and the encryption keys are never reachable from this module.

use rusqlite::types::Value;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::sidecar::{SandboxReport, SidecarStatus};
use crate::{db, retrieval, AppState};

/// How a column's value is rendered into the read-only browser. Applied to TEXT values
/// only; numbers/nulls/blobs are formatted directly, so a render can only ever *narrow*
/// what is shown, never widen it.
#[derive(Clone, Copy)]
enum Render {
    /// Short, non-personal value shown as-is (ids, enums, flags, timestamps, counts).
    Plain,
    /// Free text shown as its first `n` chars (a `…` is appended when it was longer).
    Trunc(usize),
    /// Personal/large free text shown only as a length — never its content.
    Len,
}

struct Column {
    name: &'static str,
    render: Render,
}

const fn plain(name: &'static str) -> Column {
    Column {
        name,
        render: Render::Plain,
    }
}
const fn trunc(name: &'static str, n: usize) -> Column {
    Column {
        name,
        render: Render::Trunc(n),
    }
}
const fn len(name: &'static str) -> Column {
    Column {
        name,
        render: Render::Len,
    }
}

struct Table {
    name: &'static str,
    columns: &'static [Column],
}

/// The ONE reviewed allow-list. Each entry is a browsable table and the exact columns
/// (with their redaction) exposed for it. To surface a new table or column, add it HERE —
/// that deliberate edit is the review step the security model relies on. `settings` is
/// intentionally absent: it routes to [`settings_page`], which redacts by default.
const TABLES: &[Table] = &[
    Table {
        name: "documents",
        columns: &[
            plain("id"),
            trunc("title", 80),
            plain("project"),
            plain("entity_id"),
            plain("source_type"),
            plain("source_state"),
            plain("importance"),
            plain("reviewed"),
            trunc("content_hash", 12),
            trunc("external_ref", 60),
            plain("source_id"),
            plain("ingested_at"),
        ],
    },
    Table {
        name: "chunks",
        columns: &[
            plain("id"),
            plain("document_id"),
            plain("ordinal"),
            trunc("heading", 60),
            plain("kind"),
            plain("char_count"),
            trunc("uid", 12),
            plain("parent_id"),
            len("content"),
        ],
    },
    Table {
        name: "entities",
        columns: &[
            plain("id"),
            plain("type"),
            trunc("canonical_name", 80),
            plain("confidence"),
            plain("user_confirmed"),
            plain("created_at"),
            plain("updated_at"),
        ],
    },
    Table {
        name: "entity_aliases",
        columns: &[
            plain("id"),
            plain("entity_id"),
            trunc("alias", 80),
            plain("created_at"),
        ],
    },
    Table {
        name: "preferences",
        columns: &[
            plain("id"),
            plain("scope"),
            plain("entity_id"),
            trunc("condition", 80),
            trunc("value", 80),
            plain("source"),
            plain("confidence"),
            plain("user_confirmed"),
            plain("created_at"),
            plain("updated_at"),
        ],
    },
    Table {
        name: "corrections",
        columns: &[
            plain("id"),
            plain("document_id"),
            plain("field"),
            trunc("before_val", 60),
            trunc("after_val", 60),
            trunc("title", 60),
            plain("pipeline_version"),
            plain("created_at"),
        ],
    },
    Table {
        name: "projects",
        columns: &[
            plain("name"),
            plain("entity_id"),
            plain("deadline"),
            plain("size"),
            trunc("blocked_by", 40),
            trunc("parent", 40),
            plain("created_at"),
            plain("updated_at"),
        ],
    },
    Table {
        name: "conversations",
        columns: &[
            plain("id"),
            trunc("title", 60),
            plain("project"),
            plain("created_at"),
            plain("updated_at"),
        ],
    },
    Table {
        name: "messages",
        columns: &[
            plain("id"),
            plain("conversation_id"),
            plain("role"),
            plain("model"),
            plain("created_at"),
            // Chat bodies are personal and large — length only, never content.
            len("content"),
            len("citations"),
        ],
    },
    Table {
        name: "calendar_events",
        columns: &[
            plain("id"),
            plain("calendar_id"),
            trunc("summary", 60),
            plain("start"),
            plain("end"),
            plain("all_day"),
            // Personal free text — length only.
            len("description"),
            len("location"),
            plain("synced_at"),
        ],
    },
    Table {
        name: "usage_log",
        columns: &[
            plain("id"),
            plain("model"),
            plain("kind"),
            plain("prompt_tokens"),
            plain("completion_tokens"),
            plain("cost_usd"),
            plain("created_at"),
        ],
    },
    Table {
        name: "model_pricing",
        columns: &[
            plain("model"),
            trunc("name", 40),
            plain("prompt_price"),
            plain("completion_price"),
            plain("cache_read_price"),
            plain("context_length"),
            plain("intelligence_index"),
            plain("fetched_at"),
        ],
    },
    Table {
        // Connector accounts (Google Drive today). `folder_ids` is the indexing scope (My Drive +
        // opted-in shared drives/folders) shown plainly; `cursor` is the delta-sync state — a JSON
        // map of opaque changes-feed page tokens, shown LENGTH-ONLY (so you can see a cursor is set
        // and advancing without surfacing the tokens). `last_synced_at` + `state` show sync health.
        name: "connector_sources",
        columns: &[
            plain("id"),
            plain("provider"),
            plain("service"),
            trunc("label", 60),
            plain("account_email"),
            plain("mode"),
            plain("folder_ids"),
            len("cursor"),
            plain("last_synced_at"),
            plain("state"),
            plain("created_at"),
        ],
    },
];

/// Every table whose row count the counts dashboard reports: the browsable tables plus the
/// derived indexes (`chunk_vec`/`chunks_fts`, count-only — their rows are binary/index noise)
/// and `settings`. A count is harmless and answers "is the index populated?" at a glance.
const COUNT_TABLES: &[&str] = &[
    "documents",
    "chunks",
    "chunk_vec",
    "chunks_fts",
    "entities",
    "entity_aliases",
    "preferences",
    "corrections",
    "projects",
    "conversations",
    "messages",
    "calendar_events",
    "connector_sources",
    "usage_log",
    "model_pricing",
    "settings",
];

/// `settings` is a key/value grab-bag that also holds personal blobs — the archived
/// `learning_profile`, the `appearance`/`pinboard` UI blobs, the cached daily briefing — so
/// its values are REDACTED BY DEFAULT. Only these short, operational, non-personal scalar
/// keys are rendered in full; every other key shows type + length only.
const SETTINGS_VALUE_SAFE: &[&str] = &[
    "embedding_model",
    "embedding_dim",
    "reranking_enabled",
    "help_mode",
    "time_zone",
    "app_lock_enabled",
    "preferences_migrated_at",
    "google_last_sync",
    "chat_auto_switch",
    "background_auto_switch",
    "dev_mode",
];

/// The running vault's index-time + runtime facts for the Dev tab's System panel. The app
/// version and sidecar status are composed on the frontend from their existing commands, so
/// this stays a pure read of the store.
#[derive(Serialize)]
pub struct DevSystemInfo {
    pub migration_version: i64,
    pub embedder_id: String,
    pub embedder_label: String,
    pub vector_dim: usize,
    pub reranking_enabled: bool,
    pub retrieval_stamp: Option<crate::retrieval_config::RetrievalConfig>,
}

#[derive(Serialize)]
pub struct DevTableCount {
    pub table: String,
    pub rows: i64,
}

/// One redacted page of a table: the projected column names and the rendered cell values
/// (already redacted — these strings are exactly what the UI may display).
#[derive(Serialize)]
pub struct DevTablePage {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// How many characters of a matched chunk the "Retrieval explain" panel previews. The preview is
/// the chunk's real body text cut to this many *chars* by [`truncate`] (char-boundary-safe, so it
/// can't panic on the multilingual tier) — a walk-up-convenience truncation, NOT a redaction
/// boundary; the boundary is that no full body and no outward path exist.
const PREVIEW_CHARS: usize = 160;

/// One ranked candidate in a "Retrieval explain" run, with every per-stage score. Mirrors
/// [`crate::retrieval::ExplainCandidate`] but with the chunk body replaced by a truncated preview
/// and the reranker score attached. Every value here is safe to display. `Deserialize` so the
/// in-chat diagnostic (card 7H) can take the panel's own explain payload back as read-only context.
#[derive(Serialize, Deserialize)]
pub struct DevRetrievalRow {
    pub final_rank: usize,
    pub chunk_id: i64,
    pub document_id: i64,
    pub title: String,
    pub heading: Option<String>,
    pub preview: String,
    pub vector_rank: Option<usize>,
    pub vector_distance: Option<f32>,
    pub keyword_rank: Option<usize>,
    pub fused_score: f64,
    pub decay_factor: f64,
    pub decayed_score: f64,
    pub reranker_score: Option<f32>,
}

/// The result of a "Retrieval explain" run: the ranked rows plus the engine context needed to read
/// them (embedder, whether reranking is on and actually ran, the RRF constant and half-life).
#[derive(Serialize, Deserialize)]
pub struct DevRetrievalExplain {
    pub embedder_id: String,
    pub embedder_label: String,
    pub reranking_enabled: bool,
    pub reranked: bool,
    pub rrf_k: f64,
    pub half_life_days: f64,
    pub k: usize,
    pub rows: Vec<DevRetrievalRow>,
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Render one cell. Numbers/nulls/blobs are formatted structurally; only TEXT is subject to
/// the column's redaction policy.
fn render_value(value: Value, render: Render) -> String {
    match value {
        Value::Null => String::new(),
        Value::Integer(n) => n.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Blob(b) => format!("<blob {} bytes>", b.len()),
        Value::Text(s) => match render {
            Render::Plain => s,
            Render::Trunc(n) => truncate(&s, n),
            Render::Len => format!("<{} chars>", s.chars().count()),
        },
    }
}

// ---- Pure cores (testable with a bare in-memory connection) -------------------------------

fn system_info(conn: &Connection) -> Result<DevSystemInfo> {
    let migration_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let embedder = db::selected_embedder(conn)?;
    Ok(DevSystemInfo {
        migration_version,
        embedder_id: embedder.id.to_string(),
        embedder_label: embedder.label.to_string(),
        vector_dim: db::vec0_dim(conn)?,
        reranking_enabled: db::reranking_enabled(conn)?,
        retrieval_stamp: db::get_retrieval_stamp(conn)?,
    })
}

fn table_counts(conn: &Connection) -> Result<Vec<DevTableCount>> {
    let mut out = Vec::with_capacity(COUNT_TABLES.len());
    for &t in COUNT_TABLES {
        // `t` is a compile-time constant from our own list — never user input.
        let rows: i64 = conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))?;
        out.push(DevTableCount {
            table: t.to_string(),
            rows,
        });
    }
    Ok(out)
}

/// Browsable table names for the picker (`settings` appended — it browses through the
/// redacting [`settings_page`]).
fn table_names() -> Vec<String> {
    let mut names: Vec<String> = TABLES.iter().map(|t| t.name.to_string()).collect();
    names.push("settings".to_string());
    names
}

fn table_page(conn: &Connection, table: &str, limit: u32, offset: u32) -> Result<DevTablePage> {
    let limit = limit.clamp(1, 200);
    if table == "settings" {
        return settings_page(conn, limit, offset);
    }
    // The allow-list guard: reject anything not declared in TABLES *before* any SQL is built,
    // and only ever interpolate the matched static `spec.name` — never the caller's string.
    let spec = TABLES
        .iter()
        .find(|t| t.name == table)
        .ok_or_else(|| Error::Other(format!("table '{table}' is not inspectable")))?;

    let total: i64 = conn.query_row(&format!("SELECT count(*) FROM {}", spec.name), [], |r| {
        r.get(0)
    })?;
    let col_list = spec
        .columns
        .iter()
        .map(|c| c.name)
        .collect::<Vec<_>>()
        .join(", ");
    // rowid DESC surfaces the newest rows first — the most useful order for debugging.
    let sql = format!(
        "SELECT {col_list} FROM {} ORDER BY rowid DESC LIMIT ?1 OFFSET ?2",
        spec.name
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![limit, offset], |row| {
            let mut cells = Vec::with_capacity(spec.columns.len());
            for (i, col) in spec.columns.iter().enumerate() {
                let v: Value = row.get(i)?;
                cells.push(render_value(v, col.render));
            }
            Ok(cells)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(DevTablePage {
        table: spec.name.to_string(),
        columns: spec.columns.iter().map(|c| c.name.to_string()).collect(),
        rows,
        total,
        limit,
        offset,
    })
}

/// `settings` browser: key + value type + length, with the value shown only for the
/// operational safe-list keys and `<redacted>` for everything else (redact-by-default).
fn settings_page(conn: &Connection, limit: u32, offset: u32) -> Result<DevTablePage> {
    let total: i64 = conn.query_row("SELECT count(*) FROM settings", [], |r| r.get(0))?;
    let mut stmt =
        conn.prepare("SELECT key, value FROM settings ORDER BY key LIMIT ?1 OFFSET ?2")?;
    let rows = stmt
        .query_map(params![limit, offset], |row| {
            let key: String = row.get(0)?;
            let value: String = row.get(1)?;
            let vtype = if serde_json::from_str::<serde_json::Value>(&value).is_ok() {
                "json"
            } else {
                "text"
            };
            let length = value.chars().count().to_string();
            let shown = if SETTINGS_VALUE_SAFE.contains(&key.as_str()) {
                truncate(&value, 120)
            } else {
                "<redacted>".to_string()
            };
            Ok(vec![key, vtype.to_string(), length, shown])
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(DevTablePage {
        table: "settings".to_string(),
        columns: vec![
            "key".to_string(),
            "type".to_string(),
            "length".to_string(),
            "value".to_string(),
        ],
        rows,
        total,
        limit,
        offset,
    })
}

/// The chunk breakdown for ONE document (the in-context Documents inspector). Reuses the `chunks`
/// allow-list projection (so `content` stays length-only) and binds `document_id` as a typed
/// parameter; ordered by `ordinal` (natural reading order), capped at 500 chunks.
fn document_chunks(conn: &Connection, document_id: i64) -> Result<DevTablePage> {
    let spec = TABLES
        .iter()
        .find(|t| t.name == "chunks")
        .expect("the chunks table is declared in TABLES");
    let total: i64 = conn.query_row(
        "SELECT count(*) FROM chunks WHERE document_id = ?1",
        params![document_id],
        |r| r.get(0),
    )?;
    let col_list = spec
        .columns
        .iter()
        .map(|c| c.name)
        .collect::<Vec<_>>()
        .join(", ");
    let sql =
        format!("SELECT {col_list} FROM chunks WHERE document_id = ?1 ORDER BY ordinal LIMIT 500");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![document_id], |row| {
            let mut cells = Vec::with_capacity(spec.columns.len());
            for (i, col) in spec.columns.iter().enumerate() {
                let v: Value = row.get(i)?;
                cells.push(render_value(v, col.render));
            }
            Ok(cells)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(DevTablePage {
        table: "chunks".to_string(),
        columns: spec.columns.iter().map(|c| c.name.to_string()).collect(),
        rows,
        total,
        limit: 500,
        offset: 0,
    })
}

// ---- Command wrappers (thin: acquire the store, call a pure core) --------------------------

/// One-call snapshot of the running vault's index-time + runtime facts (System panel).
#[tauri::command]
pub fn dev_system_info(state: State<'_, AppState>) -> Result<DevSystemInfo> {
    let conn = state.conn()?;
    system_info(&conn)
}

/// Row counts for every inspected table (incl. the derived indexes and `settings`).
#[tauri::command]
pub fn dev_table_counts(state: State<'_, AppState>) -> Result<Vec<DevTableCount>> {
    let conn = state.conn()?;
    table_counts(&conn)
}

/// The browsable table names for the Dev tab's table picker.
#[tauri::command]
pub fn dev_table_list() -> Vec<String> {
    table_names()
}

/// A redacted page of one allow-listed table. Rejects any table not in the allow-list.
#[tauri::command]
pub fn dev_table_rows(
    state: State<'_, AppState>,
    table: String,
    limit: u32,
    offset: u32,
) -> Result<DevTablePage> {
    let conn = state.conn()?;
    table_page(&conn, &table, limit, offset)
}

/// The chunk breakdown for one document — the in-context Documents inspector (issue #78, PR 2).
#[tauri::command]
pub fn dev_document_chunks(state: State<'_, AppState>, document_id: i64) -> Result<DevTablePage> {
    let conn = state.conn()?;
    document_chunks(&conn, document_id)
}

/// The untrusted-file worker's OS-confinement state for the Dev tab's Sandbox panel (issue #286). A
/// plain read of the last spawn's outcome — no store access, no mutation. Always registered (a harmless
/// read); the UI is gated by the runtime `devMode`. Off Windows it reports `Unsupported`.
#[tauri::command]
pub fn dev_sidecar_sandbox_report(state: State<'_, AppState>) -> SandboxReport {
    state.sidecar.sandbox_report()
}

/// The worker's answer to the network-block self-test (issue #286): whether the OS refused a direct
/// outbound socket AND out-of-process DNS resolution (the macOS mDNSResponder exfil path, finding #1),
/// each with a human detail. Mirrors the sidecar's `net_selftest` result. The `dns_*` fields default so
/// an older worker reply (socket-only) still parses.
#[cfg(debug_assertions)]
#[derive(Serialize, Deserialize)]
pub struct NetSelftest {
    pub blocked: bool,
    pub detail: String,
    pub errno: Option<i64>,
    #[serde(default)]
    pub dns_blocked: bool,
    #[serde(default)]
    pub dns_detail: String,
}

/// Dev-only (debug builds): ask the running worker to attempt ONE outbound socket and report whether it
/// was refused — the live proof the confinement denies network (issue #286). Compiled OUT of release
/// (like `dev_apply_change_event`), so the worker never attempts a socket in a shipped build; the UI
/// calls it only behind `isDevBuild`. Requires the engine ready (it spawns the worker to ask it), and
/// never touches the store. The socket targets a reserved TEST-NET address, so an unconfined attempt is
/// egress-safe.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_sidecar_net_selftest(app: AppHandle) -> Result<NetSelftest> {
    tokio::task::spawn_blocking(move || -> Result<NetSelftest> {
        let state = app.state::<AppState>();
        if !matches!(state.sidecar.status(), SidecarStatus::Ready) {
            return Err(Error::Other(
                "the document engine isn't ready yet — finish setup first".into(),
            ));
        }
        let raw = state.sidecar.net_selftest()?;
        serde_json::from_value(raw)
            .map_err(|e| Error::Other(format!("could not parse net_selftest result: {e}")))
    })
    .await
    .map_err(|e| Error::Other(format!("net self-test task panicked: {e}")))?
}

/// Read-only "Retrieval explain" (issue #81): run `query` through the live hybrid retriever and
/// return each candidate chunk's per-stage scores — vector distance, keyword rank, RRF fused
/// score, recency decay, and the reranker score (when reranking is on). A parallel, instrumented
/// read; the production chat/search path is untouched. Embeds via the sidecar exactly as chat does
/// and never holds the DB lock across a sidecar call (AGENTS rule #4). Strictly read-only — chunk
/// bodies are previewed (truncated), never returned in full; no secret or outward path exists.
#[tauri::command]
pub async fn dev_retrieval_explain(
    app: AppHandle,
    query: String,
    project: Option<String>,
    k: Option<usize>,
) -> Result<DevRetrievalExplain> {
    tokio::task::spawn_blocking(move || -> Result<DevRetrievalExplain> {
        let state = app.state::<AppState>();
        let k = k.unwrap_or(retrieval::DEFAULT_TOP_K);
        run_retrieval_explain(&state, &query, project.as_deref(), k)
    })
    .await
    .map_err(|e| Error::Other(format!("retrieval explain task panicked: {e}")))?
}

/// Run one "Retrieval explain": embed `query`, fuse + recency-decay the candidates with per-stage
/// scores, and (when reranking is on) re-score them off the DB lock — returning the ranked rows plus
/// the engine context. Blocking (embeds + optionally reranks via the sidecar); callers wrap it in
/// `spawn_blocking`. Shared by the Developer-mode panel and the in-chat panel (card 7H) so both read
/// the exact same instrumented pipeline; `k` is clamped to the retrieval-depth bounds here.
pub(crate) fn run_retrieval_explain(
    state: &AppState,
    query: &str,
    project: Option<&str>,
    k: usize,
) -> Result<DevRetrievalExplain> {
    let k = k.clamp(db::RETRIEVAL_K_MIN, db::RETRIEVAL_K_MAX);

    // Don't trigger a slow first-run install mid-inspection — require the engine ready.
    if !matches!(state.sidecar.status(), SidecarStatus::Ready) {
        return Err(Error::Other(
            "the document engine isn't ready yet — finish setup first".into(),
        ));
    }

    // Resolve the vault's models + reranking toggle + embedder identity in one short lock, then
    // drop it so neither the embed nor the rerank holds the DB lock across a sidecar call (#4).
    let (gateway, rerank_on, embedder) = {
        let conn = state.conn()?;
        (
            state.gateway(&conn)?,
            db::reranking_enabled(&conn)?,
            db::selected_embedder(&conn)?,
        )
    };

    let query_owned = query.to_string();
    let embeddings = gateway.embed_query(std::slice::from_ref(&query_owned))?;
    let Some(query_vec) = embeddings.into_iter().next() else {
        return Err(Error::Other("failed to embed the query".into()));
    };

    // Fused + recency-decayed candidates with per-stage scores, under one short lock. Pass the
    // vault's multilingual flag so the panel's keyword branch segments CJK exactly as production
    // does (F-33) — otherwise the diagnostic would show hits the real chat path wouldn't.
    let candidates = {
        let conn = state.conn()?;
        retrieval::explain(&conn, query, &query_vec, k, project, embedder.multilingual)?
    };

    // Off-lock reranking: capture each candidate's score and reorder, mirroring production but
    // keeping the scores. A `None`/mismatched result leaves the fused order (`reranked=false`).
    let mut scored: Vec<(retrieval::ExplainCandidate, Option<f32>)> =
        candidates.into_iter().map(|c| (c, None)).collect();
    let mut reranked = false;
    if rerank_on {
        // Same rerank input as production (title + heading breadcrumb, not bare body) so the panel
        // reflects the real reranked order — see `retrieval::rerank_text`.
        let texts: Vec<String> = scored
            .iter()
            .map(|(c, _)| retrieval::rerank_text(&c.chunk))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let reranker = &gateway as &dyn retrieval::Reranker;
        if let Some(scores) = reranker.scores(query, &refs)? {
            if scores.len() == scored.len() {
                for ((_, slot), s) in scored.iter_mut().zip(scores.iter()) {
                    *slot = Some(*s);
                }
                scored.sort_by(|a, b| {
                    b.1.unwrap_or(f32::MIN)
                        .partial_cmp(&a.1.unwrap_or(f32::MIN))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.0.chunk.chunk_id.cmp(&b.0.chunk.chunk_id))
                });
                reranked = true;
            }
        }
    }

    // Mirror production's final selection: from the reranked (or fused, if reranking is off) pool,
    // apply the SAME per-section cap + top-k truncation the chat path grounds on, so the panel shows
    // exactly the chunks the model receives, in the same order (`retrieval::select_top_k`). The wider
    // pool's rejected candidates stay inspectable by raising the depth slider.
    use std::collections::HashMap;
    let selected =
        retrieval::select_top_k(scored.iter().map(|(c, _)| c.chunk.clone()).collect(), k);
    let mut by_id: HashMap<i64, (retrieval::ExplainCandidate, Option<f32>)> = scored
        .into_iter()
        .map(|(c, s)| (c.chunk.chunk_id, (c, s)))
        .collect();
    let rows = selected
        .into_iter()
        .enumerate()
        .filter_map(|(i, chunk)| {
            by_id
                .remove(&chunk.chunk_id)
                .map(|(c, reranker_score)| DevRetrievalRow {
                    final_rank: i,
                    chunk_id: c.chunk.chunk_id,
                    document_id: c.chunk.document_id,
                    title: c.chunk.title,
                    heading: c.chunk.heading,
                    preview: truncate(&c.chunk.content, PREVIEW_CHARS),
                    vector_rank: c.vector_rank,
                    vector_distance: c.vector_distance,
                    keyword_rank: c.keyword_rank,
                    fused_score: c.fused_score,
                    decay_factor: c.decay_factor,
                    decayed_score: c.decayed_score,
                    reranker_score,
                })
        })
        .collect();

    Ok(DevRetrievalExplain {
        embedder_id: embedder.id.to_string(),
        embedder_label: embedder.label.to_string(),
        reranking_enabled: rerank_on,
        reranked,
        rrf_k: retrieval::RRF_K,
        half_life_days: retrieval::HALF_LIFE_DAYS,
        k,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    // A bare in-memory store with just the two tables the guards need — no vec0/FTS5/migrations
    // required, so the security guards are tested in isolation.
    fn fixture() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE entities (
                 id INTEGER PRIMARY KEY, type TEXT, canonical_name TEXT,
                 confidence REAL, user_confirmed INTEGER, created_at TEXT, updated_at TEXT
             );
             INSERT INTO entities(type,canonical_name,confidence,user_confirmed,created_at,updated_at)
                 VALUES ('project','Personal Manager',1.0,1,'t0','t0');
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings VALUES ('learning_profile','a distilled personal profile (synthetic)');
             INSERT INTO settings VALUES ('embedding_model','BAAI/bge-small-en-v1.5');",
        )
        .unwrap();
        c
    }

    #[test]
    fn rejects_a_table_not_in_the_allow_list() {
        let c = fixture();
        // A real table that exists in the store but is NOT declared inspectable.
        assert!(table_page(&c, "sqlite_master", 50, 0).is_err());
        assert!(table_page(&c, "secrets", 50, 0).is_err());
    }

    #[test]
    fn an_allowed_table_returns_only_its_projected_columns() {
        let c = fixture();
        let page = table_page(&c, "entities", 50, 0).unwrap();
        assert_eq!(
            page.columns,
            vec![
                "id",
                "type",
                "canonical_name",
                "confidence",
                "user_confirmed",
                "created_at",
                "updated_at"
            ]
        );
        assert_eq!(page.total, 1);
        assert_eq!(page.rows.len(), 1);
        // canonical_name is shown (short, under the truncation limit).
        assert_eq!(page.rows[0][2], "Personal Manager");
    }

    #[test]
    fn settings_redacts_unlisted_keys_and_never_returns_their_value() {
        let c = fixture();
        let page = table_page(&c, "settings", 50, 0).unwrap();
        assert_eq!(page.columns, vec!["key", "type", "length", "value"]);
        let learning = page
            .rows
            .iter()
            .find(|r| r[0] == "learning_profile")
            .unwrap();
        // The personal blob's value is never returned — only its shape.
        assert_eq!(learning[3], "<redacted>");
        assert_ne!(learning[2], "0"); // length is still reported
        let embed = page
            .rows
            .iter()
            .find(|r| r[0] == "embedding_model")
            .unwrap();
        // The operational safe-list key is shown in full.
        assert_eq!(embed[3], "BAAI/bge-small-en-v1.5");
    }

    #[test]
    fn truncate_marks_overflow_and_counts_chars() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
    }

    #[test]
    fn truncate_is_char_safe_on_multibyte() {
        // The multilingual embedder tier can produce multi-byte text; the preview cap counts CHARS
        // and must never slice a UTF-8 boundary (a naive byte slice would panic). "日本語テキスト"
        // is 7 chars / 21 bytes; "café" is 4 chars / 5 bytes.
        assert_eq!(truncate("日本語テキスト", 3), "日本語…");
        // A short multi-byte string is returned whole — the cap is a ceiling, not a cut length.
        assert_eq!(truncate("café", 10), "café");
    }

    #[test]
    fn document_chunks_scopes_to_one_document_and_hides_content() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE chunks (
                 id INTEGER PRIMARY KEY, document_id INTEGER, ordinal INTEGER, heading TEXT,
                 kind TEXT, char_count INTEGER, uid TEXT, parent_id INTEGER, content TEXT
             );
             INSERT INTO chunks(document_id,ordinal,heading,kind,char_count,uid,parent_id,content)
                 VALUES (1,0,'Intro','leaf',120,'uid-a',NULL,'the full secret body of chunk one'),
                        (1,1,'Body','leaf',90,'uid-b',NULL,'second chunk body'),
                        (2,0,'Other','leaf',50,'uid-c',NULL,'a different document');",
        )
        .unwrap();
        let page = document_chunks(&c, 1).unwrap();
        // Scoped to document 1 only (document 2's chunk is excluded).
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 2);
        // `content` is shown as a length, never the body text (the chunks projection's Render::Len).
        let idx = page.columns.iter().position(|c| c == "content").unwrap();
        assert!(page.rows[0][idx].ends_with("chars>"));
        assert!(!page.rows[0][idx].contains("secret body"));
    }
}
