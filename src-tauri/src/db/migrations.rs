// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::Connection;

use crate::error::Result;

/// Schema migrations, applied in order. Each entry bumps `PRAGMA user_version`
/// by one. Migrations are additive — never destructive — so app updates never
/// wipe the store (spec §7).
const MIGRATIONS: &[&str] = &[
    // v1: conversations, messages, settings.
    r#"
    CREATE TABLE conversations (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        title       TEXT NOT NULL DEFAULT 'New conversation',
        created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );

    CREATE TABLE messages (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        role            TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
        content         TEXT NOT NULL,
        model           TEXT,
        created_at      TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX idx_messages_conversation ON messages(conversation_id, id);

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
    // v2: the Archivist store — documents, their chunks, and the search indexes.
    // `chunk_vec` (sqlite-vec) and `chunks_fts` (FTS5) are derived indexes whose
    // rowid mirrors `chunks.id`, so a retrieval hit maps straight back to a chunk
    // and both are clearable with a plain DELETE on rebuild (spec §3). A regular
    // (not external-content) FTS5 table is used precisely so `DELETE FROM` works.
    // The vector dimension is fixed at 384 to match the embedding model
    // (bge-small-en-v1.5); changing the model forces a re-index, so the model
    // identity is pinned in `settings` (embedding_model / embedding_dim).
    r#"
    CREATE TABLE documents (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        source_path  TEXT,
        vault_path   TEXT NOT NULL UNIQUE,
        title        TEXT NOT NULL DEFAULT 'Untitled',
        content_hash TEXT NOT NULL UNIQUE,
        ext          TEXT,
        byte_size    INTEGER,
        created_at   TEXT,
        ingested_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        status       TEXT NOT NULL DEFAULT 'ingested'
    );

    CREATE TABLE chunks (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        ordinal     INTEGER NOT NULL,
        heading     TEXT,
        content     TEXT NOT NULL,
        char_count  INTEGER NOT NULL
    );
    CREATE INDEX idx_chunks_document ON chunks(document_id, ordinal);

    CREATE VIRTUAL TABLE chunk_vec USING vec0(embedding float[384]);

    CREATE VIRTUAL TABLE chunks_fts USING fts5(content);

    -- Pin the embedding identity. Changing the model forces a re-index, so we
    -- record it once and the vector column dimension above must match.
    INSERT OR IGNORE INTO settings(key, value) VALUES ('embedding_model', 'BAAI/bge-small-en-v1.5');
    INSERT OR IGNORE INTO settings(key, value) VALUES ('embedding_dim', '384');
    "#,
    // v3: record which source documents an assistant answer cited, as a JSON
    // array, so the "which files did this draw from" provenance survives a
    // reload (spec §8.3). Additive and nullable — older messages stay NULL.
    r#"
    ALTER TABLE messages ADD COLUMN citations TEXT;
    "#,
    // v4: the organisation + learning-capture substrate (spec §8.4, §3, §4.5).
    // Documents gain a project label, tags (JSON array), an importance level, a
    // `reviewed` flag (the sorting-review queue is `reviewed = 0`), and a
    // `last_activity` timestamp that drives retrieval recency decay. The
    // `corrections` table logs every change the user makes to a proposed value —
    // the raw material the Learning-You profile is distilled from. `project` is a
    // free-form label (not a `projects` table): the project *hierarchy* the
    // focus view needs is Step 5, so a dedicated table would be speculative now.
    // All additive — older rows take the column defaults (rule #3).
    r#"
    ALTER TABLE documents ADD COLUMN project       TEXT NOT NULL DEFAULT 'Unsorted';
    ALTER TABLE documents ADD COLUMN tags          TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE documents ADD COLUMN importance    TEXT DEFAULT NULL
        CHECK (importance IN ('high','medium','low') OR importance IS NULL);
    ALTER TABLE documents ADD COLUMN reviewed      INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE documents ADD COLUMN last_activity TEXT;

    CREATE TABLE corrections (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        document_id INTEGER REFERENCES documents(id) ON DELETE SET NULL,
        field       TEXT NOT NULL CHECK (field IN ('project','tags','importance')),
        before_val  TEXT,
        after_val   TEXT,
        title       TEXT,
        context     TEXT,
        created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_corrections_created ON corrections(created_at);
    "#,
    // v5: the Personal Assistant surfaces (spec §8.5, §4). Projects gain triage
    // metadata so the focus view can show one honest status per project: a manual
    // `deadline` (bridges "Due soon" until the calendar lands in Step 6), a `size`
    // estimate ("quick" → Quick win), a `blocked_by` link (→ Blocked), and a
    // `parent` (→ Part of …). The table is keyed by the free-form project *name*
    // that documents already store, so no `project_id` retrofit is needed; rows are
    // created lazily the first time the user (or an AI proposal) sets an attribute.
    // A document's project with no row here simply has all-null metadata. The
    // `conversations.project` column scopes a chat to one project's documents
    // (retrieval filters on it). All additive — older rows are untouched (rule #3).
    r#"
    CREATE TABLE projects (
        name       TEXT PRIMARY KEY,
        deadline   TEXT,
        size       TEXT CHECK (size IN ('quick','standard','large') OR size IS NULL),
        blocked_by TEXT,
        parent     TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );

    ALTER TABLE conversations ADD COLUMN project TEXT;
    "#,
    // v6: the read-only Google Calendar mirror (spec §8.6, §4.1). `calendar_events`
    // is a *derived* table — refilled from Google on each sync, cleared on
    // disconnect — so it's never a source of truth (Markdown/keychain remain
    // authoritative) and reconstructable from the API, in the spirit of the FTS/vec
    // indexes. It feeds the focus view's "Due soon" status (an upcoming event whose
    // title names a project), the on-screen agenda, and chat ("you have X at 3pm").
    // Which calendars are synced and the last-sync time are plain `settings` keys
    // (`google_calendar_ids` JSON / `google_last_sync`), so no schema is needed for
    // them. Additive — older stores just have an empty mirror (rule #3).
    r#"
    CREATE TABLE calendar_events (
        id          TEXT PRIMARY KEY,   -- "<calendar_id>:<event_id>" (unique across calendars)
        calendar_id TEXT NOT NULL,
        summary     TEXT NOT NULL DEFAULT '(no title)',
        description TEXT,
        location    TEXT,
        start       TEXT NOT NULL,      -- ISO datetime, or a date for all-day events
        end         TEXT,
        all_day     INTEGER NOT NULL DEFAULT 0,
        html_link   TEXT,
        synced_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_calendar_events_start ON calendar_events(start);
    "#,
    // v7: the Cost Logger (spec §11.2 / §17.1). `usage_log` records token usage per
    // model call (chat vs background), append-only, so spend can be attributed to the
    // model that actually ran — never read as a source of truth. `model_pricing`
    // caches OpenRouter's public price list (fetched ~once a day over plain HTTP, no
    // model call), so spend is a plain usage × pricing join. Both are derived /
    // regenerable and additive — older stores just start with empty tables (rule #3).
    r#"
    CREATE TABLE usage_log (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        model             TEXT NOT NULL,
        kind              TEXT NOT NULL CHECK (kind IN ('chat','background')),
        prompt_tokens     INTEGER,
        completion_tokens INTEGER,
        created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_usage_log_model_created ON usage_log(model, created_at);

    CREATE TABLE model_pricing (
        model            TEXT PRIMARY KEY,
        prompt_price     REAL,
        completion_price REAL,
        fetched_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    "#,
];

pub fn run(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let mut version = current as usize;
    while version < MIGRATIONS.len() {
        // Each step is atomic: the DDL and its `user_version` bump commit together,
        // so a mid-batch failure rolls the whole step back rather than leaving
        // half-applied schema that bricks the next startup. (Migrations stay
        // additive — rule #3 — so a rollback only loses the failed step's changes.)
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATIONS[version])?;
        tx.pragma_update(None, "user_version", (version + 1) as i64)?;
        tx.commit()?;
        version += 1;
    }
    Ok(())
}
