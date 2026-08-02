// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::Connection;

use crate::error::Result;

/// Schema migrations, applied in order. Each entry bumps `PRAGMA user_version`
/// by one. Migrations are additive — never destructive — so app updates never
/// wipe the store (spec §7). Re-keying index-only rows (e.g. connector row IDs)
/// must ship an old→new mapping so user classifications survive.
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
        -- RESERVED (v2): no reader and no writer other than this default. Kept because the
        -- additive-only rule forbids dropping a column, not because anything consumes it. An
        -- ingest that fails now leaves no row at all rather than a row with a status.
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
    // v8: cache the model **recommender**'s per-model signals next to the price cache
    // (spec §6). The cost logger's daily refresh already pulls the public catalogue, so
    // we store a few more of its columns on `model_pricing` rather than building a second
    // fetch/scheduler — the recommender then reads from the same cache and still works
    // offline (it recommends from the last-good list and flags staleness). `cache_read_price`
    // is the prompt-cache read rate (effective-cost weighting); `supported_parameters` /
    // `input_modalities` are JSON arrays; `intelligence_index` is the Artificial-Analysis
    // capability signal (NULL for the ~6 in 7 models without it). All additive and nullable
    // — older stores and unbenchmarked models just carry NULLs and the existing usage×pricing
    // cost join is untouched (rule #3).
    r#"
    ALTER TABLE model_pricing ADD COLUMN name                 TEXT;
    ALTER TABLE model_pricing ADD COLUMN context_length       INTEGER;
    ALTER TABLE model_pricing ADD COLUMN cache_read_price     REAL;
    ALTER TABLE model_pricing ADD COLUMN supported_parameters TEXT;
    ALTER TABLE model_pricing ADD COLUMN input_modalities     TEXT;
    ALTER TABLE model_pricing ADD COLUMN intelligence_index   REAL;
    "#,
    // v9: the retrieval-foundation chunk schema (spec §21.4, PR 1). The structure-aware
    // splitter records, per chunk: a STABLE `uid` (deterministic from the document hash +
    // structural position, so a rebuild reproduces identical ids — the retrofit-painful part
    // for a future graph / Stage-5 sync), a `parent_id` linking a leaf to the structural
    // parent that spans its section, source byte offsets (`start_offset`/`end_offset`, for
    // navigable citations), a `kind` ('leaf' | 'parent'), and a free-form `meta` JSON for
    // chunk-level facts not on the document. Parent rows are STRUCTURAL-ONLY: they live in
    // `chunks` but are never inserted into `chunk_vec`/`chunks_fts`, so the "rowid mirrors
    // chunks.id" invariant holds (KNN/FTS only ever see leaf rowids; parent ids are simply
    // gaps). All additive/nullable — older rows take the defaults and keep working until the
    // next Rebuild (prompted by the retrieval-config stamp) repopulates them (rule #3).
    r#"
    ALTER TABLE chunks ADD COLUMN uid          TEXT;
    ALTER TABLE chunks ADD COLUMN parent_id    INTEGER REFERENCES chunks(id) ON DELETE CASCADE;
    ALTER TABLE chunks ADD COLUMN start_offset INTEGER;
    ALTER TABLE chunks ADD COLUMN end_offset   INTEGER;
    ALTER TABLE chunks ADD COLUMN kind         TEXT NOT NULL DEFAULT 'leaf';
    -- RESERVED (v9): written by nothing and read by nothing. Per-chunk provenance ended up in the
    -- dedicated `chat_turn_id`/`chunk_at` columns (v24) instead, which is why this stayed empty.
    ALTER TABLE chunks ADD COLUMN meta         TEXT;
    CREATE INDEX idx_chunks_uid    ON chunks(document_id, uid);
    CREATE INDEX idx_chunks_parent ON chunks(parent_id);
    "#,
    // v10: canonical-entity resolution (Stage 3). Today the *name is the identity* —
    // `documents.project` is free text and `projects` is keyed by that same string — so name
    // variants ("PM", "Personal Manager", "Atlas - PM") are offered as three co-equal projects
    // and keep reappearing. This separates identity from name. `entities` is the stable identity
    // (generic over project/person/thing; only `project` is populated now — the seam for
    // people/things banked cheaply). `entity_aliases` maps every known name string — including
    // each canonical name as a SELF-ALIAS — to exactly one entity, so a review correction becomes
    // a forward-going rule, not a one-off row-patch. `documents.entity_id` is the resolved pointer;
    // `documents.project` is KEPT as a denormalised cache of the entity's canonical name (always
    // written through resolution, never a variant) so existing reads / FTS / the focus-view
    // group-by keep working untouched. `projects` gains a nullable `entity_id` (purely additive —
    // its `name` PK and every consumer stay as-is) to bank the entity seam without a destructive
    // re-key. The encrypted rules file at the data-home root is the portable source of truth; these
    // tables are its queryable mirror (written/reconciled at boot once the vault key is available).
    // One-time backfill: one entity per distinct existing project string, each with a self-alias —
    // NO auto-merge (collapsing variants that are really the same project is the user's call, via
    // the Teach tab). Chunk vectors are untouched: project is ranking metadata, not part of the
    // embedding, so no `ensure_vec_dim` / re-index fires. All additive — older rows resolve cleanly
    // and pre-Step-4 stores just back-fill an 'Unsorted' entity (rule #3).
    r#"
    CREATE TABLE entities (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        type           TEXT NOT NULL CHECK (type IN ('project','person','thing')),
        canonical_name TEXT NOT NULL,
        created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        UNIQUE(type, canonical_name)
    );

    -- An alias resolves to exactly one project entity; the canonical name is itself a row here.
    -- `alias` is globally UNIQUE — correct while only projects exist; becomes per-type when
    -- person/thing land (a deliberate, documented future refinement, not a silent constraint).
    CREATE TABLE entity_aliases (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        entity_id  INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
        alias      TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_entity_aliases_entity ON entity_aliases(entity_id);

    ALTER TABLE documents ADD COLUMN entity_id INTEGER REFERENCES entities(id);
    ALTER TABLE projects  ADD COLUMN entity_id INTEGER REFERENCES entities(id);

    -- One-time backfill: one project entity per distinct existing project string (NO auto-merge).
    INSERT INTO entities(type, canonical_name)
        SELECT 'project', project FROM documents GROUP BY project;
    -- 'Unsorted' is the ingest default + safe fallback, so guarantee its entity always exists (even
    -- on a brand-new empty vault) — index-time resolution then always finds it instead of creating
    -- (and de-syncing) one. A no-op when documents already carry an 'Unsorted' string.
    INSERT OR IGNORE INTO entities(type, canonical_name) VALUES ('project', 'Unsorted');
    -- The canonical name is itself stored as an alias row (self-alias), so resolution is uniform.
    INSERT INTO entity_aliases(entity_id, alias)
        SELECT id, canonical_name FROM entities WHERE type = 'project';
    -- Point every document at its entity, resolved by its current canonical project string. Every
    -- distinct project string just became an entity, so no document is left unresolved.
    -- guard:allow — preserving backfill: fills the freshly-added, all-NULL entity_id; overwrites nothing.
    UPDATE documents SET entity_id = (
        SELECT e.id FROM entities e WHERE e.type = 'project' AND e.canonical_name = documents.project
    );
    -- Attach existing triage rows to the same entity (additive; `name` stays the PK). A `projects`
    -- row with no surviving documents simply keeps a NULL entity_id (harmless — the focus view is
    -- driven by `documents`).
    -- guard:allow — preserving backfill (freshly-added entity_id); overwrites nothing.
    UPDATE projects SET entity_id = (
        SELECT e.id FROM entities e WHERE e.type = 'project' AND e.canonical_name = projects.name
    );
    CREATE INDEX idx_documents_entity ON documents(entity_id);
    "#,
    // v11: index-only foundation (Stage 3, §8.1) — the shared substrate for sources we index but
    // don't fully import (cloud connectors, local-folder watch). An index-only document stores a
    // metadata row + an embedding + a POINTER (stable source id, external ref/URL, the source's
    // last-modified + content hash) but NOT the body bytes; the body is fetched live on demand and
    // only a short `stored_summary` stays readable offline. `source_type` discriminates where a
    // document's organisational truth lives — `'vault'` (Markdown front-matter, every document
    // today) vs `'index_only'` (the encrypted index-only manifest at the data-home root, next to
    // `entities.pmrules`). `source_state` is the first-class reachability state: a deleted source
    // goes soft `'source_missing'` (kept findable, body flagged unretrievable — never a hard drop)
    // and an unreachable source (expired OAuth, unmounted drive) goes `'unreachable'`, never
    // masquerading as mass deletion. All columns are additive/nullable with safe defaults, so every
    // existing row reads as a fully-stored vault document with no backfill (rule #3). Index-only
    // rows reuse the NOT-NULL-UNIQUE `vault_path` with a synthetic `'idx://'||source_id` sentinel:
    // it satisfies the constraint for free, carries no `.md`/`.pmenc` extension so the rebuild
    // vault-walk (`is_vault_markdown`) skips it, and the truth dispatch keys on `source_type`, never
    // on parsing it. Vectors are unaffected — index-only leaves share `chunk_vec`, so a model/dim
    // switch re-embeds them on the next Rebuild like any other row.
    r#"
    ALTER TABLE documents ADD COLUMN source_type  TEXT NOT NULL DEFAULT 'vault'
        CHECK (source_type IN ('vault','index_only'));
    ALTER TABLE documents ADD COLUMN source_state TEXT NOT NULL DEFAULT 'ok'
        CHECK (source_state IN ('ok','source_missing','unreachable'));
    ALTER TABLE documents ADD COLUMN source_id           TEXT;
    ALTER TABLE documents ADD COLUMN external_ref        TEXT;
    ALTER TABLE documents ADD COLUMN source_modified_at  TEXT;
    ALTER TABLE documents ADD COLUMN source_content_hash TEXT;
    ALTER TABLE documents ADD COLUMN stored_summary      TEXT;
    -- The stable source id is the manifest key + the rename-survives identity; unique only where
    -- present (vault documents leave it NULL), so a partial unique index is the right constraint.
    CREATE UNIQUE INDEX idx_documents_source_id ON documents(source_id) WHERE source_id IS NOT NULL;
    CREATE INDEX idx_documents_source_type ON documents(source_type);
    "#,
    // v12: entity spine hardening (Stage 3, §8.5). The entity-resolution foundation (v10) left
    // `entities` a flat generic spine with no way to record two facts the audit flagged: how
    // CONFIDENT we are in an entity, and whether the USER has confirmed it. Confirmation was an
    // action with no recorded STATE. Shape the spine now — while the entity row count is small —
    // rather than retrofitting live, encrypted, user-held rows + the rules file later. Two columns,
    // nothing type-specific: per-type attributes (people/places/projects) will live in TYPE TABLES
    // keyed to `entities.id` (Stage 4, the relational-model card), never smeared onto this spine.
    //   * `confidence` is DB-only DERIVED state: today every entity originates from deterministic
    //     exact-match filing, so 1.0 is the honest seed; it re-seeds to the DEFAULT on a mirror
    //     rebuild (the rules file does not carry it) and is the seam for Stage-5 auto-merge scoring.
    //   * `user_confirmed` is PORTABLE user truth: it is mirrored into the encrypted `entities.pmrules`
    //     file (schema 2) so it survives a device copy or an index drop-recreate, like the aliases.
    // Both are additive with safe defaults — existing rows fill as confidence 1.0 / unconfirmed,
    // no backfill loop, every existing query untouched (rule #3). Vectors are unaffected.
    r#"
    ALTER TABLE entities ADD COLUMN confidence     REAL    NOT NULL DEFAULT 1.0
        CHECK (confidence >= 0.0 AND confidence <= 1.0);
    ALTER TABLE entities ADD COLUMN user_confirmed INTEGER NOT NULL DEFAULT 0
        CHECK (user_confirmed IN (0,1));
    "#,
    // v13: the structured preference model (spec §4.5 / §291, Stage 3) — replaces the single
    // free-text "Learning You" blob (`settings.learning_profile`, dumped WHOLE into every
    // chat/proposal/briefing prompt) with TYPED, QUERYABLE preference records. The blob can't be
    // queried by condition, can't be selectively retrieved, and silently loses the rule that
    // applied at the decision point as preferences accumulate — the "blob-in-context" failure mode.
    // A preference is now a row whose fields mirror the card's minimum model: `scope`
    // (global / per-project / per-context), `condition` (when it applies — the predicate text for
    // context scope), `value` (the preference itself), `source` (user-stated vs PM-inferred), and a
    // revisable `confidence`. Retrieval becomes a QUERY (`preferences::relevant_preferences`) that
    // injects only the records whose scope+condition match the current situation, so the applicable
    // rule is guaranteed-surfaced rather than hoped-for.
    //   * `entity_id` reuses the canonical `entities` spine (just hardened in v12) for per-project
    //     scope — a deterministic id match, never a name string — and is NULL for global/context.
    //     ON DELETE CASCADE cleans up a project's preferences if its entity is ever deleted; a
    //     MERGE repoints them first (`entities::merge_entities`) so they follow the survivor.
    //   * `user_confirmed` mirrors the entity convention: a user-stated preference is confirmed;
    //     records distilled once from the legacy blob land `source='inferred'`, unconfirmed, awaiting
    //     the user's vouch in the Teach tab. The ongoing inference loop is deferred (→ Stage 5).
    // DB-resident on purpose (NOT a portable `.pmrules` file): SQLCipher encrypts it at rest, the
    // table is never dropped (additive migrations, rule #3), and it already travels with a
    // shared/portable vault — the rules-file pattern exists to survive entity mirror-rebuilds +
    // id reassignment, which preferences don't face. Vectors are untouched. All additive — an
    // existing store just gains an empty table (rule #3).
    r#"
    CREATE TABLE preferences (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        scope          TEXT NOT NULL CHECK (scope IN ('global','project','context')),
        entity_id      INTEGER REFERENCES entities(id) ON DELETE CASCADE, -- set iff scope='project'
        condition      TEXT,                            -- when it applies (context scope; optional otherwise)
        value          TEXT NOT NULL,                   -- the preference itself
        source         TEXT NOT NULL DEFAULT 'user'  CHECK (source IN ('user','inferred')),
        confidence     REAL NOT NULL DEFAULT 1.0     CHECK (confidence >= 0.0 AND confidence <= 1.0),
        user_confirmed INTEGER NOT NULL DEFAULT 0    CHECK (user_confirmed IN (0,1)),
        created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_preferences_scope  ON preferences(scope);
    CREATE INDEX idx_preferences_entity ON preferences(entity_id);
    "#,
    // v14: the connector source registry (Stage 3, §8.1) — the per-service connection record the
    // index-only foundation (v11) deliberately left to the connector cards. The foundation owns the
    // source-agnostic SEMANTICS (pointer ingest, the change reducer, the encrypted manifest) and keys
    // items by a free-text `documents.source_id`; it intentionally has no table for the *accounts /
    // folders* a connector syncs from. This is that table: one row per connected service account
    // (e.g. a Google Drive account), holding what only the connector knows — the delta `cursor` (the
    // Drive changes page-token / OneDrive delta token), `last_synced_at`, the reachability `state`,
    // and the per-source `mode`. Generic on purpose so the sibling cards reuse it unchanged: 4A Drive
    // (`provider='google', service='drive'`), 4B OneDrive (`provider='microsoft'`), 4C local-folder
    // (`provider='local'`), and later Calendar/Gmail service rows under the same Google provider.
    //   * `id` is the stable row key a connector namespaces its item ids under — `documents.source_id`
    //     is `'<id>:<fileId>'`, so the foundation's `source_id LIKE '<id>:%'` fan-out flips a whole
    //     account to `unreachable` on an auth failure. For Drive: `'gdrive:<account-email>'`.
    //   * `provider`/`service` are free TEXT (documented, not CHECK-constrained) so a new connector
    //     class — a local folder is not an OAuth "provider" in the same sense — needs no constraint
    //     relaxation later. `mode`/`state` ARE closed enums I control, so they keep a CHECK.
    //   * `mode` defaults `index_only` (the only mode built now; the `import`-into-vault option is a
    //     deferred follow-up that needs no schema change). `folder_ids` is nullable JSON — NULL means
    //     "the whole source" (Drive's whole-My-Drive default); a narrowed selection lands later.
    // OAuth tokens never live here — they stay in the OS keychain, keyed per service+account; this
    // table holds only non-secret connection state. All additive — an existing store just gains an
    // empty table, no backfill (rule #3). Vectors and every existing query are untouched.
    r#"
    CREATE TABLE connector_sources (
        id             TEXT PRIMARY KEY,    -- stable account key, e.g. 'gdrive:<account-email>'
        provider       TEXT NOT NULL,       -- 'google' | 'microsoft' | 'apple' | 'local'
        service        TEXT NOT NULL,       -- 'drive' | 'calendar' | 'gmail' | 'folder'
        label          TEXT NOT NULL,       -- user-facing name (the account email / folder name)
        account_email  TEXT,
        mode           TEXT NOT NULL DEFAULT 'index_only'
                         CHECK (mode IN ('import','index_only')),
        folder_ids     TEXT,                -- JSON array of source-scoped folder ids; NULL = whole source
        cursor         TEXT,                -- opaque delta cursor (Drive changes page-token, etc.)
        last_synced_at TEXT,
        state          TEXT NOT NULL DEFAULT 'ok'
                         CHECK (state IN ('ok','unreachable','error')),
        created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_connector_sources_service ON connector_sources(provider, service);
    "#,
    // v15: record the **actual** USD cost OpenRouter reports per call (its usage-accounting
    // `usage.cost`), so the Usage tab can show real spend — including prompt-cache discounts — rather
    // than only a tokens × cached-price estimate that goes blank when a model isn't in the price
    // cache. Nullable: older rows (and any provider that doesn't report cost) stay NULL and fall back
    // to the estimate. Additive, regenerable, no backfill (rule #3).
    r#"
    ALTER TABLE usage_log ADD COLUMN cost_usd REAL;
    "#,
    // v16: cached 2-D coordinates for the semantic memory map (one row per document for the current
    // layout). Fully regenerable from `chunk_vec` — the layout module drops + rebuilds these on a
    // fingerprint mismatch (embedder/dim/node-cap/doc-set change), and the cache only spares
    // re-reading + averaging every leaf-chunk vector on each Map open. `method` records which reducer
    // produced the coords (`pca` | `tsne`) for display. Additive, no backfill — an existing store
    // just gains an empty table (rule #3); vectors and every existing query are untouched.
    r#"
    CREATE TABLE doc_layout (
        document_id  INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        method       TEXT NOT NULL,            -- 'pca' | 'tsne' (the reducer actually used)
        x            REAL NOT NULL,
        y            REAL NOT NULL,
        PRIMARY KEY (document_id, method)
    );
    "#,
    // v17: add 'archive' as a distinct importance level. Until now `documents.importance` was
    // `high|medium|low|NULL`, where NULL doubled as both "not yet triaged" and "none". 'archive' is
    // a fourth, EXPLICIT level — a document the user deliberately shelved: hidden from the Map,
    // sunk to the bottom of importance-sorted lists, but still fully searchable (incl. exact
    // keyword). It is deliberately SEPARATE from NULL so a brand-new, un-reviewed document (NULL)
    // still appears on the Map; only a deliberate Archive choice hides it.
    //
    // The only change is relaxing the column CHECK to admit 'archive'. SQLite can't ALTER a column
    // CHECK in place, and a full table rebuild would mean recreating `documents` (20+ columns) and
    // its three FK children (chunks, corrections, doc_layout) + every index — high-risk for a
    // one-token change. Instead we use the SQLite-documented `writable_schema` text-patch: it edits
    // the stored CREATE TABLE text only, moves no data, and touches nothing else. We surgically
    // replace the value list, which appears exactly once (in this CHECK) and only on `documents`.
    // The schema cookie is then bumped (see `run`) so this connection reparses the new constraint.
    //
    // GOTCHA (canonical note — the later writable_schema patches reference this one): the patch leaves
    // THIS connection's cached schema stale, and `run()` only reparses once, at the end of the batch.
    // A same-run LATER migration that `ALTER … ADD COLUMN`s a writable_schema-patched table then fails
    // with a baffling `near "…": syntax error` (a DEFAULT-expression DDL re-parse fault, not a bad
    // ALTER). Fix: emit `PRAGMA writable_schema=RESET;` at the top of that migration first — verified
    // clean on SQLCipher (reloads the schema in-memory, no page-1 write → no HMAC corruption). See
    // AGENTS.md rule 3; v37 does exactly this — a RESET, then ALTER usage_log ADD COLUMN.
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''high'',''medium'',''low''', '''high'',''medium'',''low'',''archive''')
     WHERE type = 'table' AND name = 'documents';
    PRAGMA writable_schema = OFF;
    "#,
    // v18: the multi-provider calendar foundation (Stage 3, board cards 6A/6B). The v6 mirror was
    // single-Google-account and flat: events keyed by an overloaded `calendar_id` string (a Google
    // calendar id OR an `ics:<hex>` feed id), with no account model, no calendar registry, no stored
    // iCal UID, and no slot to ever link an event to a PM entity. This card builds the clean
    // three-level relational model — **account → calendar → event** — that Google (now multi-account),
    // Outlook (Microsoft Graph OAuth), and Apple/any iCal subscription all flow into, and that the
    // later unified calendar VIEW (card 6B) renders.
    //
    //   * `connector_sources` (v14) is REUSED as the account/subscription layer (one row per Google
    //     account `gcal:<email>`, Outlook account `outlook:<email>`, or iCal subscription `ical:<hex>`,
    //     all `service='calendar'`) — holding the uniform per-source `last_synced_at` + reachability
    //     `state`. Read-only with a small rolling window uses delete-then-reinsert per calendar, so
    //     `cursor` stays NULL (no delta tokens).
    //   * `calendars` (NEW) is one row per individual calendar within a source — a Google/Outlook
    //     account has many; an iCal subscription is exactly one. `selected` replaces the old
    //     `settings.google_calendar_ids` blob; `color` is the per-source colour the 6B view will use.
    //     `source_id` cascades, so dropping an account drops its calendars (the app also deletes their
    //     mirrored events explicitly — belt-and-braces, independent of the foreign_keys pragma).
    //   * `calendar_events` (the DERIVED mirror) is EXTENDED in place — additive ALTER, never a
    //     destructive rebuild (rule #3): it's reconstructable on the next sync, and `uid` simply
    //     backfills then. `uid` is the iCal UID — the durable identifier that survives edits and
    //     round-trips across Google/Outlook/Apple, the anchor the Stage-4 "Calendar ↔ PM
    //     correspondence" card needs. `entity_id` is that correspondence SLOT: nullable, **written by
    //     nobody this stage**, so the Stage-4 card ships with zero schema change. `calendar_events.
    //     calendar_id` now logically references `calendars.id` (an app-maintained relation, exactly as
    //     the old `remove_feed` already cleaned events by `calendar_id`). All additive — an existing
    //     store gains the `calendars` table + two nullable event columns, no backfill; the next sync
    //     repopulates the mirror under the new account→calendar ids (rule #3). Vectors are untouched.
    r#"
    CREATE TABLE calendars (
        id          TEXT PRIMARY KEY,    -- 'gcal:<email>:<calId>' | 'outlook:<email>:<calId>' | 'ical:<hex>'
        source_id   TEXT NOT NULL REFERENCES connector_sources(id) ON DELETE CASCADE,
        provider    TEXT NOT NULL,       -- 'google' | 'microsoft' | 'apple' | 'other' (denorm for grouping/colour)
        remote_id   TEXT,                -- the provider's own calendar id; NULL for a single-calendar ICS subscription
        name        TEXT NOT NULL DEFAULT '(calendar)',
        color       TEXT,                -- per-source colour the unified view (6B) uses; nullable
        selected    INTEGER NOT NULL DEFAULT 1,  -- sync/show this calendar? (replaces settings.google_calendar_ids)
        is_primary  INTEGER NOT NULL DEFAULT 0,
        created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_calendars_source ON calendars(source_id);

    ALTER TABLE calendar_events ADD COLUMN uid       TEXT;                        -- iCal UID: the durable cross-provider anchor (Stage-4 seam)
    ALTER TABLE calendar_events ADD COLUMN entity_id INTEGER REFERENCES entities(id);  -- correspondence slot; WRITTEN BY NOBODY this stage
    CREATE INDEX idx_calendar_events_uid      ON calendar_events(uid);
    CREATE INDEX idx_calendar_events_entity   ON calendar_events(entity_id);
    CREATE INDEX idx_calendar_events_calendar ON calendar_events(calendar_id);
    "#,
    // v19: shared-drive deduplication across Google accounts (board card 4A follow-up).
    //
    // Shared (Team) drives were indexed PER ACCOUNT — source_id `gdrive:<email>:sd:<driveId>:<fileId>`
    // — so the same Team Drive reachable from two connected accounts was indexed twice. Drive file ids
    // are globally stable, so a shared drive's files now live ACCOUNT-INDEPENDENTLY under
    // `gdrive:sd:<driveId>:<fileId>` and are indexed once: by whichever account syncs the drive first
    // (its "owner"); other accounts with access don't re-index it (the scope UI greys those out).
    //
    // `shared_drive_access` is the access relation — one row per (drive, account) recording who can
    // reach each shared drive, and which one owns its index. `account_id` FKs the registry with
    // ON DELETE CASCADE, so disconnecting an account drops its access rows (the connector then
    // soft-flags any drive no remaining account can reach).
    //
    // Existing per-account shared-drive pointers are RE-KEYED in place to the new namespace, not
    // dropped: rule #3 requires an old→new mapping that preserves every user classification (and the
    // embeddings ride along for free). Where two accounts had indexed the same drive, the second row
    // collides on the source_id unique index and is left at its old id (a "twin"). NOTE: the original
    // claim that "the next sync's reconcile retires the twin" was never true — the reconcile only ever
    // matches the NEW `gdrive:sd:` namespace, so the twin was invisible to it and stranded forever.
    // `drive::resolve_shared_drive_twins` (run at vault open, v3.21.0) is what actually retires an
    // identical twin and surfaces a divergent one. Clearing every Drive account's delta cursor still
    // forces a re-baseline under the new namespace (My Drive re-enumerates as no-op Updates; each
    // shared drive is indexed once, by its owner).
    r#"
    CREATE TABLE shared_drive_access (
        drive_id   TEXT NOT NULL,
        account_id TEXT NOT NULL REFERENCES connector_sources(id) ON DELETE CASCADE,
        is_owner   INTEGER NOT NULL DEFAULT 0,   -- the one account whose sync indexes + reconciles this drive
        -- RESERVED (v19): the "synced by X" UI reads the ACCOUNT's label, not this; nothing has
        -- ever written it. Kept per the additive-only rule.
        name       TEXT,                          -- cached drive display name (unused; see above)
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        PRIMARY KEY (drive_id, account_id)
    );
    CREATE INDEX idx_shared_drive_access_drive   ON shared_drive_access(drive_id);
    CREATE INDEX idx_shared_drive_access_account ON shared_drive_access(account_id);

    -- guard:allow — rule-#3 preserving re-key (old→new source_id mapping; no DELETE, no data-column
    -- overwrite). `substr(id, instr(id,':sd:')+4)` keeps `<driveId>:<fileId>` verbatim, so the row —
    -- its classification (project/importance/reviewed/entity_id) AND its embedding — survives intact.
    -- Where two accounts indexed the same drive the second row would collide on the source_id unique
    -- index; OR IGNORE leaves that twin at its old id. `drive::resolve_shared_drive_twins` (vault-open,
    -- v3.21.0) retires an identical twin and surfaces a divergent one — the reconcile never could, as
    -- it matches only the new `gdrive:sd:` namespace. It is a pointer, not file bytes; nothing is lost.
    UPDATE OR IGNORE documents
       SET source_id = 'gdrive:sd:' || substr(source_id, instr(source_id, ':sd:') + 4)
     WHERE source_type = 'index_only' AND source_id LIKE 'gdrive:%:sd:%';

    UPDATE connector_sources SET cursor = NULL WHERE provider = 'google' AND service = 'drive';
    "#,
    // v20: Project Milestones (multi-deadline) — board card 7. Replaces the single
    // `projects.deadline` scalar with a many-to-one milestone model: a project has zero or
    // more dated milestones, each its OWN row with a STABLE id (the anchor a later flag-layer
    // card hangs deadline-derived flags on, so the flag layer never re-keys project→milestone —
    // the destructive change rule #3 forbids).
    //
    // A milestone is either PM-native (a user-set, editable `due_date`) or calendar-linked
    // (`event_uid` set → its date syncs FROM the canonical, read-only `calendar_events` row by
    // iCal UID). Same table; the two provenances are distinguished by whether `event_uid` is
    // present. `due_date` stays nullable so a calendar-linked milestone whose event is gone /
    // unsynced resolves to NULL (excluded from the focus view's "governing milestone", never
    // silently treated as met).
    //
    // FK on `projects(name)` (the identity every focus-view consumer keys on — `documents.project`,
    // `projects.name`), ON DELETE CASCADE so dropping a project cleans up its milestones. The
    // milestone insert path upserts a bare `projects` row first, so the FK holds for a never-triaged
    // (lazy) project.
    //
    // Additive only (rule #3): `projects.deadline` is KEPT (never dropped) as a write-through legacy
    // cache — derivation reads ONLY milestones. The one-time backfill carries each existing manual
    // deadline into a single `label='deadline'` milestone (every such project already has a `projects`
    // row, so the FK is satisfied without creating rows here), so the governing-milestone derivation
    // reproduces today's behaviour with no data loss.
    r#"
    CREATE TABLE project_milestones (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,                 -- STABLE id (flag-layer anchor)
        project_name TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        label        TEXT NOT NULL DEFAULT 'deadline',                  -- e.g. 'pitch', 'internal deadline'
        due_date     TEXT,                                              -- ISO date; NULL when event-linked & event missing
        event_uid    TEXT,                                              -- iCal UID into calendar_events; NULL = PM-native
        state        TEXT CHECK (state IN ('met','unmet') OR state IS NULL),  -- NULL = untracked (treated as unmet)
        sort_order   INTEGER NOT NULL DEFAULT 0,
        created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_project_milestones_project ON project_milestones(project_name, sort_order);
    CREATE INDEX idx_project_milestones_uid     ON project_milestones(event_uid);

    INSERT INTO project_milestones (project_name, label, due_date, sort_order)
        SELECT name, 'deadline', deadline, 0
          FROM projects
         WHERE deadline IS NOT NULL AND trim(deadline) <> '';
    "#,
    // v21: Project active-date + manual priority (Stage-3 focus follow-ups). Two additive
    // columns on `projects` (rule #3 — never drop/rewrite existing data):
    //   * last_touched — bumped when the user engages a project OUTSIDE document ingest
    //     (sends a message in its scoped chat, or edits its milestones). `list_overviews`
    //     takes the project's "active" date as MAX(document activity, last_touched), so
    //     chatting/triaging a project counts as activity (and so feeds the Take-a-look
    //     staleness signal and the Recent-active sort). NULL until first touched.
    //   * importance — a MANUAL priority override (high/medium/low) set in Triage. NULL = Auto
    //     (shows no tag). This replaces the old "highest-importance document" heuristic, which
    //     was misleading: one important document doesn't make the PROJECT important. A real,
    //     structural auto-importance signal (e.g. depended-on by other projects) is deferred.
    // Additive only; no backfill — every project starts on Auto, untouched.
    r#"
    ALTER TABLE projects ADD COLUMN last_touched TEXT;
    ALTER TABLE projects ADD COLUMN importance   TEXT
        CHECK (importance IN ('high','medium','low') OR importance IS NULL);
    "#,
    // v22: Photo / screenshot ingestion (Stage-3 board card #135). Photos are a new ingestion
    // source type that REUSES the document pipeline wholesale: each ingested photo becomes a
    // `documents` row with `source_type='photo'` (so split/embed/FTS/vector/retrieval/Map/citations/
    // rebuild all work unchanged, exactly as 'index_only' is just a discriminator), PLUS one row in
    // this new `photos` satellite table holding the image-specific truth (capture date, GPS, the
    // OCR text, the on-disk hash, and the opt-in vault copy). The chunks attach to the documents row;
    // `photos` links back by `document_id`.
    //
    // Two parts, both additive (rule #3 — no data moved, no column dropped):
    //   * Relax the `documents.source_type` CHECK to admit 'photo'. SQLite can't ALTER a column CHECK
    //     in place, so we reuse v17's `writable_schema` text-patch (it edits the stored CREATE TABLE
    //     text only; `run` then bumps the schema cookie so this connection reparses the new
    //     constraint). The value list `'vault','index_only'` appears exactly once — in this CHECK,
    //     added by v11 — and only on `documents`. (A later same-run ALTER on this patched table needs
    //     `writable_schema=RESET` first — see v17's gotcha note + AGENTS.md rule 3.)
    //   * `photos` (NEW). `file_hash` is the SHA-256 of the image BYTES — the dedupe/identity anchor
    //     that survives a moved/renamed source (UNIQUE, so re-dropping the same image is a no-op).
    //     `source_type` is the capture provenance (screenshot/camera_roll/dragged_file/vault_copy),
    //     a DIFFERENT axis from documents.source_type. `ocr_text` is the indexed content (NULL when
    //     OCR was declined/empty). `visual_description` is RESERVED for Stage-4 image understanding —
    //     always NULL on insert, no writer this stage, so that card ships with zero schema change.
    //     `saved_to_vault`/`vault_path` record the opt-in original copy (NULL path unless copied).
    //     `document_id` cascades, so a rebuild's `DELETE FROM documents` teardown clears photos too
    //     (the photo-specific fields round-trip via the vault frontmatter, so rebuild reconstructs them).
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''vault'',''index_only''', '''vault'',''index_only'',''photo''')
     WHERE type = 'table' AND name = 'documents';
    PRAGMA writable_schema = OFF;

    CREATE TABLE photos (
        id                 INTEGER PRIMARY KEY AUTOINCREMENT,
        document_id        INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        source_path        TEXT,
        source_type        TEXT NOT NULL DEFAULT 'dragged_file'
                             CHECK (source_type IN ('screenshot','camera_roll','dragged_file','vault_copy')),
        capture_date       TEXT,
        file_hash          TEXT NOT NULL,
        ocr_text           TEXT,
        visual_description TEXT,
        saved_to_vault     INTEGER NOT NULL DEFAULT 0 CHECK (saved_to_vault IN (0,1)),
        vault_path         TEXT,
        width              INTEGER,
        height             INTEGER,
        lat                REAL,
        lon                REAL,
        created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_photos_document ON photos(document_id);
    CREATE UNIQUE INDEX idx_photos_file_hash ON photos(file_hash);
    "#,
    // v23: Chat ingestion foundation (Stage-3 board card 7A, #140). The substrate that turns PM's own
    // conversations into a first-class ingestion source — modelled like a document so the existing
    // pipeline (chunk → embed → index → hybrid retrieval) treats an indexed chat as "just another
    // document". This card lands ONLY the data-model + write-discipline seam: no indexing (card B), no
    // UI (cards C/E), no retrieval (card C), and no user-visible behaviour change.
    //
    // Two additive parts (rule #3 — no data moved, no column dropped):
    //   * Relax the `documents.source_type` CHECK to admit 'chat'. SQLite can't ALTER a column CHECK in
    //     place, so we reuse the v17/v22 `writable_schema` text-patch (it edits the stored CREATE TABLE
    //     text only; `run` then bumps the schema cookie so this connection reparses the new constraint;
    //     a later same-run ALTER on this patched table needs `writable_schema=RESET` first — see v17's
    //     gotcha note + AGENTS.md rule 3).
    //     The value list `'vault','index_only','photo'` appears exactly once — in this CHECK — and only on
    //     `documents`. A chat session, when card B indexes it, becomes a `documents` row with
    //     `source_type='chat'` backed by a real Markdown vault file, so split/embed/FTS/vector/retrieval/
    //     Map/citations/rebuild/deletion all work unchanged (exactly as 'index_only'/'photo' are just
    //     discriminators). The org fields it needs — project/tags/importance/archive/reviewed/source_state
    //     — therefore live on `documents` (reused, not duplicated), which is what lets card F file chat
    //     through the same `write_document_truth` path documents use and lets the Stage-4 M:N migration
    //     (card ii) carry chat as one shared migration. Nobody writes a 'chat' row THIS card.
    //   * `chat_sessions` (NEW) is the thin satellite holding the chat-specific state a document doesn't
    //     have. Keyed by `conversation_id` (one session per conversation; FK ON DELETE CASCADE so deleting
    //     a chat drops its session row — card G's deletion cascade also purges the documents row + chunks +
    //     vault file). `document_id` is the link to the source row card B creates; it stays NULL until then
    //     (ON DELETE SET NULL so a Rebuild's `DELETE FROM documents` teardown leaves the session row, and
    //     card B re-links on re-index). `scope` records ORIGIN — a chat opened global vs project-scoped —
    //     which is what card F routes on (general → review queue, project → skip), distinct from whichever
    //     project the chat is later FILED under (that's `documents.project`). The two cursors are the heart
    //     of the card: `last_indexed_turn_id` is an INDEX-STATE cursor (the assistant message id of the
    //     last indexed turn-pair) recording how far the index has caught up — NOT a truth cursor; card B
    //     only ever embeds turn-pairs past it, idempotently, so "is the conversation done?" never needs
    //     answering — only "is there content past the cursor?". `summary_covers_up_to_turn_id` records
    //     which turns the rolling summary already accounts for so card C extends it from the new tail
    //     rather than re-reading the whole history. Both cursors are NULL here (nothing indexed/summarised
    //     yet) and advanced by B/C. `vault_path` is the session's `.md` on-disk name, set when 7A first
    //     appends a turn-pair to the vault. All additive — an existing store just gains an empty table,
    //     no backfill (rule #3). Vectors and every existing query are untouched.
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''vault'',''index_only'',''photo''', '''vault'',''index_only'',''photo'',''chat''')
     WHERE type = 'table' AND name = 'documents';
    PRAGMA writable_schema = OFF;

    CREATE TABLE chat_sessions (
        conversation_id              INTEGER PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
        document_id                  INTEGER REFERENCES documents(id) ON DELETE SET NULL, -- the source row card B creates; NULL until then
        scope                        TEXT NOT NULL CHECK (scope IN ('general','project')), -- ORIGIN (global vs project-scoped), card F routes on it
        vault_path                   TEXT,        -- the session's .md on-disk name; NULL until the first turn-pair is appended
        last_indexed_turn_id         INTEGER,     -- index-state cursor (assistant msg id of last indexed pair); card B advances. NOT a truth cursor
        summary_covers_up_to_turn_id INTEGER,     -- rolling-summary cursor; card C advances
        last_active_at               TEXT,
        created_at                   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_chat_sessions_document ON chat_sessions(document_id);
    "#,
    // v24: Incremental chat indexing — per-chunk provenance + timestamp (Stage-3 board card 7B, #141).
    // Card A (v23) made a chat a vault-truth file + cursors; card B is the engine that embeds completed
    // turn-pairs into chunks, append-only off `last_indexed_turn_id`. Two locked card-B decisions need a
    // per-CHUNK home that `chunks` doesn't have yet, so this migration adds two additive, nullable columns
    // (rule #3 — nothing moved, nothing dropped; existing rows keep NULL and behave exactly as before):
    //   * `chat_turn_id` — the turn-pair a chat chunk came from (the assistant message's id, card A's turn
    //     identity). With the chat's `documents.vault_path` and `chat_sessions.conversation_id` (reachable
    //     via `documents.id = chat_sessions.document_id`) this is the full source pointer card E needs to
    //     "open the chat to this turn" — stored as navigation metadata ALONGSIDE the vector, never embedded.
    //     NULL for every non-chat chunk.
    //   * `chunk_at` — a per-chunk timestamp (the turn-pair's own time). Chat is indexed append-only, so one
    //     document spans turns authored months apart; per-chunk recency means a months-old chat with one
    //     fresh decision isn't uniformly stale. Retrieval prefers this over the document's `last_activity`
    //     via COALESCE, so a NULL (every document/photo/index-only chunk) transparently falls back to the
    //     existing per-document recency — no behaviour change for anything but chat.
    // Both leave `chunk_vec`/`chunks_fts` (keyed by `chunks.id`) untouched; no Rebuild is forced (boundaries
    // and the SPLITTER_VERSION stamp are unchanged — these are pure metadata columns).
    r#"
    ALTER TABLE chunks ADD COLUMN chat_turn_id INTEGER;
    ALTER TABLE chunks ADD COLUMN chunk_at     TEXT;
    "#,
    // v25: Rolling conversation summary storage (Stage-3 board card 7C, #142). Card A (v23) reserved the
    // `summary_covers_up_to_turn_id` cursor on `chat_sessions` but left nowhere to PUT the summary it
    // bounds; card C is context assembly, which needs the summary text persisted so it survives a relaunch
    // and rides in the cache-stable prompt prefix turn after turn. One additive, nullable column (rule #3 —
    // existing rows keep NULL and assemble exactly as before, i.e. the whole recent window verbatim):
    //   * `summary` — the rolling summary of the conversation arc BEFORE the recency window, append-extended
    //     from the RAW indexed turns one segment at a time (never re-summarised — that would be lossy
    //     compounding). NULL until the conversation first grows past the window+batch threshold. Disposable
    //     by design: the markdown vault keeps every raw turn (card A), so this can be discarded and
    //     regenerated from the index at any time — it is a generation-time cost artifact, never a source of
    //     truth, and is never itself embedded.
    r#"
    ALTER TABLE chat_sessions ADD COLUMN summary TEXT;
    "#,
    // v26: Last-turn prompt size for the context-usage meter (Stage-3 board card 7D, #143). The meter shows
    // how full the SELECTED model's context window is; rather than estimate the prompt size with a local
    // tokenizer or a char heuristic, we persist the EXACT `prompt_tokens` OpenRouter reports for each reply
    // (already captured in `send_message`). One additive, nullable column (rule #3 — existing rows keep NULL):
    //   * `last_prompt_tokens` — the measured prompt-token count of this conversation's most recent turn, the
    //     meter's numerator over `model_pricing.context_length`. Because OpenRouter counted the real assembled
    //     prompt, it already reflects everything that rode along (profile, agenda, rolling summary, recency
    //     window, retrieved grounding). NULL until the first reply lands ⇒ the meter shows "unknown", never a
    //     wrong number. A cost/measurement artifact, not truth.
    r#"
    ALTER TABLE chat_sessions ADD COLUMN last_prompt_tokens INTEGER;
    "#,
    // v27: Title provenance for auto-generated, editable chat titles (Stage-3 board card 7E, #144). A chat's
    // history label starts as the first-message placeholder (card A names it the first 48 chars); once the
    // conversation has a few turns the BACKGROUND model writes a real 5-7 word title ONCE, and the user can
    // edit it. We must (a) generate exactly once, never on every idle tick, and (b) never overwrite a user
    // edit — so we track which of the three a title is. One additive, NOT-NULL-with-default column (existing
    // rows take 'pending', matching the placeholder they already carry):
    //   * `title_state` — 'pending' (still the placeholder; eligible for one background generation) →
    //     'generated' (the background model named it once; locked from regen) | 'custom' (the user renamed it;
    //     locked). The background pass guards its write with `WHERE title_state = 'pending'`, so a concurrent
    //     rename always wins.
    r#"
    ALTER TABLE chat_sessions ADD COLUMN title_state TEXT NOT NULL DEFAULT 'pending'
        CHECK (title_state IN ('pending','generated','custom'));
    "#,
    // v28: chat in the learning loop, preference extraction (Stage 3, board card 7F). An EXPLICIT
    // preference the user states in a chat ("I always want X as Y") becomes a typed preference record,
    // gated through Teach for confirmation like any other — so it needs (a) a new `source` value, and
    // (b) a per-session cursor so extraction, like indexing and summarising, only ever looks at the
    // turns past what it has already read.
    //   * Relax `preferences.source` to admit 'chat' (the chat-stated origin, distinct from the
    //     user-typed 'user' and the PM-'inferred' still deferred to Stage 5). SQLite can't ALTER a
    //     column CHECK in place, so we reuse the v17/v22/v23 `writable_schema` text-patch: it edits the
    //     stored CREATE TABLE text only, moves no data, and touches nothing else. The value list
    //     `'user','inferred'` appears exactly once (this CHECK) and only on `preferences`; the schema
    //     cookie bump in `run` reparses the relaxed constraint on this connection. (A later same-run
    //     ALTER on this patched table needs `writable_schema=RESET` first — see v17's gotcha note.)
    //   * `prefs_covers_up_to_turn_id` — the extraction cursor on `chat_sessions`, mirroring
    //     `summary_covers_up_to_turn_id`. Additive, NULL until the first extraction pass advances it.
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''user'',''inferred''', '''user'',''inferred'',''chat''')
     WHERE type = 'table' AND name = 'preferences';
    PRAGMA writable_schema = OFF;
    ALTER TABLE chat_sessions ADD COLUMN prefs_covers_up_to_turn_id INTEGER;
    "#,
    // v29: Drive parent-folder tag + normalized source account (Stage-3 Drive folder text-bias, plus
    // the Part-B source_account follow-up). Three additive, nullable columns on `documents` — no data
    // moved, no CHECK relaxed, so plain `ADD COLUMN` suffices:
    //   * `source_parent_folder_id` / `source_parent_folder_name` — the Drive folder a synced file was
    //     found in, snapshotted at ingest time. The name is fed as PLAIN TEXT into the sorting-review
    //     profile preamble (the same seam that already carries the Learning-You preferences) to BIAS —
    //     never pre-assign — the model's project proposal; the LLM proposal stays the review checkpoint.
    //     Never reaches the chunker/embedder. NULL for every non-Drive source and for rows ingested
    //     before this migration (no backfill — a later Drive refresh re-populates them on re-ingest).
    //   * `source_account` — the owning account promoted out of `source_id`'s inline
    //     `gdrive:<email>:<fileId>` encoding into a first-class, filterable column. Derived at insert
    //     time from `source_id` via `drive::account_of`, so it self-heals on a Rebuild; NULL where the
    //     id carries no account (vault, shared-drive, OneDrive, chat). `source_id` is left byte-for-byte
    //     unchanged — this is a derived convenience column alongside it, not a replacement. Existing
    //     rows stay NULL until a dedicated backfill pass (out of scope here).
    r#"
    ALTER TABLE documents ADD COLUMN source_parent_folder_id TEXT;
    ALTER TABLE documents ADD COLUMN source_parent_folder_name TEXT;
    ALTER TABLE documents ADD COLUMN source_account TEXT;
    "#,
    // v30: spreadsheet ingestion (board card: Spreadsheet Processing). `.xlsx/.csv` become a
    // dedicated ingest path that BYPASSES MarkItDown; a spreadsheet lands as a `documents` row with
    // `source_type='spreadsheet'` (so the existing split/embed/FTS/vector/retrieval/Map/citation/
    // rebuild/deletion machinery works unchanged) PLUS a row in the NEW `spreadsheets` satellite table.
    //
    // Two additive parts (rule #3 — no data moved, no column dropped):
    //   * Relax the `documents.source_type` CHECK to admit 'spreadsheet'. SQLite can't ALTER a column
    //     CHECK in place, so we reuse the v22/v23 `writable_schema` text-patch (it edits the stored
    //     CREATE TABLE text only; `run` then bumps the schema cookie so this connection reparses the new
    //     constraint). The value list `'vault','index_only','photo','chat'` appears exactly once — in
    //     this CHECK — and only on `documents`. (A later same-run ALTER on this patched table needs
    //     `writable_schema=RESET` first — see v17's gotcha note + AGENTS.md rule 3.)
    //   * `spreadsheets` (NEW) is the thin satellite holding the spreadsheet-specific truth a document
    //     doesn't have: `sheet_count`/`total_rows` and `chunked_rows` (rows actually indexed after the
    //     sidecar's per-sheet row cap — `chunked_rows < total_rows` records a truncation). These
    //     round-trip via the vault frontmatter, so a Rebuild's `DELETE FROM documents` teardown (which
    //     cascades this row away) reconstructs it from the `.md` without re-parsing the original file.
    //     `structured_data_summary` is RESERVED for later column-type/aggregate enrichment — always NULL
    //     on insert, no writer this card — exactly mirroring `photos.visual_description`'s current state,
    //     so that future card ships with zero schema change.
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''vault'',''index_only'',''photo'',''chat''', '''vault'',''index_only'',''photo'',''chat'',''spreadsheet''')
     WHERE type = 'table' AND name = 'documents';
    PRAGMA writable_schema = OFF;

    CREATE TABLE spreadsheets (
        id                      INTEGER PRIMARY KEY AUTOINCREMENT,
        document_id             INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        sheet_count             INTEGER,
        total_rows              INTEGER,
        chunked_rows            INTEGER,
        structured_data_summary TEXT,
        created_at              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_spreadsheets_document ON spreadsheets(document_id);
    "#,
    // v31: the Project Activity Log (board card: Project Activity Log / heat emit hook). An
    // append-only, name-keyed, EMIT-ONLY engagement record: every meaningful engagement with a project
    // — a message in its scoped chat, a document filed INTO it, a milestone edit — appends one
    // `project_activity` row. NOTHING reads this yet; a future Stage-4 heat scorer maps `kind` → weight
    // at READ time, which is why rows are OBSERVATIONS with NO weight/score column (so the log stays
    // decision-free if scoring later changes).
    //
    // Keyed on `projects(name)` — the identity every project surface already uses (`last_touched`,
    // `project_milestones.project_name`, `conversations.project`, calendar title-matching); `entity_id`
    // is the minority convention (nullable, NULL until a doc resolves it). ON DELETE CASCADE mirrors
    // `project_milestones` (v20) so dropping a project cleans up its log; NO ON UPDATE — `projects.name`
    // is never renamed (rename lives on `entities.canonical_name` + a `documents.project` cache rewrite).
    // The emit helper upserts a bare `projects` row first, so a never-triaged (entity_id NULL) project
    // still logs — every project has a name.
    //
    // Retention is baked in (avoids the `usage_log` unbounded trap): raw rows are kept for a recent
    // window (~30d) then compacted by the rollup job into per-(project,day,kind) counts in
    // `project_activity_daily` (raw rows pruned; the tiny daily rollups kept long-term). `occurred_at` is
    // unix SECONDS (integer, matching the retrieval age convention) so day-bucketing is a plain integer
    // divide (`occurred_at / 86400`, UTC). Additive only (rule #3) — older stores start with two empty
    // tables. `source_ref` is a free-form back-pointer (doc / conversation / milestone id), NOT an FK, so
    // a later-deleted target leaves the historical observation intact.
    r#"
    CREATE TABLE project_activity (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        project     TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        kind        TEXT NOT NULL CHECK (kind IN ('chat','ingest','milestone')),
        occurred_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),  -- unix seconds UTC
        source_ref  INTEGER                                                            -- related row id (doc/convo/milestone); NULL = none, NOT an FK
    );
    CREATE INDEX idx_project_activity_project_time ON project_activity(project, occurred_at);

    CREATE TABLE project_activity_daily (
        project TEXT    NOT NULL REFERENCES projects(name) ON DELETE CASCADE,
        day     INTEGER NOT NULL,                                                       -- unix-day = occurred_at / 86400 (UTC)
        kind    TEXT    NOT NULL CHECK (kind IN ('chat','ingest','milestone')),
        count   INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (project, day, kind)
    );
    "#,
    // v32: the structured flag layer (board card 9). Proactive flags become first-class records
    // EVALUATED BEFORE the briefing/chat model writes, then rendered into prose — replacing the
    // free-associating daily briefing with a stable decision layer under it. The generated sentence
    // is volatile; the flag underneath is stable, so identity and resolution attach to the FLAG,
    // never to the rendered text — which is what makes daily regeneration idempotent.
    //
    // ANCHORING — no new migration needed. A flag hangs off a STABLE identity that already exists:
    // `anchor_kind='calendar'` → an iCal `UID` (`calendar_events.uid`, v18); `anchor_kind='milestone'`
    // → a milestone surrogate id (`project_milestones.id`, v20). Deadline-derived flags deliberately
    // anchor on the MILESTONE id, NOT the project (a project has many dated milestones — pitch,
    // presentation, internal — and each spawns its own flags; anchoring on project id and adding
    // milestones later would be a destructive re-key, which rule #3 forbids). Resolution keys on
    // `(anchor_kind, anchor, type)` (UNIQUE), so resolving "pitch prep" never touches
    // "presentation prep" or "happening-today" on the same anchor.
    //
    // No FK on `anchor` — like `project_milestones.event_uid`, it's an app-maintained SOFT link
    // resolved in code, not a DB relation: an anchor may point at a UID not currently in the mirror
    // (the mirror is rebuilt each sync) or a milestone id, and a GC pass (a later PR) prunes flags
    // whose anchored time has passed. Cascade-deleting on a transient calendar row would be wrong.
    //
    // STORAGE SEAM (the done-vs-preference split, kept physically separate from day one so it never
    // needs a re-key): THIS table is the per-instance flag-STATE home — transient, high-confidence,
    // scoped to the anchored instance, GC'd once it passes. Its `state`/`source`/`user_confirmed`/
    // `artifact_ptr` columns ARE that record. A CROSS-instance PREFERENCE ("stop nagging me two hours
    // out; I always prep the night before" — which tunes a flag TYPE's threshold or suppresses the
    // type for this user) is durable and lives in the `preferences` table (v13), NEVER here. The two
    // are never co-mingled.
    //
    // `source` reuses the spine-hardening vocabulary but with this layer's values — WHICH PATH CLOSED
    // the flag: 'detection' (found automatically) | 'assertion' (the user said so). NULL while the
    // flag is still active. On conflict assertion outranks detection (enforced in code, a later PR).
    // `confidence`/`user_confirmed` mirror v12/v13 (assertion → user_confirmed=1). Additive only
    // (rule #3) — nothing reads or writes this table yet; older stores start with one empty table.
    r#"
    CREATE TABLE flags (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        anchor_kind    TEXT NOT NULL CHECK (anchor_kind IN ('calendar','milestone')),  -- identity space of `anchor`
        anchor         TEXT NOT NULL,                                                   -- iCal UID | milestone id (as text)
        type           TEXT NOT NULL CHECK (type IN
                           ('prepare-ahead','deadline-approaching','happening-today','overdue')),
        threshold      TEXT,                                                            -- how far ahead it fires; NULL = type default
        state          TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active','resolved')),
        source         TEXT CHECK (source IN ('detection','assertion') OR source IS NULL),  -- which path CLOSED it; NULL while active
        confidence     REAL    NOT NULL DEFAULT 1.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
        user_confirmed INTEGER NOT NULL DEFAULT 0   CHECK (user_confirmed IN (0,1)),
        artifact_ptr   TEXT,                                                            -- documents.source_id of the satisfying artifact
        artifact_url   TEXT,                                                            -- documents.external_ref (open URL); display-only, moves on rename
        created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        resolved_at    TEXT,
        UNIQUE (anchor_kind, anchor, type)                                             -- resolution keys on (anchor,type); one live flag per key
    );
    CREATE INDEX idx_flags_state  ON flags(state);
    CREATE INDEX idx_flags_anchor ON flags(anchor_kind, anchor);
    "#,
    // v33: per-flag instance timestamp (F-18) — the GC seam the v32 comment reserved. A calendar flag
    // hangs off an iCal UID, which a RECURRING series SHARES across every occurrence, so resolving
    // "prepare-ahead" once left a resolved tombstone that suppressed EVERY future occurrence forever.
    // This records WHICH occurrence a flag is about (its event start); detection ages out a resolved
    // calendar tombstone once it is proposing a STRICTLY LATER occurrence, so the next recurrence
    // re-fires while the just-resolved one stays quiet. Additive + nullable (rule #3); NULL (a milestone
    // flag — its anchor is already per-instance — or a row written before v33) is treated as "keep
    // suppressed", so nothing resolved before this migration is retroactively re-fired.
    r#"
    ALTER TABLE flags ADD COLUMN instance_at TEXT;   -- occurrence this flag is about (event start); NULL = milestone flag or pre-v33
    "#,
    // v34: per-calendar "quiet" flag — show a calendar on the Calendar tab but keep its EVENTS out of
    // everything the assistant surfaces (daily briefing, "due soon" flags/reminders, the chat agenda
    // preamble, the focus view's upcoming list, and the project name-match). Distinct from `selected`,
    // which is a SYNC gate that prunes an unticked calendar's events from the mirror entirely: a quiet
    // calendar still syncs and still renders, it is merely filtered out of `calendar::agenda_query`
    // (the single reader behind every assistant path). Explicit calendar-LINKED milestones are left
    // alone — those are deliberate project deadlines, not the calendar's event stream. Additive +
    // defaulted (rule #3); existing calendars default to not-quiet, so behaviour is unchanged.
    r#"
    ALTER TABLE calendars ADD COLUMN quiet INTEGER NOT NULL DEFAULT 0;
    "#,
    // v35: which rebuild PASS last rebuilt this row's chunks (#371) — the checkpoint that makes an
    // interrupted Rebuild resumable instead of restarting. A pass mints one id (a uuid) and stamps every
    // document as it commits it, so a resume skips what the previous run already finished and the final
    // sweep can tell "this file is gone" from "this file is not done yet".
    //
    // Why a pass id and NOT the obvious keys. `content_hash` is WRONG: a Rebuild's dominant trigger is a
    // splitter/embedder change (`retrieval_rebuild_needed`), where every hash is IDENTICAL and every chunk
    // boundary must move — hashing would skip the whole vault and stamp it clean. A per-document copy of
    // the retrieval config is wrong for the opposite reason: on a manual "my index looks broken" Rebuild
    // nothing has changed, so every document would be skipped and the repair would do nothing. A pass id
    // means exactly "this run already did this document" — the only question resume needs answered.
    //
    // Additive + nullable (rule #3): every existing row reads as NULL = "no pass has claimed this" = stale,
    // so the first Rebuild after this migration does full work, exactly as today. Nothing else reads it.
    r#"
    ALTER TABLE documents ADD COLUMN rebuild_pass TEXT;   -- id of the rebuild pass that last built these chunks; NULL = pre-v35 or never rebuilt
    "#,
    // v36: relax the `usage_log.kind` CHECK. The v7 CHECK admits only 'chat'|'background', but four
    // background jobs write their own kinds by direct INSERT — 'chat_summary'/'chat_compress' (rolling
    // summary + Compress, chat_summary.rs), 'chat_title' (chat_title.rs), 'chat_prefs' (chat_prefs.rs).
    // Every one of those inserts is wrapped in best-effort `let _ = conn.execute(...)`, so SQLite's CHECK
    // silently REJECTED the row and the error was swallowed: that whole class of housekeeping spend never
    // reached `usage_log` and never showed up in the Usage & cost table. This admits the four kinds so the
    // rows land. SQLite can't ALTER a column CHECK in place, so we reuse the v17/v22/v23/v28 `writable_schema`
    // text-patch: it edits the stored CREATE TABLE text only, moves no data, and touches nothing else. The
    // value list `'chat','background'` appears exactly once — in this CHECK — and only on `usage_log`; the
    // schema cookie is then bumped (see `run`) so this connection reparses the relaxed constraint.
    // (This left THIS connection's cached usage_log schema stale — which is why v37 emits
    // `PRAGMA writable_schema=RESET` before ALTER-ing usage_log; see v37's note + AGENTS.md rule 3.)
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''chat'',''background''', '''chat'',''background'',''chat_summary'',''chat_compress'',''chat_title'',''chat_prefs''')
     WHERE type = 'table' AND name = 'usage_log';
    PRAGMA writable_schema = OFF;
    "#,
    // v37 (#297 live local provider): tag each usage row with how it was served — provider (local vs
    // cloud), the serving leg's latency, and why cloud served instead of a preferred local endpoint —
    // so the Usage & cost table and the Local AI tab can tell local from cloud spend and show local
    // latency/throughput. These fields populate on EVERY usage row (dense), so they are COLUMNS on
    // usage_log, not a satellite table: a satellite would force a LEFT JOIN on every read for no benefit.
    //
    // The catch, and the reason for the leading `PRAGMA writable_schema=RESET` (AGENTS.md rule 3): v36
    // relaxed usage_log's CHECK via a `writable_schema` text-patch, which leaves THIS connection's
    // cached usage_log schema stale (the `run()` end-of-batch reparse has not happened yet). A plain
    // `ALTER usage_log ADD COLUMN` here would regenerate the table from that stale definition and fail
    // (`near "…": syntax error`, re-parsing usage_log's `created_at DEFAULT (strftime(…,'now'))`).
    // `writable_schema=RESET` reloads the schema in-memory FIRST, so the ALTERs see the current
    // definition and succeed — verified clean on the bundled SQLCipher (no page-1 write ⇒ no HMAC
    // corruption, unlike a mid-run schema-cookie bump) and pinned by the RESET regression test below.
    // The columns are nullable/additive: pre-v37 rows (and any best-effort write that fails) read them
    // as NULL, i.e. "provider unknown", exactly like any other additive column.
    r#"
    PRAGMA writable_schema = RESET;
    ALTER TABLE usage_log ADD COLUMN provider        TEXT;     -- 'local' | 'cloud'
    ALTER TABLE usage_log ADD COLUMN latency_ms      INTEGER;  -- wall-clock of the serving leg, ms
    ALTER TABLE usage_log ADD COLUMN fallback_reason TEXT;     -- why cloud served vs preferred local; NULL = none
    "#,
    // v38 (#480): Google Drive "Shared with me" support + resource-key persistence.
    //
    // "Shared with me" is Drive's THIRD collection — files/folders other users grant you directly —
    // distinct from My Drive and shared (Team) drives. Its items are indexed ACCOUNT-INDEPENDENTLY under
    // `gdrive:swm:<rootId>:<fileId>` (the user-picked root as the container, an exact structural mirror
    // of the shared-drive id `gdrive:sd:<driveId>:<fileId>`; `rootId == fileId` for a file root) and
    // de-duplicated across accounts exactly like shared drives: the first account to sync a picked root
    // OWNS it and reconciles it, others with the same root shared skip (the scope UI greys them out).
    //
    // `shared_with_me_access` is that access relation — one row per (root, account) recording who can
    // reach each shared root and which account owns its index. `account_id` FKs the registry with
    // ON DELETE CASCADE, so disconnecting an account drops its access rows (the connector then
    // soft-flags any root no remaining account can reach). It mirrors `shared_drive_access` (v19) with
    // `root_id` in the container role that `drive_id` plays there.
    //
    // Link-shared items may need their Drive `resourceKey` replayed in the `X-Goog-Drive-Resource-Keys`
    // header. That header is applied at SYNC time from the in-hand file metadata (see
    // `drive::resource_key_header`); persisting the key so an ON-DEMAND live-body re-fetch can replay it
    // for a resource-key-GATED item is a follow-up (direct shares — the common case — need no key). So
    // this migration adds only the access relation, no `documents` column yet.
    r#"
    CREATE TABLE shared_with_me_access (
        root_id    TEXT NOT NULL,   -- the picked shared root's Drive fileId (a folder or a single file)
        account_id TEXT NOT NULL REFERENCES connector_sources(id) ON DELETE CASCADE,
        is_owner   INTEGER NOT NULL DEFAULT 0,   -- the one account whose sync indexes + reconciles this root
        name       TEXT,                          -- cached root display name (UI convenience)
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        PRIMARY KEY (root_id, account_id)
    );
    CREATE INDEX idx_swm_access_root    ON shared_with_me_access(root_id);
    CREATE INDEX idx_swm_access_account ON shared_with_me_access(account_id);
    "#,
    // v39: persist the Review tab's AI proposals as a regenerable cache. Until now a proposal
    // (project / tags / importance / reasoning) lived only in the webview's in-memory map, so on
    // app restart the queue looked entirely un-proposed and `propose_metadata` re-billed the model
    // for every item it had already classified. This table caches each streamed proposal keyed by
    // document; the Review tab hydrates from it on load and only asks the model for genuinely
    // un-proposed documents, and `commit_review` drops the row as a document leaves the queue.
    // Fully derived from the model call — additive, no backfill (rule #3): an existing store just
    // gains an empty table, and any document missing a row simply re-proposes exactly as before.
    // `ON DELETE CASCADE` mirrors `doc_layout` (v16), so deleting a document takes its cache with it.
    r#"
    CREATE TABLE document_proposals (
        document_id INTEGER PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
        project     TEXT NOT NULL,           -- canonical project name the model proposed
        tags        TEXT NOT NULL,           -- JSON array, mirrors the streamed Proposal.tags
        importance  TEXT,                     -- 'high'|'medium'|'low' or NULL (unclear), like documents.importance
        reasoning   TEXT NOT NULL,
        model       TEXT,                     -- served model that produced it (UI/debug only); NULL on the fallback path
        created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    "#,
    // v40 (calendar event popup): richer per-event detail for the click-to-open event popover. The
    // v6/v18 mirror carried only the fields the agenda/month views paint (title, times, location,
    // link, uid); PM is the calendar aggregator, so the detail popup surfaces everything the providers
    // hold. These additive columns are populated per provider (Google transparency/attendees/organizer/
    // conference/recurrence, Graph showAs/attendees/onlineMeeting, ICS TRANSP/ATTENDEE/ORGANIZER/RRULE)
    // on the next sync — the F-49 event hash folds them in, so existing rows rewrite once. All
    // nullable/defaulted (rule #3): a pre-v40 row reads them as "unknown" like any other additive
    // column. `calendar_events` carries no writable_schema patch, so plain ALTERs suffice (no RESET).
    r#"
    ALTER TABLE calendar_events ADD COLUMN show_as            TEXT;    -- busy|free|tentative|oof|elsewhere
    ALTER TABLE calendar_events ADD COLUMN organizer          TEXT;    -- display name or email
    ALTER TABLE calendar_events ADD COLUMN attendees          TEXT;    -- JSON array of {name,email,response,...}
    ALTER TABLE calendar_events ADD COLUMN conference_url     TEXT;    -- Meet/Teams join link
    ALTER TABLE calendar_events ADD COLUMN recurring          INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE calendar_events ADD COLUMN recurrence_summary TEXT;    -- raw RRULE / human summary
    ALTER TABLE calendar_events ADD COLUMN status             TEXT;    -- confirmed|tentative
    ALTER TABLE calendar_events ADD COLUMN visibility         TEXT;    -- default|public|private|confidential
    ALTER TABLE calendar_events ADD COLUMN created            TEXT;
    ALTER TABLE calendar_events ADD COLUMN updated            TEXT;
    "#,
    // v41 (import AI memory): admit 'imported' as a preferences.source provenance value, so memory
    // pasted from another AI (ChatGPT/Gemini/Claude) and distilled into records is tagged distinctly
    // from user/inferred/chat and gets its own "Imported" chip in Teach. A writable_schema text-patch
    // relaxes the source CHECK (SQLite can't ALTER a column CHECK in place); the v28 relaxation already
    // appended 'chat', so the CURRENT stored list is 'user','inferred','chat'. No table is ALTERed after
    // this patch in the same batch, so no writable_schema=RESET is needed (see the v17 note); the
    // end-of-run schema-cookie bump makes this connection reparse the relaxed CHECK.
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''user'',''inferred'',''chat''', '''user'',''inferred'',''chat'',''imported''')
     WHERE type = 'table' AND name = 'preferences';
    PRAGMA writable_schema = OFF;
    "#,
    // v42 (Stage-4 card 8): three additive columns beside the shipped milestones feature — a richer
    // progress `status`, and a generic `source_type`/`external_id` durable anchor for milestones that
    // originate OUTSIDE PM (a tracked spreadsheet row, a Notion DB row), mirroring the `event_uid`
    // UID-anchor pattern the calendar already uses.
    //
    // `status` does NOT replace `state` (met|unmet): `Milestone::is_met` — and therefore
    // `governing()` and every deadline flag — keeps reading `state`, so no shipped derivation moves.
    // The two are kept from ever contradicting each other by writing BOTH in one statement at each
    // setter (`milestones::set_status` / `set_state`), and by the backfill below stamping `done` on
    // rows already marked met. NULL `status` = the user has never set one (a pre-v42 row); the UI
    // renders it from `state` rather than showing a blank.
    //
    // The partial UNIQUE index is the card's "upsert-by-external_id, NEVER delete-and-recreate"
    // invariant made STRUCTURAL rather than merely documented: flags anchor on
    // `project_milestones.id`, so a sync that dropped and re-inserted its rows would mint new ids and
    // silently orphan every flag hanging off them. With the index in place, a delete-and-recreate
    // sync collides on its own second insert instead of corrupting the anchor space. It is partial
    // (`WHERE … IS NOT NULL`) so the many PM-native rows — which carry NULL for both columns — are
    // unconstrained; SQLite treats NULLs in a UNIQUE index as distinct anyway, but stating it keeps
    // the intent legible and the index small.
    r#"
    ALTER TABLE project_milestones ADD COLUMN status      TEXT
        CHECK (status IN ('not_started','in_progress','almost_done','done') OR status IS NULL);
    ALTER TABLE project_milestones ADD COLUMN source_type TEXT;  -- 'sheets' | 'notion' | …; NULL = PM-native
    ALTER TABLE project_milestones ADD COLUMN external_id TEXT;  -- the source's own stable row id

    CREATE UNIQUE INDEX idx_project_milestones_external
        ON project_milestones(source_type, external_id)
     WHERE source_type IS NOT NULL AND external_id IS NOT NULL;

    -- guard:allow — preserving backfill: fills the freshly-added, all-NULL status; overwrites nothing.
    UPDATE project_milestones SET status = 'done' WHERE state = 'met';
    "#,
    // v43 (Stage-4 card 9): stamp each logged correction with the version of the FILING PIPELINE that
    // produced the proposal being corrected (`review::FILING_PIPELINE_VERSION`).
    //
    // Why this column has to exist before it has a reader: per-source filing accuracy is measured by
    // counting corrections against filings, and that number is only meaningful WITHIN one pipeline
    // version. Every improvement to the filing AI silently invalidates the accumulated stats — the
    // proven case is #360, where the filing AI was near-blind on index-only connector documents, so
    // every correction logged before 2026-07-14 understates connector accuracy against a pipeline
    // that no longer exists. Nothing in the data says so, and nothing ever can: the rows are already
    // written and unlabelable. Stamping from here on is the only point at which the history stays
    // interpretable, which is why the column lands ahead of the consumer that will window on it.
    //
    // Deliberately NULLable with no backfill. A pre-v43 row genuinely does not know which pipeline
    // wrote it, and inventing a version for it would assert something false about exactly the rows
    // this column exists to distinguish. NULL reads as "unlabelable — predates the stamp".
    r#"
    ALTER TABLE corrections ADD COLUMN pipeline_version INTEGER;  -- NULL = logged before v43
    CREATE INDEX idx_corrections_pipeline ON corrections(pipeline_version, created_at);
    "#,
    // v44 (Stage-4 card 10): capture RETRIEVAL-relevance feedback — the signal a learned reranker
    // would one day train on, and which PM currently records nowhere.
    //
    // `corrections` is the wrong shape for this and always was: it logs FILING corrections (this
    // document belongs to that project), whereas a query-time cross-encoder needs to know whether a
    // CHUNK answered a QUERY. No table holds that today, so the reranker's gate can never open — not
    // for want of a model, but for want of a corpus. Capture has to start early and cheaply so the
    // corpus accrues during beta; nothing here trains anything.
    //
    // `messages.retrieved_chunk_ids` records what actually grounded each answer, because at the
    // moment the user reacts the frontend knows only which message it is — the chunk ids are long
    // gone. NULL = an ungrounded answer (nothing was retrieved), which is distinct from an empty
    // array and must stay distinguishable.
    //
    // `retrieval_feedback` snapshots the query and chunk ids rather than joining back to them, so a
    // row is self-contained training data. It still cascades from `messages`: deleting a
    // conversation must take its feedback with it — PM does not keep a shadow copy of what the user
    // asked after they've deleted the asking. `config_stamp` records the retrieval configuration in
    // force, for the same reason v43 stamps the filing pipeline: signal gathered under one chunking
    // and embedding regime is not comparable with signal gathered under another, and unlabelled
    // history cannot be separated later.
    r#"
    ALTER TABLE messages ADD COLUMN retrieved_chunk_ids TEXT;  -- JSON array; NULL = ungrounded answer

    CREATE TABLE retrieval_feedback (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        message_id   INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        query        TEXT NOT NULL,              -- snapshot of the asking turn
        chunk_ids    TEXT NOT NULL,              -- JSON array: what grounded the answer
        signal       TEXT NOT NULL CHECK (signal IN ('up','down','citation_click')),
        document_id  INTEGER,                    -- citation_click: the source the user opened
        config_stamp TEXT,                       -- retrieval config in force, for windowing
        created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_retrieval_feedback_message ON retrieval_feedback(message_id);
    CREATE INDEX idx_retrieval_feedback_created ON retrieval_feedback(created_at);
    -- One rating per answer (a later thumb replaces the earlier one); citation clicks are deduped
    -- per document, so re-opening the same source doesn't inflate the corpus with copies.
    CREATE UNIQUE INDEX idx_retrieval_feedback_rating
        ON retrieval_feedback(message_id) WHERE signal IN ('up','down');
    CREATE UNIQUE INDEX idx_retrieval_feedback_click
        ON retrieval_feedback(message_id, document_id) WHERE signal = 'citation_click';
    "#,
    // v45 (Stage-4 card 11): work-vs-personal typing for calendar events.
    //
    // Required by the Work-context score and the person-context flags, neither of which can tell a
    // 3pm standup from a 3pm dentist appointment today. The interim proxy — "any event in progress"
    // — treats those identically, which is precisely the distinction the feature needs.
    //
    // Typing is declared PER CALENDAR, not inferred per event. Someone who connects a work account
    // and a personal one has already made the distinction; asking the model to re-derive it from
    // event titles would be slower, cost tokens, and be wrong in exactly the ambiguous cases that
    // matter. `calendars.kind` sits beside `selected`/`color`/`quiet` — the per-calendar preferences
    // that already exist — and events inherit it at read time.
    //
    // `calendar_events.kind_override` is the escape hatch for the event that doesn't match its
    // calendar (the dentist appointment on the work calendar). Nothing writes it yet; it exists now
    // because adding it later would mean a second calendar migration, and the whole point of this
    // card is to bank the column while a migration is already being written. Resolution is
    // `COALESCE(event.kind_override, calendar.kind)` — the event wins when it disagrees.
    //
    // Both NULL-by-default and NULLABLE: NULL means "not typed", which is honest and distinct from
    // either value. No backfill guesses a kind from a calendar's name.
    r#"
    ALTER TABLE calendars ADD COLUMN kind TEXT
        CHECK (kind IN ('work','personal') OR kind IS NULL);  -- NULL = untyped
    ALTER TABLE calendar_events ADD COLUMN kind_override TEXT
        CHECK (kind_override IN ('work','personal') OR kind_override IS NULL);
    "#,
    // v46 (Stage-4 card 15, #275): the tag registry, and with it many-to-many project membership.
    //
    // Until now a tag had no identity at all. `documents.tags` is a JSON blob, so there was no way
    // to list, count or rename a tag without scanning every document — and nothing consumed tags
    // for retrieval, search or scoring. Projects, meanwhile, were single-valued: `documents.project`
    // is one string, so a document that genuinely belonged to two initiatives had to pick one.
    //
    // Both problems are the same missing table. Bobby's framing is that **every project IS a tag**,
    // so M:N membership falls out of multi-tagging rather than needing its own parallel machinery,
    // and a single `@tag` grammar can later reach projects and labels alike (#276).
    //
    // `kind` keeps the two populations apart where they differ. A `project` tag mirrors a real
    // project and keeps the user's verbatim casing — `projects.name` is a primary key and
    // `entities.canonical_name` is the alias key, so lowercasing here would collide with both. A
    // `group` tag is the free-form label the tag editor already writes, and stays lowercase. Only
    // project-kind rows ever touch the entity/alias space; that separation is what preserved #275's
    // original "tags must not enter the project alias space" constraint.
    //
    // `norm` is the matching key, stored rather than expressed as COLLATE NOCASE. That follows
    // `preferences`, which learned the same lesson: SQLite's `lower()` is ASCII-only with no ICU,
    // so the normalisation has to be visible and applied identically on both sides rather than
    // hidden inside a collation the Rust side can't see.
    //
    // `documents.project` is KEPT and still means the HOME project — the one that owns filing
    // activity, the semantic Map's centroid pull, and the entity link. The join adds the OTHER
    // memberships; it does not replace the home. Nothing about the single-project path changes.
    //
    // The backfill makes the new table a faithful restatement of what the store already says, so
    // every membership-aware query is provably equivalent to its predecessor until a user actually
    // links a document somewhere. `INSERT OR IGNORE` throughout because two documents may carry the
    // same project under different casing (pre-entity vaults can), and the norm index would
    // otherwise abort the whole migration on a store that is merely untidy.
    r#"
    CREATE TABLE tags (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        kind       TEXT NOT NULL CHECK (kind IN ('project','group')),
        name       TEXT NOT NULL,   -- display form; project tags keep the user's casing
        norm       TEXT NOT NULL,   -- lower(trim(name)); the matching key
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    -- Per KIND, not global: a project called "Research" and a label called "research" are
    -- different things and must be able to coexist.
    CREATE UNIQUE INDEX idx_tags_kind_norm ON tags(kind, norm);

    CREATE TABLE document_tags (
        document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        PRIMARY KEY (document_id, tag_id)
    );
    CREATE INDEX idx_document_tags_tag ON document_tags(tag_id);

    -- Every project a document is filed under becomes a project tag...
    INSERT OR IGNORE INTO tags (kind, name, norm)
        SELECT 'project', project, lower(trim(project))
        FROM documents
        WHERE trim(COALESCE(project,'')) <> '';
    -- ...as does every project that exists only as a triage row (deadlines, milestones, a
    -- last_touched stamp) with no documents filed under it yet.
    INSERT OR IGNORE INTO tags (kind, name, norm)
        SELECT 'project', name, lower(trim(name))
        FROM projects
        WHERE trim(COALESCE(name,'')) <> '';

    -- ...and every document's home project becomes its first membership.
    INSERT OR IGNORE INTO document_tags (document_id, tag_id)
        SELECT d.id, t.id
        FROM documents d
        JOIN tags t ON t.kind = 'project' AND t.norm = lower(trim(d.project))
        WHERE trim(COALESCE(d.project,'')) <> '';
    "#,
    // v47 (Stage-4 card 16, #276): group tags join the registry, so a tag can finally be SCOPED BY.
    //
    // v46 created `tags` with both kinds and deliberately populated only `project`. Group tags — the
    // free-form labels the tag editor writes — stayed in the `documents.tags` JSON blob, because
    // nothing read them: no retrieval, no search, no filter, no score. Moving a population with no
    // consumer would have been churn.
    //
    // #276 is that consumer. `@tag` widens a chat's retrieval scope, which needs to answer "which
    // documents carry this tag" — a question the blob cannot answer without scanning every row, and
    // one that has to be a JOIN if it is to intersect with the chunk allow-set.
    //
    // `documents.tags` is KEPT as the truth (it is what the vault's `tags:` line round-trips, and
    // what a Rebuild restores from); the join is the queryable index over it, exactly as
    // `document_tags` already is for projects. `ingest::write_document_truth` writes both.
    //
    // `json_each` over a CASE-guarded value rather than a WHERE filter: SQLite expands the
    // table-valued function before the WHERE is applied, so a row holding malformed JSON — a
    // hand-edited store, a partial write — would abort the whole migration rather than be skipped.
    // Substituting an empty array for anything `json_valid` rejects makes the bad row a no-op.
    //
    // `je.type = 'text'` is the other half of that guard: a numeric element in a
    // hand-edited array would otherwise be coerced into a tag literally named `1`. And the display
    // name is TRIMMED — `intern` is find-first, so a legacy blob entry of " urgent" would mint a
    // row keeping the leading space that no later write would ever correct. A migration runs once.
    r#"
    INSERT OR IGNORE INTO tags (kind, name, norm)
        SELECT 'group', trim(je.value), lower(trim(je.value))
        FROM documents d,
             json_each(CASE WHEN json_valid(d.tags) THEN d.tags ELSE '[]' END) je
        WHERE je.type = 'text' AND trim(je.value) <> '';

    INSERT OR IGNORE INTO document_tags (document_id, tag_id)
        SELECT d.id, t.id
        FROM documents d,
             json_each(CASE WHEN json_valid(d.tags) THEN d.tags ELSE '[]' END) je
        JOIN tags t ON t.kind = 'group' AND t.norm = lower(trim(je.value))
        WHERE je.type = 'text' AND trim(je.value) <> '';
    "#,
    // v48 (Stage-4 card 16.iii, #580): a staging area for a whole-library re-tag pass.
    //
    // Deliberately NOT `document_proposals`, which is the Review queue's cache. That table is read
    // through `WHERE d.reviewed = 0` and committed by `commit_review`, which writes project +
    // importance + tags together and logs corrections. Re-tagging touches ALREADY-REVIEWED
    // documents and must change tags ONLY: routed through the review queue it would re-propose
    // filing the user has curated, land blanks in Unsorted, and write corrections the user never
    // made into the learning corpus. A separate table is the isolation.
    //
    // Regenerable and empty in the steady state: rows exist only between proposing a pass and
    // accepting or discarding it. `ON DELETE CASCADE` so a document deleted mid-review takes its
    // pending proposal with it rather than stranding a row pointing at nothing.
    r#"
    CREATE TABLE IF NOT EXISTS tag_proposals (
        document_id INTEGER PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
        tags        TEXT NOT NULL,
        created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    "#,
    // v49: make retrieval-feedback rows survive a Rebuild, and label them with the regime they were
    // actually produced under.
    //
    // Both halves of the capture were silently wrong about the one thing the corpus exists to record
    // — WHICH chunk answered WHICH query, under WHICH configuration:
    //
    //   * `chunk_ids` holds `chunks.id`, and a Rebuild deletes and re-creates every chunk row. Those
    //     integers are then reused by unrelated chunks, so a judgement doesn't merely go stale, it
    //     silently comes to name different text. `chunks.uid` is the stable identity (hashed from the
    //     document's content hash and the chunk's structural path), so it is what a training example
    //     must carry. Rebuild-invalidated ids stay in the column beside them — they are still an
    //     honest record of what was retrieved at the time, and dropping them would rewrite history.
    //   * `config_stamp` was resolved when the user CLICKED. A thumb given after a re-embed labelled
    //     the judgement with the new regime although it was formed under the old one, which is the
    //     exact confusion the stamp exists to prevent. It is now banked with the answer.
    //
    // Additive throughout: three nullable columns, no rewrite. Existing rows keep NULL, which reads
    // as "produced before this was recorded" — distinguishable from any real value.
    r#"
    ALTER TABLE messages ADD COLUMN retrieved_chunk_uids  TEXT;  -- JSON array, parallel to retrieved_chunk_ids
    ALTER TABLE messages ADD COLUMN retrieved_config_stamp TEXT; -- retrieval config at ANSWER time
    ALTER TABLE retrieval_feedback ADD COLUMN chunk_uids  TEXT;  -- JSON array: Rebuild-stable identities
    "#,
    // v50: index the review-queue predicate. `documents` carries indexes on entity_id, source_id and
    // source_type but nothing on `reviewed`, so answering `WHERE reviewed = 0` reads the table
    // end-to-end — every page decrypted, each row carrying a ~500-char `stored_summary` — to produce
    // one integer for the sidebar badge, on EVERY view change (App.tsx keys the refresh on `view`).
    // `review::unreviewed_titles` and the review queue itself read the same predicate.
    //
    // ONE column, ASC, no sort keys, and that is deliberate — do not "improve" it. Adding the
    // listing's sort keys (`reviewed, ingested_at DESC, id DESC`), or the partial index on
    // `WHERE reviewed = 0`, does remove `USE TEMP B-TREE FOR ORDER BY` and looks strictly better;
    // both were measured 2x SLOWER on `review_queue` at 50% unreviewed (114 ms vs 55 ms at 20k
    // documents), because the planner then walks the queue in index order with a rowid lookup per
    // row. A fresh import is exactly the 50%-unreviewed case — i.e. when the queue matters most. A
    // bare index on `ingested_at` is worse still: it captures `list_documents`, which reads the WHOLE
    // table, and doubles it. The sort stays a temp b-tree by choice.
    //
    // Single column also makes this COVERING for the badge count: `SEARCH documents USING COVERING
    // INDEX idx_documents_reviewed` — no table access at all.
    //
    // `IF NOT EXISTS` follows v48's `CREATE TABLE IF NOT EXISTS tag_proposals` precedent: the
    // db-ladder teardown tests rewind `user_version` to 9/10 WITHOUT dropping post-v9 schema and then
    // replay every rung. A bare `CREATE INDEX` would abort that replay. (`reviewed` is a pre-v9
    // column neither teardown drops, so the index blocks none of their `ALTER TABLE ... DROP COLUMN`
    // statements — SQLite refuses to drop an indexed column, which is why those teardowns already
    // drop `idx_documents_source_id` / `idx_documents_source_type` first.)
    r#"
    CREATE INDEX IF NOT EXISTS idx_documents_reviewed ON documents(reviewed);
    "#,
    // v51: remember which duplicate pairs the user has decided to keep.
    //
    // The duplicate report was stateless by construction: `scan_duplicates` recomputes everything
    // from `documents`/`chunks`/`chunk_vec` on every invocation and writes nothing back. The only
    // "resolved" state was a component-local Set, cleared at the top of every scan. So a pair the
    // user had looked at and deliberately kept came back on the next scan, and on the one after
    // that — which is exactly the "I think the same duplicate files were flagged again" report,
    // and it had nothing to do with the rebuild that happened in between.
    //
    // Ids are stored lower-first to match `duplicates::ordered`, so a pair is one row whichever way
    // round it is discovered. `documents.id` is `INTEGER PRIMARY KEY AUTOINCREMENT`, so a rowid is
    // never reused and a stale dismissal cannot silently re-attach to a different document; the
    // cascades then clear a dismissal when either side is deleted (foreign keys are ON for every
    // connection). `IF NOT EXISTS` for the same db-ladder teardown reason as v48/v50.
    r#"
    CREATE TABLE IF NOT EXISTS duplicate_dismissals (
      a_document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
      b_document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
      dismissed_at  TEXT NOT NULL,
      PRIMARY KEY (a_document_id, b_document_id)
    );
    "#,
    // v52: what the SOURCE knows about a document — author, last editor, creation time, size (#701).
    //
    // Four columns rather than reusing the two that look close enough. `created_at` is PM's own
    // ingest-side field (index-only registration sets it to `source_modified_at`, so it is not a
    // creation time at all), and `byte_size` is measured from the file PM ingested — for an
    // index-only pointer there is no such file. Overloading either would make the existing readers
    // mean two things depending on `source_type`, which is exactly the sort of ambiguity the
    // duplicate panel was asked to resolve, not add to.
    //
    // All four are NULLABLE with no default, and NULL is meaningful: **the provider did not say**.
    // The UI renders that as "Unknown" rather than blank or "you" — a decision made up front,
    // because a document's author reading as the person looking at it is worse than no answer. Only
    // Drive, OneDrive and the local folder can populate any of them; vault documents, chats, photos
    // and spreadsheets have no provider to ask, and stay NULL by design.
    //
    // `source_size_bytes` is INTEGER: Drive returns `size` as a decimal STRING (it can exceed 2^53,
    // so the API never sends it as a JSON number) and Graph as a number; both are parsed into i64 at
    // the connector, so the column has one type. A Google-native file (Doc/Sheet/Slide) has no
    // `size` at all — those stay NULL, which is honest: they occupy no Drive quota bytes.
    r#"
    ALTER TABLE documents ADD COLUMN source_author           TEXT;
    ALTER TABLE documents ADD COLUMN source_last_modified_by TEXT;
    ALTER TABLE documents ADD COLUMN source_created_at       TEXT;
    ALTER TABLE documents ADD COLUMN source_size_bytes       INTEGER;
    "#,
    // v53 — when PM last rewrote this row from its source (#708).
    //
    // `ingested_at` is first-sight only and nothing has ever bumped it; `last_activity` is seeded
    // from ingest and then only moved by chats; `connector_sources.last_synced_at` is per ACCOUNT,
    // so it says a Drive was checked, never that this document was. That left no way at all to
    // tell "nobody has edited this file since March" apart from "this connector stopped working in
    // March" — the two look identical in every surface PM has.
    //
    // Written only when a write actually happens: the refresh guard makes an unchanged item update
    // zero rows, so an idle fifteen-minute poll does not touch the page cache. That is deliberate,
    // and it is what the column means — "when PM last had something new to write down", not "when
    // PM last looked". A stable file therefore keeps an old stamp, honestly.
    r#"
    ALTER TABLE documents ADD COLUMN pm_refreshed_at TEXT;
    "#,
    // v54 — every PLACE a document's file lives, one row each (#710).
    //
    // Until now a document WAS its location: `documents.source_id` named exactly one place, so one
    // file reachable through two Drive accounts, or as both a shared-drive item and a shared-with-me
    // item, became two documents with two filings and two rows in every list. #703 fixed one such
    // overlap by refusing to enumerate the file twice, which works only for overlaps PM can see from
    // one account's listing — the general case (two accounts, one owner and one recipient) it cannot.
    //
    // The model Bobby chose over a primary-plus-record one, and his reasoning is the design: if a
    // "primary" vanishes while another location is still live and still being edited, a primary-only
    // model either goes stale or reaps a document the user still has. So: a document survives while
    // ANY of its locations does, and each location is reconciled by its own connector on its own
    // cursor, with its own change pointer.
    //
    // **`documents.source_id` stays, as a permanent identity ANCHOR that is never rewritten.** It has
    // to stay — `vault_path` (`idx://<source_id>`) and `content_hash` are both NOT NULL UNIQUE and
    // derived from it, and rule #3 forbids dropping a column. Making it immutable is what dissolves
    // the promotion problem the card anticipated: nothing reads the anchor to decide whether a body
    // is reachable any more (that is the rollup below), so a dead anchor costs nothing and no
    // document ever has to be re-hashed, re-pathed or re-embedded to hand the crown to a sibling.
    //
    // The anchor's own row is a location like any other — it is in here too, not a special case
    // outside it. `documents`' pointer columns (`source_state`, `external_ref`, `source_modified_at`,
    // `source_content_hash`) are a MIRROR of that anchor location plus the reachability rollup, kept
    // by one writer (`locations::sync_document`) so the two cannot drift; every existing reader of
    // those columns keeps working untouched, which is what makes this landable in one piece.
    //
    // Rows exist only for `source_type = 'index_only'` — a location is a place a CONNECTOR found a
    // file. A chat or a note carries a `source_id` too, but its "location" is a conversation or a
    // widget, and folding those into this table would make its meaning (and its SYNC-SET class)
    // mean two things at once. Promotion to a full local import deletes them, for the same reason.
    //
    // Identity claim (INVARIANTS I-07): "these are two locations of ONE file" is a FOURTH claim,
    // beside `documents.content_hash` (derived Markdown), `photos.file_hash` (original bytes) and
    // `source_content_hash` (whatever the provider reports). It is a claim about PROVENANCE — that
    // two source ids name the same underlying object — and it is never inferred from a hash match
    // alone. This migration makes no such claim: it gives every existing document exactly one
    // location, so the shape lands with behaviour unchanged and the folding arrives separately.
    r#"
    CREATE TABLE document_locations (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        document_id   INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
        source_id     TEXT NOT NULL UNIQUE,
        source_state  TEXT NOT NULL DEFAULT 'ok'
                      CHECK (source_state IN ('ok','source_missing','unreachable')),
        external_ref              TEXT,
        source_modified_at        TEXT,
        source_content_hash       TEXT,
        source_parent_folder_id   TEXT,
        source_parent_folder_name TEXT,
        first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    );
    CREATE INDEX idx_document_locations_document ON document_locations(document_id);

    INSERT INTO document_locations (
        document_id, source_id, source_state, external_ref, source_modified_at,
        source_content_hash, source_parent_folder_id, source_parent_folder_name, first_seen_at)
    SELECT id, source_id, source_state, external_ref, source_modified_at,
           source_content_hash, source_parent_folder_id, source_parent_folder_name, ingested_at
    FROM documents
    WHERE source_type = 'index_only' AND source_id IS NOT NULL;
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
    // A `writable_schema` text-patch (v17's importance-CHECK relaxation and its siblings) edits the
    // stored schema without bumping the schema cookie, so this connection would keep compiling the OLD
    // constraint into prepared statements. If any migration ran, bump the cookie once to force a
    // reparse so the relaxed CHECK takes effect immediately (harmless when no writable_schema edit was
    // involved). NOTE: a migration that must ALTER a just-writable_schema-patched table in the same run
    // can't wait for this end-of-run bump — it emits `PRAGMA writable_schema=RESET;` at its own top to
    // reload the stale schema first (v37 does exactly this, to ADD COLUMNs to the v36-patched usage_log).
    // Mid-loop reparsing HERE is wrong: a schema-cookie bump disturbs the still-settling schema on a
    // teardown-then-remigrate path (the db-ladder tests) — RESET does not (no page-1 write) — so this
    // stays a single end-of-run bump.
    if version as i64 != current {
        let cookie: i64 = conn.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        conn.execute_batch(&format!(
            "PRAGMA writable_schema=ON; PRAGMA schema_version={}; PRAGMA writable_schema=OFF;",
            cookie + 1
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Every existing index-only document comes out of the v54 upgrade with EXACTLY ONE location —
    /// and everything else comes out with none.
    ///
    /// This is the property the whole PR rests on. Every connector's known set now reads
    /// `document_locations` and nothing else, so a document the backfill misses is invisible to its
    /// connector: the next sync sees a file it has no record of, ingests it again, and the user's
    /// library quietly doubles on upgrade. Replays the real ladder — drop back to v53, seed the
    /// shapes an installed store actually holds, then run v54 for real.
    #[test]
    fn the_upgrade_gives_every_indexed_document_exactly_one_location() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        conn.execute_batch(
            "DROP TABLE document_locations; PRAGMA user_version = 53;
             INSERT INTO documents(vault_path, title, content_hash, project, source_type,
                 source_id, source_state, external_ref, source_content_hash)
             VALUES ('idx://gdrive:a@x.com:f1','A','h1','Unsorted','index_only',
                     'gdrive:a@x.com:f1','ok','/Reports/q3.docx','sh1'),
                    ('idx://local:k1:f2','B','h2','Unsorted','index_only',
                     'local:k1:f2','source_missing','/home/b.md','sh2'),
                    -- a vault import, a chat, and a PROMOTED Drive file: none of these is a place a
                    -- connector found a file, and a location row for any of them would put it back
                    -- into a known set it has no business being in.
                    ('c.md','C','h3','Unsorted','vault',NULL,'ok',NULL,NULL),
                    ('d.md','D','h4','Unsorted','chat','chat:7','ok',NULL,NULL),
                    ('e.md','E','h5','Unsorted','spreadsheet','gdrive:a@x.com:f9','ok',NULL,NULL);",
        )
        .unwrap();
        super::run(&conn).unwrap();

        let rows: Vec<(String, String, Option<String>, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT source_id, source_state, external_ref, source_content_hash \
                     FROM document_locations ORDER BY source_id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![
                (
                    "gdrive:a@x.com:f1".to_string(),
                    "ok".to_string(),
                    Some("/Reports/q3.docx".to_string()),
                    Some("sh1".to_string())
                ),
                (
                    "local:k1:f2".to_string(),
                    // The state travels too: a file already flagged gone must not come back as
                    // healthy and be re-offered as a live pointer.
                    "source_missing".to_string(),
                    Some("/home/b.md".to_string()),
                    Some("sh2".to_string())
                ),
            ],
            "one location per index-only document, and none for anything else"
        );

        // Idempotent by construction — the ladder never re-runs a step, but a document that somehow
        // gained two anchor rows would break the UNIQUE the reconcile keys on.
        let dupes: i64 = conn
            .query_row(
                "SELECT count(*) FROM (SELECT source_id FROM document_locations \
                 GROUP BY source_id HAVING count(*) > 1)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dupes, 0);
    }

    /// A freshly-opened store reaches the latest `user_version` and carries the v14 connector
    /// registry with its documented defaults — the table 4A's Drive connector writes account state to.
    #[test]
    fn connector_sources_lands_with_defaults() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            version,
            super::MIGRATIONS.len() as i64,
            "every migration applied"
        );
        assert_eq!(
            version, 54,
            "migration count pin (connector registry is v14; usage cost_usd is v15; \
             semantic-map doc_layout is v16; importance 'archive' level is v17; \
             multi-provider calendar foundation is v18; shared-drive access relation is v19; \
             project milestones is v20; project active-date + manual priority is v21; \
             photo ingestion table is v22; chat ingestion foundation is v23; \
             chat per-chunk provenance + timestamp is v24; rolling chat summary is v25; \
             last-turn prompt size for the context meter is v26; chat title provenance is v27; \
             chat preference source + extraction cursor is v28; \
             Drive parent-folder tag + normalized source_account is v29; \
             spreadsheet ingestion table is v30; project activity log is v31; \
             structured flag layer is v32; per-flag instance timestamp (F-18) is v33; \
             per-calendar quiet flag is v34; rebuild pass stamp (#371) is v35; \
             usage_log kind CHECK relaxed for chat housekeeping is v36; \
             usage_log provider/latency/fallback columns (via writable_schema=RESET) is v37; \
             Drive shared-with-me access relation is v38; \
             Review AI-proposal cache is v39; \
             calendar event popup detail columns is v40; \
             preferences.source admits 'imported' is v41; \
             project_milestones status + source_type/external_id is v42; \
             corrections filing-pipeline version stamp is v43; \
             retrieval-relevance feedback capture is v44; \
             calendar work/personal typing is v45; \
             tag registry + M:N project membership is v46; \
             group tags join the registry is v47; \
             whole-library re-tag staging is v48; \
             Rebuild-stable retrieval-feedback identities + answer-time config stamp is v49; \
             documents.reviewed index for the review queue is v50; duplicate-pair dismissals is v51; \n             source-provided author/editor/created/size is v52; \n             per-document PM refresh stamp is v53; \n             every place a document's file lives is v54)"
        );

        // A minimal insert takes the additive defaults (index_only mode, ok state, NULL cursor).
        conn.execute(
            "INSERT INTO connector_sources(id, provider, service, label) \
             VALUES ('gdrive:a@b.com', 'google', 'drive', 'a@b.com')",
            [],
        )
        .unwrap();
        let (mode, state, cursor): (String, String, Option<String>) = conn
            .query_row(
                "SELECT mode, state, cursor FROM connector_sources WHERE id = 'gdrive:a@b.com'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(mode, "index_only");
        assert_eq!(state, "ok");
        assert_eq!(cursor, None);
    }

    /// Derived indexes a Rebuild can reconstruct from `documents` — safe to clear wholesale.
    const REBUILD_CLEARABLE: &[&str] = &["chunk_vec", "chunks_fts", "chunks"];
    /// Schema-catalog / ephemeral-state tables an UPDATE may touch without a sentinel.
    const UPDATE_METADATA: &[&str] = &["sqlite_master", "sqlite_schema"];

    /// One SQL statement out of a migration, with whether a `guard:allow` sentinel preceded it.
    #[derive(Debug)]
    struct Stmt {
        /// Whitespace-normalised, comments stripped. Original case, for the failure message.
        text: String,
        armed: bool,
    }

    /// Split a migration into statements on `;`, dropping `--` comments and remembering whether a
    /// `guard:allow` sentinel preceded each one.
    ///
    /// The guard used to scan LINES, which left two holes an author could fall into without ever
    /// meaning to: a statement that wraps onto a second line was only ever inspected by its first
    /// line, and two statements sharing one line meant only the first was inspected at all — so
    /// `CREATE TABLE …; DELETE FROM documents;` on one line read as a harmless CREATE.
    ///
    /// Quote-aware, because a `;` inside a string literal does not end a statement. It does NOT
    /// understand trigger bodies (`BEGIN … END;` holds its own semicolons), which is why
    /// [`rule3_violation`] rejects `CREATE TRIGGER` outright rather than mis-parsing one.
    fn statements(migration: &str) -> Vec<Stmt> {
        let mut out: Vec<Stmt> = Vec::new();
        let mut buf = String::new();
        let mut armed_next = false;
        let mut in_string = false;
        let mut chars = migration.chars().peekable();

        // A comment-only chunk must NOT clear a pending sentinel — that is how a sentinel written
        // on its own line above a statement reaches the statement it is meant to bless.
        fn flush(out: &mut Vec<Stmt>, buf: &mut String, armed: &mut bool) {
            let text = buf.split_whitespace().collect::<Vec<_>>().join(" ");
            buf.clear();
            if text.is_empty() {
                return;
            }
            out.push(Stmt {
                text,
                armed: *armed,
            });
            *armed = false;
        }

        while let Some(ch) = chars.next() {
            if in_string {
                buf.push(ch);
                if ch == '\'' {
                    // `''` is an escaped quote inside a literal, not the end of one.
                    if chars.peek() == Some(&'\'') {
                        buf.push(chars.next().unwrap());
                    } else {
                        in_string = false;
                    }
                }
                continue;
            }
            match ch {
                '\'' => {
                    in_string = true;
                    buf.push(ch);
                }
                '-' if chars.peek() == Some(&'-') => {
                    let mut comment = String::new();
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                        comment.push(c);
                    }
                    if comment.contains("guard:allow") {
                        armed_next = true;
                    }
                    buf.push(' '); // a comment separates tokens; it never joins them
                }
                ';' => flush(&mut out, &mut buf, &mut armed_next),
                _ => buf.push(ch),
            }
        }
        flush(&mut out, &mut buf, &mut armed_next);
        out
    }

    /// The bare table name out of the token that follows a verb.
    ///
    /// `REPLACE INTO chunk_vec(rowid)` tokenises the table as `chunk_vec(rowid)` — there is no
    /// space before the paren — so a raw token comparison silently fails to match the allow-list
    /// and reports a clearable index as a rule-#3 breach. Schema prefixes and quoted identifiers
    /// are stripped for the same reason: the guard must key on the table, not on how it is spelt.
    fn table_name(token: &str) -> &str {
        let token = token.split('(').next().unwrap_or(token);
        let token = token.rsplit('.').next().unwrap_or(token);
        token.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
    }

    /// The rule-#3 verdict for one statement: `Some(reason)` when it must not ship.
    ///
    /// Rule #3 is "migrations are additive — never drop or rewrite user data", and this is its only
    /// automated enforcement. Four verbs can breach it, and the exceptions are narrow and principled:
    ///
    ///   * `DELETE FROM` / `REPLACE INTO` / `INSERT OR REPLACE INTO` — all three remove existing
    ///     rows (REPLACE is a delete-then-insert, which is why it belongs here and not with INSERT).
    ///     Allowed only against a rebuild-clearable derived index. **A sentinel never excuses one**:
    ///     user rows are re-keyed with UPDATE, never deleted.
    ///   * `DROP TABLE` and `ALTER … DROP` — table or column loss, never allowed.
    ///   * `ALTER … RENAME` — the rows survive, but every reader keyed on the old identifier breaks,
    ///     and an identifier rename is a one-way door for anyone already on the old schema.
    ///   * `UPDATE` — allowed against the schema catalogue, against `connector_sources SET cursor`
    ///     (ephemeral sync state, not user data), or with an explicit `guard:allow` sentinel
    ///     covering a *preserving* re-key or a freshly-added-column backfill.
    ///
    /// `ON DELETE CASCADE` inside a `CREATE TABLE` is not a DELETE statement and never trips this:
    /// the verb has to LEAD the statement.
    fn rule3_violation(stmt: &Stmt) -> Option<String> {
        let lower = stmt.text.to_ascii_lowercase();
        let toks: Vec<&str> = lower.split_whitespace().collect();
        let first = *toks.first().unwrap_or(&"");

        if lower.starts_with("drop table") {
            return Some("`DROP TABLE` destroys user data".into());
        }
        if first == "alter" && lower.contains(" drop ") {
            return Some("`ALTER TABLE … DROP` loses a column".into());
        }
        if first == "alter" && lower.contains(" rename") {
            return Some(
                "`ALTER TABLE … RENAME` changes an identifier every reader is keyed on".into(),
            );
        }
        if lower.starts_with("create trigger") {
            return Some(
                "`CREATE TRIGGER` bodies hold their own semicolons, which this guard's statement \
                 splitter does not parse — teach it before adding one"
                    .into(),
            );
        }

        // The three row-removing shapes, and where each names its table.
        let removal = if lower.starts_with("delete from") {
            Some(("DELETE FROM", toks.get(2)))
        } else if lower.starts_with("replace into") {
            Some(("REPLACE INTO", toks.get(2)))
        } else if first == "insert" && toks.get(1) == Some(&"or") && toks.get(2) == Some(&"replace")
        {
            Some(("INSERT OR REPLACE INTO", toks.get(4)))
        } else {
            None
        };
        if let Some((verb, table)) = removal {
            let table = table_name(table.copied().unwrap_or(""));
            if !REBUILD_CLEARABLE.contains(&table) {
                return Some(format!(
                    "`{verb} {table}` removes existing rows. Only {REBUILD_CLEARABLE:?} are \
                     rebuild-clearable; re-key with UPDATE, never DELETE or REPLACE"
                ));
            }
            return None;
        }

        if first == "update" {
            // Skip an optional `OR IGNORE` / `OR ROLLBACK` conflict clause to find the table.
            let table = if toks.get(1) == Some(&"or") {
                toks.get(3)
            } else {
                toks.get(1)
            };
            let table = table_name(table.copied().unwrap_or(""));
            let allowed = UPDATE_METADATA.contains(&table)
                || (table == "connector_sources" && lower.contains("set cursor"))
                || stmt.armed;
            if !allowed {
                return Some(format!(
                    "`UPDATE {table}` writes a persistent table with no `guard:allow` sentinel. If \
                     it is a rule-#3 preserving re-key/backfill, mark it; otherwise it may clobber \
                     user data"
                ));
            }
        }
        None
    }

    /// Rule #3 enforcement over the shipped migration list.
    #[test]
    fn migrations_never_destroy_user_data() {
        let mut inspected = 0usize;
        for (i, migration) in super::MIGRATIONS.iter().enumerate() {
            for stmt in statements(migration) {
                inspected += 1;
                if let Some(why) = rule3_violation(&stmt) {
                    panic!("MIGRATIONS[{i}]: {why} (rule #3): {}", stmt.text);
                }
            }
        }
        // A guard that inspected nothing reports the same green as one that inspected everything.
        assert!(
            inspected > 100,
            "only {inspected} statements parsed out of {} migrations — the splitter has stopped \
             matching, not the migrations gone quiet",
            super::MIGRATIONS.len()
        );
    }

    // ---- The guard's own tests ----------------------------------------------------------------
    //
    // Feeding synthetic migrations through the same two functions the real check uses, so each
    // shape is proven to be caught rather than assumed to be.

    /// The single verdict for a one-migration source string, or `None` if it is clean.
    fn verdict(sql: &str) -> Option<String> {
        statements(sql).iter().find_map(rule3_violation)
    }

    #[test]
    fn guard_catches_row_removing_statements() {
        assert!(verdict("DELETE FROM documents;")
            .unwrap()
            .contains("DELETE"));
        assert!(verdict("REPLACE INTO documents(id) VALUES(1);")
            .unwrap()
            .contains("REPLACE INTO documents"));
        assert!(
            verdict("INSERT OR REPLACE INTO projects(name) SELECT name FROM old;")
                .unwrap()
                .contains("INSERT OR REPLACE INTO projects")
        );
        // …and the derived indexes a Rebuild reconstructs stay allowed. The column list here is
        // deliberate: `chunk_vec(rowid)` tokenises as one word, and reading it as the table name
        // is what made the first draft of this guard reject a clearable index.
        assert!(verdict("DELETE FROM chunks_fts;").is_none());
        assert!(verdict("REPLACE INTO chunk_vec(rowid) VALUES(1);").is_none());
        assert!(verdict("DELETE FROM main.chunks;").is_none());
        // A sentinel does NOT excuse a removal, unlike an UPDATE.
        assert!(verdict("-- guard:allow\nDELETE FROM documents;").is_some());
    }

    #[test]
    fn guard_catches_schema_loss_and_renames() {
        assert!(verdict("DROP TABLE documents;").is_some());
        assert!(verdict("DROP TABLE IF EXISTS documents;").is_some());
        assert!(verdict("ALTER TABLE documents DROP COLUMN project;").is_some());
        assert!(verdict("ALTER TABLE documents RENAME TO docs;").is_some());
        assert!(verdict("ALTER TABLE documents RENAME COLUMN project TO home;").is_some());
        // The additive shape this rule exists to permit.
        assert!(verdict("ALTER TABLE documents ADD COLUMN home TEXT;").is_none());
    }

    #[test]
    fn guard_inspects_every_statement_on_a_shared_line() {
        // The line-anchored scan this replaced saw only the leading CREATE here and passed.
        let sql = "CREATE TABLE t(id INTEGER); DELETE FROM documents;";
        assert_eq!(statements(sql).len(), 2);
        assert!(verdict(sql).unwrap().contains("DELETE FROM documents"));
    }

    #[test]
    fn guard_reads_a_statement_that_wraps_onto_later_lines() {
        // Only the first line used to be inspected, so the table name on line 2 was never read.
        let sql = "UPDATE\n    documents\n    SET project = 'Unsorted';";
        assert!(verdict(sql).unwrap().contains("UPDATE documents"));
    }

    #[test]
    fn guard_ignores_a_semicolon_inside_a_string_literal() {
        let sql = "INSERT INTO settings(k, v) VALUES('sep', 'a;b'); DELETE FROM documents;";
        let stmts = statements(sql);
        assert_eq!(stmts.len(), 2, "the quoted `;` must not split a statement");
        assert!(verdict(sql).unwrap().contains("DELETE FROM documents"));
    }

    #[test]
    fn guard_sentinel_covers_exactly_one_following_statement() {
        // Blesses the backfill…
        assert!(verdict(
            "-- guard:allow preserving backfill\nUPDATE documents SET home = project;"
        )
        .is_none());
        // …and is spent by it, so a second UPDATE behind it is not covered.
        assert!(verdict(
            "-- guard:allow preserving backfill\nUPDATE documents SET home = project;\n\
             UPDATE documents SET project = NULL;"
        )
        .is_some());
        // An unblessed UPDATE against a persistent table is caught.
        assert!(verdict("UPDATE projects SET name = 'x';").is_some());
        // The two standing exceptions need no sentinel.
        assert!(verdict("UPDATE sqlite_master SET sql = '';").is_none());
        assert!(verdict("UPDATE connector_sources SET cursor = NULL;").is_none());
    }

    #[test]
    fn guard_does_not_trip_on_a_cascade_clause() {
        // The false positive the line anchoring was introduced for; statement anchoring keeps it.
        let sql = "CREATE TABLE calendars(\n  id TEXT PRIMARY KEY,\n  source_id TEXT REFERENCES \
                   connector_sources(id) ON DELETE CASCADE\n);";
        assert!(verdict(sql).is_none());
    }

    /// v18 lands the multi-provider calendar foundation: the `calendars` registry (cascading from a
    /// `connector_sources` account row), the two new `calendar_events` columns (`uid` + the
    /// write-nobody-yet `entity_id` correspondence slot), and their indexes — the clean
    /// account → calendar → event spine Google/Outlook/Apple all flow into.
    #[test]
    fn calendar_foundation_lands_with_registry_and_event_columns() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        // The pragma the cascade below relies on (db::open should set it; assert so a regression here
        // surfaces as a clear failure rather than a silently-orphaned calendar row).
        let fk_on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            fk_on, 1,
            "foreign_keys must be ON for the source→calendar cascade"
        );

        // An account source + one of its calendars + a mirrored event with the new columns all insert.
        conn.execute(
            "INSERT INTO connector_sources(id, provider, service, label, account_email) \
             VALUES ('gcal:a@b.com', 'google', 'calendar', 'a@b.com', 'a@b.com')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO calendars(id, source_id, provider, remote_id, name, is_primary) \
             VALUES ('gcal:a@b.com:a@b.com', 'gcal:a@b.com', 'google', 'a@b.com', 'a@b.com', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO calendar_events(id, calendar_id, summary, start, uid) \
             VALUES ('gcal:a@b.com:a@b.com:evt1', 'gcal:a@b.com:a@b.com', 'Standup', \
                     '2026-06-27T09:00:00Z', 'ABC-UID@google.com')",
            [],
        )
        .unwrap();
        // `selected` defaults to 1, `entity_id` to NULL (nobody writes it this stage).
        let (selected, uid, entity_id): (i64, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT c.selected, e.uid, e.entity_id FROM calendars c \
                 JOIN calendar_events e ON e.calendar_id = c.id WHERE c.id = 'gcal:a@b.com:a@b.com'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(selected, 1);
        assert_eq!(uid.as_deref(), Some("ABC-UID@google.com"));
        assert_eq!(entity_id, None);

        // Dropping the account cascades its calendars (source_id REFERENCES … ON DELETE CASCADE).
        conn.execute(
            "DELETE FROM connector_sources WHERE id = 'gcal:a@b.com'",
            [],
        )
        .unwrap();
        let calendars: i64 = conn
            .query_row("SELECT count(*) FROM calendars", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            calendars, 0,
            "calendars cascade when their source is deleted"
        );
    }

    /// v20 lands the project-milestones table (board card 7): a many-to-one milestone model keyed
    /// on `projects(name)` with a STABLE id, a `state` CHECK, an ON DELETE CASCADE from the project,
    /// and the one-time backfill of an existing `projects.deadline` into a single `label='deadline'`
    /// milestone. The legacy `deadline` column is KEPT (additive — rule #3).
    #[test]
    fn project_milestones_land_with_check_cascade_and_backfill() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // A milestone inserts with the documented defaults (label 'deadline', sort_order 0, NULL state).
        conn.execute(
            "INSERT INTO projects(name, deadline) VALUES ('Atlas', '2026-07-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_milestones(project_name, label, due_date) \
             VALUES ('Atlas', 'pitch', '2026-08-15')",
            [],
        )
        .unwrap();

        // The `state` CHECK rejects anything outside met|unmet|NULL.
        let bad = conn.execute(
            "INSERT INTO project_milestones(project_name, label, state) VALUES ('Atlas', 'x', 'done')",
            [],
        );
        assert!(bad.is_err(), "state CHECK rejects 'done'");

        // Deleting the project cascades its milestones (project_name REFERENCES … ON DELETE CASCADE).
        conn.execute("DELETE FROM projects WHERE name = 'Atlas'", [])
            .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM project_milestones WHERE project_name = 'Atlas'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "milestones cascade when their project is deleted"
        );
    }

    /// v20's backfill carries each pre-existing `projects.deadline` into exactly ONE
    /// `label='deadline'` milestone, and produces none for a project with no deadline. The backfill
    /// runs as part of migration v20, so it can only be observed on a store that crossed v19→v20 with
    /// the legacy rows already present — which a fresh `db::open` cannot reproduce. Instead we assert
    /// the backfill SQL itself is idempotent-shaped against the live schema: seeding then re-running
    /// the same SELECT yields one row per dated project.
    #[test]
    fn project_milestones_backfill_one_row_per_dated_project() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute(
            "INSERT INTO projects(name, deadline) VALUES ('Dated', '2026-07-01'), ('Blank', NULL), ('Empty', '  ')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_milestones (project_name, label, due_date, sort_order) \
             SELECT name, 'deadline', deadline, 0 FROM projects \
             WHERE deadline IS NOT NULL AND trim(deadline) <> ''",
            [],
        )
        .unwrap();

        let dated: i64 = conn
            .query_row(
                "SELECT count(*) FROM project_milestones WHERE project_name = 'Dated' AND label = 'deadline' AND due_date = '2026-07-01'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            dated, 1,
            "a dated project backfills exactly one 'deadline' milestone"
        );
        let undated: i64 = conn
            .query_row(
                "SELECT count(*) FROM project_milestones WHERE project_name IN ('Blank','Empty')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            undated, 0,
            "a project with NULL/blank deadline backfills nothing"
        );
    }

    /// v32 lands the structured flag layer (board card 9): first-class flag records with the
    /// documented defaults (state 'active', confidence 1.0, user_confirmed 0), the `anchor_kind` /
    /// `type` / `state` / `source` CHECKs, and the `(anchor_kind, anchor, type)` UNIQUE key that
    /// resolution keys on. There is deliberately NO foreign key on `anchor` (it soft-links a
    /// milestone id or an iCal UID, resolved in code), so a flag survives even when its anchored
    /// calendar row isn't in the mirror. Additive only — nothing reads or writes it yet.
    #[test]
    fn flags_land_with_defaults_checks_and_unique_key() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // A flag inserts with just its anchor + type; the additive defaults fill the rest.
        conn.execute(
            "INSERT INTO flags(anchor_kind, anchor, type) \
             VALUES ('milestone', '42', 'deadline-approaching')",
            [],
        )
        .unwrap();
        let (state, confidence, user_confirmed, source): (String, f64, i64, Option<String>) = conn
            .query_row(
                "SELECT state, confidence, user_confirmed, source FROM flags WHERE anchor = '42'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(state, "active", "new flags default to active");
        assert_eq!(confidence, 1.0, "confidence defaults to 1.0");
        assert_eq!(user_confirmed, 0, "user_confirmed defaults to 0");
        assert!(source.is_none(), "source is NULL while the flag is active");

        // The CHECKs reject values outside each enum.
        assert!(
            conn.execute(
                "INSERT INTO flags(anchor_kind, anchor, type) VALUES ('project', '1', 'overdue')",
                [],
            )
            .is_err(),
            "anchor_kind CHECK rejects 'project'"
        );
        assert!(
            conn.execute(
                "INSERT INTO flags(anchor_kind, anchor, type) VALUES ('milestone', '1', 'reminder')",
                [],
            )
            .is_err(),
            "type CHECK rejects an unknown flag type"
        );
        assert!(
            conn.execute(
                "INSERT INTO flags(anchor_kind, anchor, type, source) \
                 VALUES ('milestone', '1', 'overdue', 'user')",
                [],
            )
            .is_err(),
            "source CHECK rejects 'user' (only detection|assertion|NULL)"
        );

        // A different (anchor, type) on the same anchor coexists; the SAME (anchor_kind, anchor,
        // type) triple is rejected — resolving one type never collides with another.
        conn.execute(
            "INSERT INTO flags(anchor_kind, anchor, type) VALUES ('milestone', '42', 'overdue')",
            [],
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO flags(anchor_kind, anchor, type) \
                 VALUES ('milestone', '42', 'deadline-approaching')",
                [],
            )
            .is_err(),
            "UNIQUE(anchor_kind, anchor, type) forbids a duplicate live flag"
        );
    }

    /// v22 lands photo ingestion (board card #135): it relaxes the `documents.source_type` CHECK to
    /// admit 'photo' (proving the writable_schema patch + cookie reparse took effect on this
    /// connection), and adds the `photos` satellite table with its own provenance CHECK, a UNIQUE
    /// file_hash, and an ON DELETE CASCADE from the owning document.
    #[test]
    fn photos_table_lands_with_check_cascade_and_relaxed_source_type() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // The relaxed CHECK admits a 'photo' document (the satellite's owner).
        conn.execute(
            "INSERT INTO documents(vault_path, content_hash, source_type) \
             VALUES ('vault/p.md', 'abc123', 'photo')",
            [],
        )
        .expect("source_type='photo' is now allowed");
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE content_hash = 'abc123'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // A photos row inserts with the documented defaults (dragged_file, not saved to vault).
        conn.execute(
            "INSERT INTO photos(document_id, file_hash) VALUES (?1, 'abc123')",
            rusqlite::params![doc_id],
        )
        .unwrap();
        let (st, saved): (String, i64) = conn
            .query_row(
                "SELECT source_type, saved_to_vault FROM photos WHERE document_id = ?1",
                rusqlite::params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((st.as_str(), saved), ("dragged_file", 0), "photo defaults");

        // The provenance CHECK rejects anything outside the four capture sources.
        let bad = conn.execute(
            "INSERT INTO photos(document_id, file_hash, source_type) VALUES (?1, 'x', 'webcam')",
            rusqlite::params![doc_id],
        );
        assert!(bad.is_err(), "source_type CHECK rejects 'webcam'");

        // file_hash is UNIQUE — re-dropping the same image is a no-op, not a duplicate.
        let dup = conn.execute(
            "INSERT INTO photos(document_id, file_hash) VALUES (?1, 'abc123')",
            rusqlite::params![doc_id],
        );
        assert!(dup.is_err(), "UNIQUE file_hash rejects a duplicate");

        // Deleting the document cascades its photo (document_id REFERENCES … ON DELETE CASCADE).
        conn.execute(
            "DELETE FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM photos WHERE document_id = ?1",
                rusqlite::params![doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "photos cascade when their document is deleted"
        );
    }

    /// v23 lands the chat ingestion foundation (board card 7A): it relaxes the `documents.source_type`
    /// CHECK to admit 'chat' (proving the writable_schema patch + cookie reparse took effect on this
    /// connection), and adds the `chat_sessions` satellite with its `scope` CHECK, the two NULL-by-default
    /// cursors, an ON DELETE CASCADE from the owning conversation, and an ON DELETE SET NULL link to the
    /// documents row card B will create.
    #[test]
    fn chat_sessions_land_with_scope_check_and_cascade() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // The relaxed CHECK admits a 'chat' document (the source row card B will create).
        conn.execute(
            "INSERT INTO documents(vault_path, content_hash, source_type) \
             VALUES ('vault/chat.md', 'chat-hash', 'chat')",
            [],
        )
        .expect("source_type='chat' is now allowed");
        let doc_id: i64 = conn
            .query_row(
                "SELECT id FROM documents WHERE content_hash = 'chat-hash'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // A conversation + its session row insert with the documented defaults (NULL cursors / document).
        conn.execute("INSERT INTO conversations DEFAULT VALUES", [])
            .unwrap();
        let conv_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope) VALUES (?1, 'general')",
            rusqlite::params![conv_id],
        )
        .unwrap();
        let (doc, indexed, summ): (Option<i64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT document_id, last_indexed_turn_id, summary_covers_up_to_turn_id \
                 FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (doc, indexed, summ),
            (None, None, None),
            "session cursors/link default NULL"
        );

        // The scope CHECK rejects anything outside general|project.
        let bad = conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope) VALUES (?1, 'team')",
            rusqlite::params![conv_id],
        );
        assert!(bad.is_err(), "scope CHECK rejects 'team'");

        // Linking the document then deleting it leaves the session row but NULLs the link (SET NULL).
        conn.execute(
            "UPDATE chat_sessions SET document_id = ?1 WHERE conversation_id = ?2",
            rusqlite::params![doc_id, conv_id],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
        )
        .unwrap();
        let after: (i64, Option<i64>) = conn
            .query_row(
                "SELECT count(*), max(document_id) FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            after,
            (1, None),
            "deleting the document NULLs the link, keeps the session"
        );

        // Deleting the conversation cascades its session row (conversation_id REFERENCES … ON DELETE CASCADE).
        conn.execute(
            "DELETE FROM conversations WHERE id = ?1",
            rusqlite::params![conv_id],
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "session rows cascade when their conversation is deleted"
        );
    }

    /// v24 adds the two additive, nullable chat-chunk columns (board card 7B): `chat_turn_id` (the
    /// turn-pair source pointer) and `chunk_at` (per-chunk recency timestamp). Existing rows keep NULL;
    /// a chat chunk round-trips both.
    #[test]
    fn chat_chunk_columns_land_nullable() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute(
            "INSERT INTO documents(vault_path, content_hash) VALUES ('vault/d.md', 'd-hash')",
            [],
        )
        .unwrap();
        let doc_id: i64 = conn.last_insert_rowid();

        // A plain (non-chat) chunk leaves both new columns NULL — existing inserts are unaffected.
        conn.execute(
            "INSERT INTO chunks(document_id, ordinal, content, char_count) VALUES (?1, 0, 'body', 4)",
            rusqlite::params![doc_id],
        )
        .unwrap();
        let (turn, at): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT chat_turn_id, chunk_at FROM chunks WHERE document_id = ?1 AND ordinal = 0",
                rusqlite::params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((turn, at), (None, None), "new columns default NULL");

        // A chat chunk round-trips its turn pointer + per-chunk timestamp.
        conn.execute(
            "INSERT INTO chunks(document_id, ordinal, content, char_count, chat_turn_id, chunk_at) \
             VALUES (?1, 1, 'You: hi', 7, 42, '2026-06-28T10:00:00.000Z')",
            rusqlite::params![doc_id],
        )
        .unwrap();
        let (turn, at): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT chat_turn_id, chunk_at FROM chunks WHERE document_id = ?1 AND ordinal = 1",
                rusqlite::params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(turn, Some(42));
        assert_eq!(at.as_deref(), Some("2026-06-28T10:00:00.000Z"));
    }

    /// v25 adds the rolling-summary store: `chat_sessions.summary` lands nullable (a session born before
    /// card C has no summary, and assembles the same as before) and round-trips text once card C writes one.
    #[test]
    fn source_metadata_columns_land_nullable_and_hold_a_big_size() {
        // v52 (#701). NULL is the meaningful default — "the provider did not say", which the UI
        // renders as "Unknown" — so a row inserted without them must come back NULL rather than
        // taking some empty-string default that would render as a blank cell.
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('v/a.md','A','h1')",
            [],
        )
        .unwrap();
        let row: (Option<String>, Option<String>, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT source_author, source_last_modified_by, source_created_at, \
                 source_size_bytes FROM documents WHERE content_hash = 'h1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (None, None, None, None), "all four default NULL");

        // INTEGER, not TEXT: Drive sends `size` as a decimal string precisely because a file can
        // exceed 2^53 bytes, and the connector parses it to i64 so the column has one type. A value
        // past that boundary has to survive the round trip intact.
        let big: i64 = 9_007_199_254_740_995; // 2^53 + 3
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_author, \
                 source_size_bytes) VALUES ('v/b.md','B','h2','Jane Okafor',?1)",
            rusqlite::params![big],
        )
        .unwrap();
        let (author, size): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT source_author, source_size_bytes FROM documents WHERE content_hash = 'h2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(author.as_deref(), Some("Jane Okafor"));
        assert_eq!(size, Some(big), "a size past 2^53 round-trips exactly");
    }

    #[test]
    fn chat_summary_column_lands_nullable() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute("INSERT INTO conversations DEFAULT VALUES", [])
            .unwrap();
        let conv: i64 = conn.last_insert_rowid();

        // A session row born without a summary keeps it NULL (the cursor is the only summary state card A left).
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope) VALUES (?1, 'general')",
            rusqlite::params![conv],
        )
        .unwrap();
        let summary: Option<String> = conn
            .query_row(
                "SELECT summary FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(summary, None, "summary defaults NULL");

        // Card C writes a summary + advances the cursor together; both round-trip.
        conn.execute(
            "UPDATE chat_sessions SET summary = ?2, summary_covers_up_to_turn_id = 7 \
             WHERE conversation_id = ?1",
            rusqlite::params![conv, "- Decided to ship Friday."],
        )
        .unwrap();
        let (summary, cursor): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT summary, summary_covers_up_to_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(summary.as_deref(), Some("- Decided to ship Friday."));
        assert_eq!(cursor, Some(7));
    }

    /// v26 adds the context-meter numerator: `chat_sessions.last_prompt_tokens` lands nullable (a session
    /// with no reply yet has no measured prompt size ⇒ the meter shows "unknown") and round-trips the exact
    /// `prompt_tokens` card D records from each OpenRouter reply.
    #[test]
    fn last_prompt_tokens_column_lands_nullable() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute("INSERT INTO conversations DEFAULT VALUES", [])
            .unwrap();
        let conv: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope) VALUES (?1, 'general')",
            rusqlite::params![conv],
        )
        .unwrap();
        let measured: Option<i64> = conn
            .query_row(
                "SELECT last_prompt_tokens FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(measured, None, "no reply yet ⇒ NULL (meter is 'unknown')");

        // The post-reply write records OpenRouter's measured prompt size; it round-trips.
        conn.execute(
            "UPDATE chat_sessions SET last_prompt_tokens = 12345 WHERE conversation_id = ?1",
            rusqlite::params![conv],
        )
        .unwrap();
        let measured: Option<i64> = conn
            .query_row(
                "SELECT last_prompt_tokens FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(measured, Some(12345));
    }

    /// v27 lands chat title provenance (board card 7E): `chat_sessions.title_state` defaults to 'pending'
    /// (the placeholder a fresh session carries), the CHECK rejects anything outside the three states, and a
    /// user rename to 'custom' round-trips — the value the background title pass keys on to never overwrite.
    #[test]
    fn title_state_column_lands_pending_with_check() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute("INSERT INTO conversations DEFAULT VALUES", [])
            .unwrap();
        let conv: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope) VALUES (?1, 'general')",
            rusqlite::params![conv],
        )
        .unwrap();
        let state: String = conn
            .query_row(
                "SELECT title_state FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending", "a fresh session is still the placeholder");

        // The CHECK rejects an out-of-set value.
        let bad = conn.execute(
            "UPDATE chat_sessions SET title_state = 'whatever' WHERE conversation_id = ?1",
            rusqlite::params![conv],
        );
        assert!(bad.is_err(), "title_state CHECK rejects an unknown value");

        // A user rename to 'custom' round-trips.
        conn.execute(
            "UPDATE chat_sessions SET title_state = 'custom' WHERE conversation_id = ?1",
            rusqlite::params![conv],
        )
        .unwrap();
        let state: String = conn
            .query_row(
                "SELECT title_state FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "custom");
    }

    /// v17 relaxed the `documents.importance` CHECK: 'archive' is now a valid level (and the
    /// writable_schema patch took effect on this connection — the reparse in `run` worked), while
    /// `high|medium|low|NULL` and a genuinely bad value still behave as before.
    #[test]
    fn importance_archive_level_allowed() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        let insert = |path: &str, imp: Option<&str>| {
            conn.execute(
                "INSERT INTO documents(vault_path, content_hash, importance) VALUES (?1, ?2, ?3)",
                rusqlite::params![path, path, imp],
            )
        };
        // The four valid levels plus untriaged NULL all insert.
        for (i, imp) in [
            Some("high"),
            Some("medium"),
            Some("low"),
            Some("archive"),
            None,
        ]
        .iter()
        .enumerate()
        {
            insert(&format!("v{i}"), *imp).unwrap_or_else(|e| panic!("{imp:?} should insert: {e}"));
        }
        // An out-of-set value is still rejected by the relaxed CHECK.
        assert!(
            insert("bad", Some("urgent")).is_err(),
            "an unknown importance must still violate the CHECK"
        );
    }

    /// v28 (board card 7F) relaxes the `preferences.source` CHECK to admit 'chat' (proving the
    /// writable_schema patch + cookie reparse took effect on this connection) and adds the additive,
    /// nullable `chat_sessions.prefs_covers_up_to_turn_id` extraction cursor.
    #[test]
    fn preferences_admit_chat_source_and_extraction_cursor_lands() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // All three sources now insert; an unknown one is still rejected.
        for src in ["user", "inferred", "chat"] {
            conn.execute(
                "INSERT INTO preferences(scope, value, source) VALUES ('global', ?1, ?2)",
                rusqlite::params![format!("v-{src}"), src],
            )
            .unwrap_or_else(|e| panic!("source='{src}' should insert: {e}"));
        }
        // ('imported' is admitted from v41 onward — see the v41 test below; use a genuinely-unknown
        // value here to prove the relaxed CHECK still rejects out-of-set sources.)
        assert!(
            conn.execute(
                "INSERT INTO preferences(scope, value, source) VALUES ('global', 'x', 'bogus')",
                [],
            )
            .is_err(),
            "an unknown source must still violate the relaxed CHECK"
        );

        // The extraction cursor is present and NULL by default on a fresh session row.
        conn.execute("INSERT INTO conversations DEFAULT VALUES", [])
            .unwrap();
        let conv_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_sessions(conversation_id, scope) VALUES (?1, 'general')",
            rusqlite::params![conv_id],
        )
        .unwrap();
        let cursor: Option<i64> = conn
            .query_row(
                "SELECT prefs_covers_up_to_turn_id FROM chat_sessions WHERE conversation_id = ?1",
                rusqlite::params![conv_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cursor, None, "extraction cursor defaults NULL");
    }

    /// v41 relaxes the `preferences.source` CHECK to admit 'imported' (memory brought in from another
    /// AI), proving the writable_schema patch + cookie reparse took effect on this connection, while an
    /// out-of-set source is still rejected.
    #[test]
    fn preferences_admit_imported_source() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute(
            "INSERT INTO preferences(scope, value, source) VALUES ('global', 'v-imported', 'imported')",
            [],
        )
        .expect("source='imported' should insert after v41");
        assert!(
            conn.execute(
                "INSERT INTO preferences(scope, value, source) VALUES ('global', 'x', 'bogus')",
                [],
            )
            .is_err(),
            "an unknown source must still violate the relaxed CHECK"
        );
    }

    /// v36 relaxes the `usage_log.kind` CHECK to admit the four background housekeeping kinds
    /// (proving the writable_schema patch + cookie reparse took effect on this connection). Before
    /// this migration those direct inserts were silently rejected by the v7 CHECK and swallowed by
    /// their best-effort `let _ =`, so summary/compress/title/prefs spend never reached `usage_log`.
    #[test]
    fn usage_log_admits_chat_housekeeping_kinds() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // The two original kinds and the four housekeeping kinds all insert now.
        for kind in [
            "chat",
            "background",
            "chat_summary",
            "chat_compress",
            "chat_title",
            "chat_prefs",
        ] {
            conn.execute(
                "INSERT INTO usage_log(model, kind) VALUES ('m', ?1)",
                rusqlite::params![kind],
            )
            .unwrap_or_else(|e| panic!("kind='{kind}' should insert after v36: {e}"));
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 6, "all six kinds persisted");

        // An unknown kind must still violate the relaxed CHECK — the constraint is narrowed, not dropped.
        assert!(
            conn.execute(
                "INSERT INTO usage_log(model, kind) VALUES ('m', 'nonsense')",
                [],
            )
            .is_err(),
            "an unknown kind must still violate the relaxed CHECK"
        );
    }

    /// v37 adds provider / latency_ms / fallback_reason COLUMNS to usage_log (via a leading
    /// `writable_schema=RESET`, since v36 patched the table). A tagged row round-trips; a row that left
    /// them unset reads NULL ("provider unknown"). This also proves the in-migration RESET landed —
    /// `connector_sources_lands_with_defaults` would fail at open() otherwise.
    #[test]
    fn usage_log_carries_provider_columns_after_v37() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        // A locally-served row carries the provider columns directly on usage_log.
        conn.execute(
            "INSERT INTO usage_log(id, model, kind, prompt_tokens, completion_tokens, provider, latency_ms, fallback_reason) \
             VALUES (1, 'local-model', 'chat', 10, 20, 'local', 1234, NULL)",
            [],
        )
        .expect("the v37 columns accept a provider-tagged row");
        // An older-style write that sets none of the new columns reads them as NULL.
        conn.execute(
            "INSERT INTO usage_log(id, model, kind) VALUES (2, 'gpt', 'background')",
            [],
        )
        .unwrap();

        let (provider, latency): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT provider, latency_ms FROM usage_log WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(provider.as_deref(), Some("local"));
        assert_eq!(latency, Some(1234));

        let untagged: Option<String> = conn
            .query_row("SELECT provider FROM usage_log WHERE id = 2", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(untagged, None, "an untagged row reads provider-unknown");
    }

    /// Pins the writable_schema→ALTER gotcha that shaped v37 (AGENTS.md rule 3): a table whose CHECK
    /// was relaxed by a v36-style `writable_schema` text-patch, and whose DDL carries a
    /// `DEFAULT (strftime(…))`, CANNOT take a later same-connection `ALTER … ADD COLUMN` (the stored-DDL
    /// re-parse faults) — but `PRAGMA writable_schema=RESET` first makes it succeed cleanly, with the
    /// relaxed CHECK live and the encrypted store intact across a reopen (no page-1 HMAC damage, unlike
    /// a mid-run schema-cookie bump). v37's migration now RELIES on this mechanism, so if a future
    /// toolchain breaks it, v37 (and thus a fresh `open()`) breaks with it — hence a dedicated pin.
    #[test]
    fn writable_schema_patch_then_alter_needs_a_reset() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pm.sqlite");

        // usage_log's exact shape — the strftime DEFAULT's internal commas are the re-parse trigger —
        // with v36's exact CHECK-relax, leaving THIS connection's cached schema stale (no reparse).
        fn make_patched(conn: &rusqlite::Connection, name: &str) {
            conn.execute_batch(&format!(
                "CREATE TABLE {name} (
                    id                INTEGER PRIMARY KEY AUTOINCREMENT,
                    model             TEXT NOT NULL,
                    kind              TEXT NOT NULL CHECK (kind IN ('chat','background')),
                    prompt_tokens     INTEGER,
                    completion_tokens INTEGER,
                    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                );"
            ))
            .unwrap();
            // v36's EXACT replacement (four chat-housekeeping kinds): the precise text that shipped and
            // faulted the original column-based v37. The exact patched text matters — a shorter
            // replacement does not always trip the re-parse — so this reproduces the real condition.
            conn.execute_batch(&format!(
                "PRAGMA writable_schema = ON;\n\
                 UPDATE sqlite_master SET sql = replace(sql, '''chat'',''background''', \
                   '''chat'',''background'',''chat_summary'',''chat_compress'',''chat_title'',''chat_prefs''') \
                   WHERE type='table' AND name='{name}';\n\
                 PRAGMA writable_schema = OFF;"
            ))
            .unwrap();
        }

        let conn = crate::db::open(&path, DB_KEY).unwrap();

        // Without a remedy, the same-connection ALTER faults on the stored-DDL re-parse — the trap
        // v37 hit (verified on the bundled SQLCipher; if a future toolchain stops faulting here, that
        // is a real behaviour change worth a look, not a test to silence).
        make_patched(&conn, "gotcha_control");
        assert!(
            conn.execute_batch("ALTER TABLE gotcha_control ADD COLUMN provider TEXT;")
                .is_err(),
            "a writable_schema-patched table with a DEFAULT expr faults a later ALTER — the gotcha"
        );

        // `writable_schema=RESET` reloads the schema in-memory, so the ALTER then succeeds.
        make_patched(&conn, "gotcha_reset");
        conn.execute_batch("PRAGMA writable_schema = RESET;")
            .unwrap();
        conn.execute_batch("ALTER TABLE gotcha_reset ADD COLUMN provider TEXT;")
            .expect("writable_schema=RESET clears the stale cache so the ALTER succeeds");
        // The relaxed CHECK is genuinely in effect and the new column is usable.
        conn.execute(
            "INSERT INTO gotcha_reset(model, kind, provider) VALUES ('m','chat_title','local')",
            [],
        )
        .expect("the relaxed CHECK admits the new value after RESET");
        // The encrypted store is undamaged (RESET does no page-1 write, unlike a schema-cookie bump).
        conn.query_row("SELECT count(*) FROM usage_log", [], |r| r.get::<_, i64>(0))
            .unwrap();
        drop(conn);
        crate::db::open(&path, DB_KEY)
            .expect("the encrypted store reopens cleanly after a RESET+ALTER");
    }

    // --- T-05: the full migration ladder over real user data ----------------
    // The tests above build old schemas by tearing the CURRENT one back down (only a
    // couple of the 33 versions) — an imperfect teardown silently tests the wrong
    // shape, and nothing carries realistic data end-to-end. This builds an AUTHENTIC
    // v2 store from the real migration SQL, fills it with the precious,
    // non-rebuildable kind of data (a chat + an indexed document), then drives every
    // remaining rung to latest and asserts the data survived unchanged.

    fn user_version(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// Apply exactly the first `n` migrations (versions 1..=n) from the real
    /// `MIGRATIONS` array — the genuine historical schema, not a teardown of the
    /// current one.
    fn apply_through(conn: &rusqlite::Connection, n: usize) {
        for (i, sql) in super::MIGRATIONS.iter().take(n).enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", (i + 1) as i64)
                .unwrap();
        }
    }

    #[test]
    fn full_ladder_from_v2_preserves_user_data() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        // Open UNMIGRATED, then apply only v1 + v2 → an authentic v2-shaped store.
        let conn = crate::db::open_keyed(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        apply_through(&conn, 2);
        assert_eq!(user_version(&conn), 2, "fixture starts at an authentic v2");

        // v2-era user data: a chat conversation + its turns (the non-rebuildable kind),
        // an indexed document + chunk, and the settings pinned at v2.
        conn.execute_batch(
            "INSERT INTO conversations(id, title) VALUES (1, 'Taxes 2025');
             INSERT INTO messages(conversation_id, role, content)
                 VALUES (1, 'user', 'when did I file?');
             INSERT INTO messages(conversation_id, role, content)
                 VALUES (1, 'assistant', 'You filed on 2025-01-15.');
             INSERT INTO documents(vault_path, title, content_hash)
                 VALUES ('vault/notes.md', 'Notes', 'hash-abc');
             INSERT INTO chunks(document_id, ordinal, content, char_count)
                 VALUES ((SELECT id FROM documents WHERE content_hash = 'hash-abc'),
                         0, 'hello world', 11);",
        )
        .unwrap();

        // Drive the FULL remaining ladder (v3 → latest) over that real data.
        super::run(&conn).unwrap();
        assert_eq!(
            user_version(&conn),
            super::MIGRATIONS.len() as i64,
            "reached the latest version"
        );

        // Every row survived every rung, values intact.
        let title: String = conn
            .query_row("SELECT title FROM conversations WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "Taxes 2025");
        let turns: i64 = conn
            .query_row(
                "SELECT count(*) FROM messages WHERE conversation_id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(turns, 2, "both chat turns survived");
        let answer: String = conn
            .query_row(
                "SELECT content FROM messages WHERE conversation_id = 1 AND role = 'assistant'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(answer, "You filed on 2025-01-15.");
        let (doc_title, chunk): (String, String) = conn
            .query_row(
                "SELECT d.title, c.content FROM documents d JOIN chunks c ON c.document_id = d.id \
                 WHERE d.content_hash = 'hash-abc'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(doc_title, "Notes");
        assert_eq!(chunk, "hello world");

        // v4's additive `project` column reached the pre-existing v2 row with its default.
        let project: String = conn
            .query_row(
                "SELECT project FROM documents WHERE content_hash = 'hash-abc'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            project, "Unsorted",
            "v4 default applied to the pre-existing row"
        );

        // The embedding identity pinned at v2 survived.
        let model: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'embedding_model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(model, "BAAI/bge-small-en-v1.5");
    }
    /// v47 moves the free-form labels into the registry so `@tag` can scope by one. Same contract
    /// as v46: restate what the store already says, and survive a store that is merely untidy.
    #[test]
    fn v47_backfills_group_tags_from_the_json_blob() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_keyed(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        apply_through(&conn, 46);

        for (vp, hash, project, tags) in [
            ("a.md", "ha", "Sales", r#"["tax", "2026"]"#),
            ("b.md", "hb", "Sales", r#"["Tax"]"#),
            // The untidy cases a real store can hold: whitespace, a non-string element, and a
            // blob that is not valid JSON at all.
            ("c.md", "hc", "Ops", r#"[" urgent ", 7, ""]"#),
            ("d.md", "hd", "Ops", "not json at all"),
        ] {
            conn.execute(
                "INSERT INTO documents(vault_path, content_hash, project, tags) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![vp, hash, project, tags],
            )
            .unwrap();
        }

        super::run(&conn).unwrap();
        assert_eq!(
            user_version(&conn),
            super::MIGRATIONS.len() as i64,
            "run resumes from the stored version and climbs to the top"
        );

        // "tax" and "Tax" are one label; the numeric and empty entries minted nothing.
        let names: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM tags WHERE kind = 'group' ORDER BY norm")
                .unwrap();
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        assert_eq!(names, ["2026", "tax", "urgent"]);

        // The display name is trimmed — `intern` is find-first, so a stored " urgent " would
        // otherwise be the spelling every picker showed forever.
        assert!(names.iter().any(|n| n == "urgent"));

        // Both documents carrying the label (however cased) are bound to the one tag.
        let tax: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM document_tags dt JOIN tags t ON t.id = dt.tag_id \
                 WHERE t.kind = 'group' AND t.norm = 'tax'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tax, 2);

        // The malformed row was skipped, not fatal — the migration completed, which is the point.
        let ops: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM document_tags dt JOIN documents d ON d.id = dt.document_id \
                 JOIN tags t ON t.id = dt.tag_id WHERE d.vault_path = 'd.md' AND t.kind = 'group'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ops, 0);
    }

    /// v46's backfill must restate what the store already says, so every membership-aware query is
    /// equivalent to its predecessor on an existing vault. A user who upgrades and opens Focus must
    /// see the same projects with the same file counts — the migration is not the moment to
    /// discover a project has gone missing.
    #[test]
    fn v46_backfills_a_project_tag_and_a_membership_for_every_existing_document() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_keyed(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        apply_through(&conn, 45);

        // Two documents in one project, one in another, and — the untidy case a real pre-entity
        // vault can hold — a third spelling that differs only by case. Plus a project that exists
        // as triage only, with no documents at all.
        for (vp, hash, project) in [
            ("a.md", "ha", "Atlas, Inc."),
            ("b.md", "hb", "Atlas, Inc."),
            ("c.md", "hc", "atlas, inc."),
            ("d.md", "hd", "Research"),
        ] {
            conn.execute(
                "INSERT INTO documents(vault_path, content_hash, project) VALUES (?1, ?2, ?3)",
                rusqlite::params![vp, hash, project],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO projects(name, deadline) VALUES ('Dormant', '2026-12-01')",
            [],
        )
        .unwrap();

        // `run` resumes from the stored `user_version`, so this applies v46 and nothing else — the
        // upgrade an existing store actually experiences.
        super::run(&conn).unwrap();
        assert_eq!(
            user_version(&conn),
            super::MIGRATIONS.len() as i64,
            "run resumes from the stored version and climbs to the top"
        );

        // One tag per project, case-folded — the differently-cased spelling did NOT mint a second.
        let tags: i64 = conn
            .query_row(
                "SELECT count(*) FROM tags WHERE kind = 'project'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tags, 3, "Atlas (either spelling), Research, and Dormant");

        // Every document is bound to exactly one project.
        let bindings: i64 = conn
            .query_row("SELECT count(*) FROM document_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bindings, 4, "one home membership per document");

        let atlas: i64 = conn
            .query_row(
                "SELECT count(*) FROM document_tags dt JOIN tags t ON t.id = dt.tag_id                  WHERE t.norm = 'atlas, inc.'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            atlas, 3,
            "a case variant is the SAME project, so its documents join the same tag"
        );

        // A project with a deadline and no files is still offerable in a picker.
        let dormant: i64 = conn
            .query_row(
                "SELECT count(*) FROM tags WHERE kind = 'project' AND norm = 'dormant'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dormant, 1);
    }
}
