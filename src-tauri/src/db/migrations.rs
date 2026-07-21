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
    //     added by v11 — and only on `documents`.
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
    //     text only; `run` then bumps the schema cookie so this connection reparses the new constraint).
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
    //     cookie bump in `run` reparses the relaxed constraint on this connection.
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
    //     this CHECK — and only on `documents`.
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
    r#"
    PRAGMA writable_schema = ON;
    UPDATE sqlite_master
       SET sql = replace(sql, '''chat'',''background''', '''chat'',''background'',''chat_summary'',''chat_compress'',''chat_title'',''chat_prefs''')
     WHERE type = 'table' AND name = 'usage_log';
    PRAGMA writable_schema = OFF;
    "#,
    // v37 (#297 live local provider): tag each usage row with how it was served, so the Usage & cost
    // table and the Local AI tab can tell local from cloud spend and show local latency/throughput.
    //
    // A SATELLITE table (1:1 with usage_log, cascading on delete), NOT new columns on usage_log. Why:
    // v36 relaxed usage_log's CHECK via a `writable_schema` text-patch that leaves this connection's
    // cached definition of usage_log stale, so an `ALTER usage_log ADD COLUMN` would regenerate the
    // table from that stale definition and fail ("near ',': syntax error"). A rebuild would need a
    // `DROP TABLE` (the additive-migrations guard forbids it). The satellite sidesteps both — it is a
    // plain additive CREATE that never touches usage_log's schema, matching PM's existing satellite
    // pattern (v30 spreadsheets). A row is absent for pre-v37 spend and for any write that fails
    // best-effort — read it with a LEFT JOIN, treating absent as "provider unknown".
    r#"
    CREATE TABLE usage_meta (
        usage_id        INTEGER PRIMARY KEY REFERENCES usage_log(id) ON DELETE CASCADE,
        provider        TEXT,     -- 'local' | 'cloud'
        latency_ms      INTEGER,  -- wall-clock of the serving leg, milliseconds
        fallback_reason TEXT      -- why cloud served instead of the preferred local; NULL = none
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
    // A `writable_schema` text-patch (v17's importance-CHECK relaxation and its siblings) edits the
    // stored schema without bumping the schema cookie, so this connection would keep compiling the OLD
    // constraint into prepared statements. If any migration ran, bump the cookie once to force a
    // reparse so the relaxed CHECK takes effect immediately (harmless when no writable_schema edit was
    // involved). NOTE: a migration that must ALTER a just-writable_schema-patched table forces its own
    // reparse inline (see v37) — mid-loop reparsing here disturbs the still-settling schema on a
    // teardown-then-remigrate path (the db-ladder tests), so this stays a single end-of-run bump.
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
            version, 37,
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
             usage_log provider/latency/fallback columns for the local provider is v37)"
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

    /// Rule #3 enforcement: a migration must never silently drop, clear, or rewrite user data. This
    /// walks every migration statement and rejects a statement-LEADING destructive verb against a
    /// persistent user-data table. It anchors on the first keyword of each line (not a substring), so
    /// an `ON DELETE CASCADE` constraint clause inside a `CREATE TABLE` never trips it — that DELETE
    /// is mid-line. It would have caught v19's original `DELETE FROM documents`.
    ///
    /// Three narrow, principled exceptions:
    ///   * `DELETE FROM` a rebuild-clearable derived index (`chunk_vec` / `chunks_fts` / `chunks`) — a
    ///     Rebuild reconstructs these from `documents`, so clearing them loses nothing durable.
    ///   * `UPDATE sqlite_master` (a `writable_schema` CHECK-relaxation patch: edits stored DDL text,
    ///     moves no user rows) or `UPDATE connector_sources SET cursor` (a re-baseline reset — cursors
    ///     are ephemeral sync state, not user data).
    ///   * a statement a human has explicitly blessed with a `guard:allow` sentinel comment on the
    ///     line(s) immediately above it — used ONLY for a rule-#3 *preserving* UPDATE (a re-key, or a
    ///     freshly-added-column backfill that overwrites nothing). A DELETE/DROP is never excusable
    ///     this way: user rows are re-keyed with UPDATE, never deleted.
    #[test]
    fn migrations_never_destroy_user_data() {
        // Derived indexes a Rebuild can reconstruct from `documents` — safe to `DELETE FROM`.
        const REBUILD_CLEARABLE: &[&str] = &["chunk_vec", "chunks_fts", "chunks"];
        // Schema-catalog / ephemeral-state tables an UPDATE may touch without a sentinel.
        const UPDATE_METADATA: &[&str] = &["sqlite_master", "sqlite_schema"];

        for (i, migration) in super::MIGRATIONS.iter().enumerate() {
            // A `guard:allow` sentinel comment arms the *next* statement line, then is consumed.
            let mut armed = false;
            for raw in migration.lines() {
                let line = raw.trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with("--") {
                    if line.contains("guard:allow") {
                        armed = true;
                    }
                    continue; // comments never disarm a pending sentinel
                }
                let lower = line.to_ascii_lowercase();
                let first = lower.split_whitespace().next().unwrap_or("");

                // DROP TABLE / ALTER … DROP: table or column loss, never allowed in a migration.
                assert!(
                    !lower.starts_with("drop table"),
                    "MIGRATIONS[{i}]: `DROP TABLE` destroys user data (rule #3): {line}"
                );
                assert!(
                    !(first == "alter" && lower.contains(" drop ")),
                    "MIGRATIONS[{i}]: `ALTER TABLE … DROP` loses a column (rule #3): {line}"
                );

                // DELETE FROM <t>: only the rebuild-clearable derived indexes. A sentinel does NOT
                // excuse deleting user rows — re-key with UPDATE instead.
                if lower.starts_with("delete from") {
                    let table = lower.split_whitespace().nth(2).unwrap_or("");
                    assert!(
                        REBUILD_CLEARABLE.contains(&table),
                        "MIGRATIONS[{i}]: `DELETE FROM {table}` removes user rows (rule #3). Only \
                         {REBUILD_CLEARABLE:?} are rebuild-clearable; re-key with UPDATE, never DELETE."
                    );
                    armed = false;
                    continue;
                }

                // UPDATE <t>: metadata / cursor resets, or an explicitly-blessed preserving write.
                if first == "update" {
                    // Skip an optional `OR IGNORE` / `OR REPLACE` conflict clause to find the table.
                    let mut toks = lower.split_whitespace().skip(1).peekable();
                    if toks.peek() == Some(&"or") {
                        toks.next();
                        toks.next();
                    }
                    let table = toks.next().unwrap_or("");
                    let allowed = UPDATE_METADATA.contains(&table)
                        || (table == "connector_sources" && lower.contains("set cursor"))
                        || armed;
                    assert!(
                        allowed,
                        "MIGRATIONS[{i}]: `UPDATE {table}` writes a persistent table with no \
                         `guard:allow` sentinel. If it is a rule-#3 preserving re-key/backfill, mark \
                         it; otherwise it may clobber user data: {line}"
                    );
                    armed = false;
                    continue;
                }

                // Any other statement (CREATE / INSERT / ALTER … ADD / PRAGMA / SELECT …) consumes a
                // pending sentinel without using it, so a sentinel only ever covers the next line.
                armed = false;
            }
        }
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
        assert!(
            conn.execute(
                "INSERT INTO preferences(scope, value, source) VALUES ('global', 'x', 'imported')",
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

    /// v37 adds the `usage_meta` satellite (provider / latency_ms / fallback_reason, 1:1 with
    /// usage_log, cascading). A tagged row round-trips; a usage_log row with no satellite reads as
    /// "provider unknown" via LEFT JOIN; deleting the usage row cascades the satellite away.
    #[test]
    fn usage_meta_satellite_lands_and_cascades() {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();

        conn.execute(
            "INSERT INTO usage_log(id, model, kind, prompt_tokens, completion_tokens) \
             VALUES (1, 'local-model', 'chat', 10, 20)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_meta(usage_id, provider, latency_ms, fallback_reason) \
             VALUES (1, 'local', 1234, NULL)",
            [],
        )
        .expect("a provider-tagged satellite row inserts after v37");
        // A cloud fallback row: no satellite for it yet, so the LEFT JOIN reads provider NULL.
        conn.execute(
            "INSERT INTO usage_log(id, model, kind) VALUES (2, 'gpt', 'background')",
            [],
        )
        .unwrap();

        let (provider, latency): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT m.provider, m.latency_ms FROM usage_log u \
                 LEFT JOIN usage_meta m ON m.usage_id = u.id WHERE u.id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(provider.as_deref(), Some("local"));
        assert_eq!(latency, Some(1234));

        let untagged: Option<String> = conn
            .query_row(
                "SELECT m.provider FROM usage_log u LEFT JOIN usage_meta m ON m.usage_id = u.id WHERE u.id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            untagged, None,
            "a row with no satellite reads as provider-unknown"
        );

        // Deleting the usage row cascades its satellite away (foreign_keys is ON in db::open).
        conn.execute("DELETE FROM usage_log WHERE id = 1", [])
            .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT count(*) FROM usage_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "the satellite cascades on usage_log delete");
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
}
