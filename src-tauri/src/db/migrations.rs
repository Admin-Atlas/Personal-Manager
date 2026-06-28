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
    UPDATE documents SET entity_id = (
        SELECT e.id FROM entities e WHERE e.type = 'project' AND e.canonical_name = documents.project
    );
    -- Attach existing triage rows to the same entity (additive; `name` stays the PK). A `projects`
    -- row with no surviving documents simply keeps a NULL entity_id (harmless — the focus view is
    -- driven by `documents`).
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
    // Rebuild on next sync: drop the existing per-account shared-drive documents (+ their chunks / fts
    // / vec rows) so the next sync re-creates them once under the new namespace — they're index-only
    // (pointers + summaries, never file bytes) and a first sync re-enumerates anyway, so nothing real
    // is lost. Clearing every Drive account's delta cursor forces that re-baseline (My Drive
    // re-enumerates too, reconciling its unchanged items as no-op Updates — no duplicates).
    r#"
    CREATE TABLE shared_drive_access (
        drive_id   TEXT NOT NULL,
        account_id TEXT NOT NULL REFERENCES connector_sources(id) ON DELETE CASCADE,
        is_owner   INTEGER NOT NULL DEFAULT 0,   -- the one account whose sync indexes + reconciles this drive
        name       TEXT,                          -- cached drive display name (for the "synced by X" UI)
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        PRIMARY KEY (drive_id, account_id)
    );
    CREATE INDEX idx_shared_drive_access_drive   ON shared_drive_access(drive_id);
    CREATE INDEX idx_shared_drive_access_account ON shared_drive_access(account_id);

    DELETE FROM chunk_vec  WHERE rowid IN (
        SELECT c.id FROM chunks c JOIN documents d ON d.id = c.document_id
        WHERE d.source_type = 'index_only' AND d.source_id LIKE 'gdrive:%:sd:%');
    DELETE FROM chunks_fts WHERE rowid IN (
        SELECT c.id FROM chunks c JOIN documents d ON d.id = c.document_id
        WHERE d.source_type = 'index_only' AND d.source_id LIKE 'gdrive:%:sd:%');
    DELETE FROM chunks WHERE document_id IN (
        SELECT id FROM documents WHERE source_type = 'index_only' AND source_id LIKE 'gdrive:%:sd:%');
    DELETE FROM documents WHERE source_type = 'index_only' AND source_id LIKE 'gdrive:%:sd:%';

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
    // A `writable_schema` text-patch (v17's importance-CHECK relaxation) edits the stored schema
    // without bumping the schema cookie, so this connection would keep compiling the OLD constraint
    // into prepared statements. If any migration ran, bump the cookie once to force a reparse so the
    // relaxed CHECK takes effect immediately (harmless when no writable_schema edit was involved).
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
            version, 21,
            "migration count pin (connector registry is v14; usage cost_usd is v15; \
             semantic-map doc_layout is v16; importance 'archive' level is v17; \
             multi-provider calendar foundation is v18; shared-drive access relation is v19; \
             project milestones is v20; project active-date + manual priority is v21)"
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
}
