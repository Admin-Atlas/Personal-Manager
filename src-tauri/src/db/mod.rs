// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The store: one bundled SQLite connection that is encrypted (SQLCipher /
//! AES-256), vector-capable (sqlite-vec), and keyword-capable (FTS5). Nothing
//! here links against a system SQLite — it is all vendored, so Windows and Mac
//! builds are one command (spec §6, §8.7).

mod migrations;

use std::path::Path;
use std::sync::Once;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Result};

/// Read a value from the `settings` key/value table.
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Error::from)
}

/// Read a `settings` value stored as an RFC3339 timestamp, as UTC. Missing, unreadable, and
/// unparseable all read as `None` — the scheduler callers treat "no valid stamp" as "never ran".
pub fn get_setting_time(conn: &Connection, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    get_setting(conn, key)
        .ok()
        .flatten()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Upsert a value into the `settings` key/value table.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Remove a key from the `settings` table (a no-op if it isn't present).
pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}

/// Read a boolean `settings` value stored as `"true"`/`"false"`, using `default` when the key is
/// absent or holds anything else. The one true reader for the on/off toggles that used to hand-roll
/// `== Some("true")` at each call site.
pub fn get_bool(conn: &Connection, key: &str, default: bool) -> Result<bool> {
    Ok(match get_setting(conn, key)?.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => default,
    })
}

/// Persist a boolean `settings` value as `"true"`/`"false"` — the writer paired with [`get_bool`],
/// replacing the hand-rolled `if enabled { "true" } else { "false" }` sites.
pub fn set_bool(conn: &Connection, key: &str, value: bool) -> Result<()> {
    set_setting(conn, key, if value { "true" } else { "false" })
}

/// Settings key for the indexing-speed preference: "fast" (default, max throughput) or "gentle"
/// (paced so a low-end machine stays usable while it indexes in the background).
pub const INDEXING_SPEED_KEY: &str = "indexing_speed";

/// Pause (ms) inserted after each indexed file in "gentle" mode, so embedding doesn't pin the CPU
/// continuously. A balance: enough idle for the machine to stay responsive, not so much that a large
/// index never finishes. "fast" inserts no pause.
const GENTLE_INDEX_PAUSE_MS: u64 = 250;

/// Embedding batch cap in "gentle" mode: the sidecar embeds at most this many chunks per forward
/// pass, bounding peak activation memory so indexing on a low-memory machine doesn't spike RAM.
/// "fast" passes `None` (the embedder's own default batch — max throughput). Small enough to bound
/// memory, large enough that per-batch overhead stays negligible.
const GENTLE_EMBED_BATCH: usize = 8;

/// How long indexing should pause between files for the current speed setting (0 unless "gentle").
/// Cheap to read, so callers re-read it between files — flipping Fast/Gentle then takes effect on
/// the very next file, even partway through a sync.
pub fn indexing_pause_ms(conn: &Connection) -> u64 {
    match get_setting(conn, INDEXING_SPEED_KEY) {
        Ok(Some(v)) if v == "gentle" => GENTLE_INDEX_PAUSE_MS,
        _ => 0,
    }
}

/// The index-time embedding batch cap for the current speed setting: `Some(GENTLE_EMBED_BATCH)` in
/// "gentle" mode (bounded memory), `None` otherwise (embedder default). Read alongside
/// [`indexing_pause_ms`]; the Drive path re-reads it per item, so gentle batching also engages
/// mid-sync.
pub fn indexing_embed_batch(conn: &Connection) -> Option<usize> {
    match get_setting(conn, INDEXING_SPEED_KEY) {
        Ok(Some(v)) if v == "gentle" => Some(GENTLE_EMBED_BATCH),
        _ => None,
    }
}

/// The `settings` key holding the retrieval-config stamp (spec §21.4). One JSON row capturing
/// the index-time config that produced this vault's index.
const RETRIEVAL_STAMP_KEY: &str = "retrieval_config";

/// The retrieval-config stamp recorded for this vault, if any. `None` for a vault indexed
/// before stamping existed (pre-PR-1) or one whose stamp can't be parsed — both treated as a
/// mismatch by the caller, so the user is offered a one-time Rebuild.
pub fn get_retrieval_stamp(
    conn: &Connection,
) -> Result<Option<crate::retrieval_config::RetrievalConfig>> {
    match get_setting(conn, RETRIEVAL_STAMP_KEY)? {
        Some(json) => Ok(serde_json::from_str(&json).ok()),
        None => Ok(None),
    }
}

/// Record the retrieval-config stamp for this vault — written after a fresh ingest or a Rebuild,
/// so the stored index and the stamp always describe the same pipeline.
pub fn set_retrieval_stamp(
    conn: &Connection,
    cfg: &crate::retrieval_config::RetrievalConfig,
) -> Result<()> {
    let json = serde_json::to_string(cfg)
        .map_err(|e| Error::Other(format!("encode retrieval stamp: {e}")))?;
    set_setting(conn, RETRIEVAL_STAMP_KEY, &json)
}

/// The `settings` key naming the vault's embedder (seeded by migration v2; chosen at onboarding).
const EMBEDDING_MODEL_KEY: &str = "embedding_model";
/// The `settings` key for the query-time reranking toggle (absent ⇒ on).
const RERANKING_ENABLED_KEY: &str = "reranking_enabled";

/// The embedder this vault indexes with — its stored id resolved against the registry, falling
/// back to the English default for an unset/unknown value. Per-vault and **index-time**: changing
/// it (PR 3) re-embeds the vault.
pub fn selected_embedder(conn: &Connection) -> Result<crate::registry::ModelEntry> {
    Ok(match get_setting(conn, EMBEDDING_MODEL_KEY)? {
        Some(id) => crate::registry::embedder_or_default(&id),
        None => crate::registry::active_embedder(),
    })
}

/// Record the vault's embedder selection. Stores **only** the canonical model id — the physical
/// `embedding_dim` is owned by [`ensure_vec_dim`] (the thing that actually resizes the vector
/// table), so a selection that hasn't been re-indexed yet can never desync the recorded width from
/// the real one. Caller validates the id is selectable first.
pub fn set_selected_embedder(conn: &Connection, id: &str) -> Result<()> {
    let e = crate::registry::embedder_or_default(id);
    set_setting(conn, EMBEDDING_MODEL_KEY, e.id)
}

/// Whether query-time reranking is on. Default **true** (absent, or anything but `"false"`, ⇒ on),
/// so existing vaults get reranking after upgrade without needing a settings write.
pub fn reranking_enabled(conn: &Connection) -> Result<bool> {
    Ok(get_setting(conn, RERANKING_ENABLED_KEY)?.as_deref() != Some("false"))
}

/// Turn query-time reranking on or off (stateless — never triggers a Rebuild).
pub fn set_reranking(conn: &Connection, enabled: bool) -> Result<()> {
    set_bool(conn, RERANKING_ENABLED_KEY, enabled)
}

/// The `settings` key for the retrieval depth `k` — how many fused candidates survive to the
/// reranker (card 7H). Query-time and stateless (like reranking): changing it never re-indexes.
const RETRIEVAL_K_KEY: &str = "retrieval_k";

/// Smallest / largest retrieval depth the user can set. Mirrors the bounds the Retrieval-explain
/// panel clamps to; 1 keeps at least one candidate, 50 caps the reranker's workload.
pub const RETRIEVAL_K_MIN: usize = 1;
pub const RETRIEVAL_K_MAX: usize = 50;

/// The retrieval depth `k` this vault uses — the number of fused candidates that reach the
/// reranker (and, after it, the model). Absent/invalid ⇒ [`crate::retrieval::DEFAULT_TOP_K`], so an
/// upgraded vault behaves exactly as before until the user tunes it. Always clamped to
/// `[RETRIEVAL_K_MIN, RETRIEVAL_K_MAX]` so a hand-edited setting can't widen the pool unbounded.
pub fn retrieval_k(conn: &Connection) -> usize {
    get_setting(conn, RETRIEVAL_K_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|k| k.clamp(RETRIEVAL_K_MIN, RETRIEVAL_K_MAX))
        .unwrap_or(crate::retrieval::DEFAULT_TOP_K)
}

/// Set the retrieval depth `k`, clamped to `[RETRIEVAL_K_MIN, RETRIEVAL_K_MAX]`. Stateless — the
/// effect lands on the next query, no Rebuild.
pub fn set_retrieval_k(conn: &Connection, k: usize) -> Result<()> {
    let k = k.clamp(RETRIEVAL_K_MIN, RETRIEVAL_K_MAX);
    set_setting(conn, RETRIEVAL_K_KEY, &k.to_string())
}

/// The confidence-gate DEFAULT threshold — the minimum top-rerank score for PM to treat retrieved
/// sources as authoritative. Calibrated on a full vault (2026-07-18): a clean gap separates genuinely-
/// grounded answers (top rerank score ~ -6 and up) from no-source junk (~ -11), and this sits in it.
/// Applied when the setting is ABSENT, so the gate is ON by default for every vault; a below-threshold
/// top score makes PM hedge instead of fabricating around a weak match. A dev can override the value or
/// disable the gate entirely (see the control in Developer mode); nothing else exposes it.
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = -8.5;

/// The stored sentinel that DISABLES the gate — deliberately distinct from an absent row, which means
/// "use the default". Written by the Developer-mode control when the gate is toggled off.
const CONFIDENCE_GATE_OFF: &str = "off";

/// The `settings` key for the confidence-gate threshold.
const RETRIEVAL_CONFIDENCE_THRESHOLD_KEY: &str = "retrieval_confidence_threshold";

/// The EFFECTIVE confidence-gate threshold, or `None` when the gate is disabled. Resolution:
/// - absent -> `Some(DEFAULT_CONFIDENCE_THRESHOLD)` — the gate is ON by default;
/// - the `"off"` sentinel -> `None` — a dev has explicitly disabled it;
/// - a finite number -> `Some(n)` — a dev override;
/// - anything else (garbage / non-finite) -> `Some(DEFAULT_CONFIDENCE_THRESHOLD)` — never silently drop
///   the safety gate.
///
/// When it returns a value, a retrieval whose TOP rerank score falls below it swaps in the
/// low-confidence grounding instruction so PM hedges instead of grounding on a weak match. Query-time
/// and stateless (like `retrieval_k` / reranking): changing it never re-indexes. Only meaningful when
/// reranking is on, since that is where the score comes from.
pub fn retrieval_confidence_threshold(conn: &Connection) -> Option<f32> {
    match get_setting(conn, RETRIEVAL_CONFIDENCE_THRESHOLD_KEY)
        .ok()
        .flatten()
    {
        None => Some(DEFAULT_CONFIDENCE_THRESHOLD),
        Some(v) if v.as_str() == CONFIDENCE_GATE_OFF => None,
        Some(v) => Some(
            v.parse::<f32>()
                .ok()
                .filter(|t| t.is_finite())
                .unwrap_or(DEFAULT_CONFIDENCE_THRESHOLD),
        ),
    }
}

/// Set the confidence gate. `Some(finite n)` -> the gate is ON at `n`; `None` (or a non-finite value)
/// -> the gate is OFF (writes the [`CONFIDENCE_GATE_OFF`] sentinel, NOT a delete — an absent row means
/// "use the default", a different state). Stateless — the effect lands on the next query, no Rebuild.
pub fn set_retrieval_confidence_threshold(conn: &Connection, threshold: Option<f32>) -> Result<()> {
    match threshold.filter(|t| t.is_finite()) {
        Some(t) => set_setting(conn, RETRIEVAL_CONFIDENCE_THRESHOLD_KEY, &t.to_string()),
        None => set_setting(
            conn,
            RETRIEVAL_CONFIDENCE_THRESHOLD_KEY,
            CONFIDENCE_GATE_OFF,
        ),
    }
}

/// The **live** vector width of this vault's `chunk_vec`, read from the table's own DDL. Migration
/// v2 creates it at `float[384]`; a multilingual vault is drop+recreated to a wider column by
/// [`ensure_vec_dim`] at re-index time, so this reflects the *current physical* width — the source
/// of truth the ingest dimension-guard and the resize check both read. We parse the `sqlite_master`
/// DDL rather than trust the `embedding_dim` setting so the two can never silently disagree.
pub fn vec0_dim(conn: &Connection) -> Result<usize> {
    let sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'chunk_vec'",
        [],
        |row| row.get(0),
    )?;
    parse_vec_width(&sql)
        .ok_or_else(|| Error::Other(format!("could not read chunk_vec vector width from: {sql}")))
}

/// Pull the `N` out of a vec0 column declaration `... float[N] ...` — case- and whitespace-
/// tolerant, no `regex` dependency. Pure so it can be unit-tested against the exact DDL sqlite-vec
/// round-trips into `sqlite_master` (which may differ from the `CREATE` string we wrote).
fn parse_vec_width(ddl: &str) -> Option<usize> {
    let lower = ddl.to_ascii_lowercase();
    let start = lower.find("float[")? + "float[".len();
    let rest = &ddl[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse::<usize>().ok()
}

/// Resize `chunk_vec` to `target` by drop+recreate — the destructive heart of PR 3's re-index that
/// deliberately is **not** an additive migration (a vec0 column's width is fixed at creation; you
/// cannot `ALTER` it, and a migration would wrongly fire for every vault, English ones included).
/// Called only when the table is **empty**: from [`crate::commands::set_vault_embedder`] on a
/// brand-new vault, and from `ingest::rebuild` after it has cleared the store. A no-op when the
/// width already matches, so callers can invoke it unconditionally.
///
/// Safety: refuses to drop a **non-empty** `chunk_vec` (that would silently lose vectors with no
/// rebuild) — the caller clears chunks first. The drop+recreate and the `embedding_dim` mirror
/// update run in one transaction, so a crash can never leave the vault with no `chunk_vec`. This is
/// the **sole writer** of `embedding_dim` (the human-readable mirror of the physical width).
pub fn ensure_vec_dim(conn: &Connection, target: usize) -> Result<()> {
    if vec0_dim(conn)? == target {
        return Ok(());
    }
    let rows: i64 = conn.query_row("SELECT count(*) FROM chunk_vec", [], |r| r.get(0))?;
    if rows > 0 {
        return Err(Error::Other(format!(
            "refusing to resize a populated vector index ({rows} vectors) to {target}-d; clear the \
             index first (this is what the Re-index flow does)"
        )));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(&format!(
        "DROP TABLE chunk_vec; \
         CREATE VIRTUAL TABLE chunk_vec USING vec0(embedding float[{target}]);"
    ))?;
    set_setting(&tx, "embedding_dim", &target.to_string())?;
    tx.commit()?;
    Ok(())
}

/// Distinct project labels across all documents, alphabetical. The one query
/// behind the review picker, the proposal prompts' "existing projects" list, and
/// per-project proposals — kept here so those callers can't drift apart.
pub fn distinct_projects(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT project FROM documents ORDER BY project")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

static REGISTER_VEC: Once = Once::new();

/// Register the sqlite-vec extension as an auto-extension so every connection
/// opened afterwards in this process gets the `vec0` virtual table. This is
/// static linkage — no dynamic `.dll`/`.so` loading.
// A single, audited FFI cast (sqlite-vec's init fn → the auto-extension fn pointer).
// Spelling out the transmute's types would couple this to rusqlite's internal
// libsqlite3-sys type paths, which is more fragile than the cast itself.
#[allow(clippy::missing_transmute_annotations)]
fn register_sqlite_vec() {
    REGISTER_VEC.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// The message [`open`] reports for a genuine, non-transient open failure — a wrong key or a
/// corrupt file, as opposed to a recoverable transient lock / disk-I/O error, which get distinct
/// messages (see the error-code mapping in [`open_keyed`]). This one is deterministic: the store
/// will never open, so it — and ONLY it — is what the boot-error "Start fresh" recovery keys on
/// before it deletes an unreadable store (`wipe::reset_after_open_error`). Sharing the literal
/// keeps the two in lockstep so a message tweak can never silently arm deletion for a transient
/// failure (or disarm it for a real brick).
pub const WRONG_KEY_OR_CORRUPT_MSG: &str = "could not open database (wrong key or corrupt file)";

/// Open the encrypted store, unlock it with the raw 256-bit key, and bring the
/// schema up to date. Returns an error if the key is wrong or the file is
/// corrupt.
pub fn open(path: &Path, key: &str) -> Result<Connection> {
    let conn = open_keyed(path, key)?;
    migrations::run(&conn)?;
    Ok(conn)
}

/// Open + unlock the encrypted store WITHOUT running migrations: the returned
/// connection is keyed, has sqlite-vec registered, WAL + foreign_keys on, and sits at
/// whatever `user_version` the file carries (0 for a fresh file). [`open`] is this plus
/// `migrations::run`. The split lets the migration-ladder test (T-05) build an
/// authentic old-version store from the real migration SQL and drive the full ladder
/// over real data, rather than tearing the current schema back down.
fn open_keyed(path: &Path, key: &str) -> Result<Connection> {
    register_sqlite_vec();
    let conn = Connection::open(path)?;

    // Unlock first — SQLCipher requires `PRAGMA key` before any other access.
    // `key` should be 32 bytes hex (64 chars); validate to avoid malformed input.
    if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Other("invalid database key format".into()));
    }
    // The `x'…'` form means SQLCipher takes these 64 hex chars as the raw 256-bit
    // key directly — no passphrase KDF. Build the PRAGMA in a `Zeroizing` buffer so
    // the heap copy carrying the key is wiped immediately after we use it.
    let key_pragma = zeroize::Zeroizing::new(format!("PRAGMA key = \"x'{key}'\";"));
    conn.execute_batch(&key_pragma)?;

    // Pin the cipher profile explicitly to the SQLCipher 4 defaults. Existing
    // default-created stores still open (the values match), and a future SQLCipher
    // bump can't silently change the at-rest page size / HMAC / KDF and fail to
    // open them. (For a raw key the KDF is bypassed, so kdf_iter is recorded only
    // to keep the profile deliberate and complete.)
    conn.execute_batch(
        "PRAGMA cipher_page_size = 4096; \
         PRAGMA kdf_iter = 256000; \
         PRAGMA cipher_hmac_algorithm = HMAC_SHA512; \
         PRAGMA cipher_kdf_algorithm = PBKDF2_HMAC_SHA512;",
    )?;

    // Touch the schema to confirm the key actually decrypts the database. Map the
    // failure by SQLite error code: a transient file lock (common on Windows when
    // antivirus or the search indexer is holding the file) or a disk I/O error is
    // recoverable on retry, so don't report it as a wrong key / corruption — that
    // would steer the user toward deleting their store. Anything else (incl.
    // NotADatabase from a wrong key) keeps the original message.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|e| {
            use rusqlite::ErrorCode;
            match e.sqlite_error_code() {
                Some(ErrorCode::DatabaseBusy) | Some(ErrorCode::DatabaseLocked) => Error::Other(
                    "the database is in use by another program; close other copies of PM (or your antivirus's lock) and try again".into(),
                ),
                Some(ErrorCode::SystemIoFailure) => {
                    Error::Other(format!("could not read the database file (disk I/O error): {e}"))
                }
                _ => Error::Other(WRONG_KEY_OR_CORRUPT_MSG.into()),
            }
        })?;

    // journal_mode returns a row, so consume it via query rather than execute.
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    Ok(conn)
}

/// Re-key the open store in place (SQLCipher `PRAGMA rekey`). The same connection
/// stays usable with the new key afterward. Used by the vault mode transitions
/// (device <-> passphrase). Validates the key shape exactly like [`open`].
pub fn rekey(conn: &Connection, new_key: &str) -> Result<()> {
    if new_key.len() != 64 || !new_key.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Other("invalid database key format".into()));
    }
    // Same `x'…'` raw-key form + Zeroizing buffer as `open`, so the heap copy carrying
    // the new key is wiped right after SQLCipher re-encrypts the database with it.
    let pragma = zeroize::Zeroizing::new(format!("PRAGMA rekey = \"x'{new_key}'\";"));
    conn.execute_batch(&pragma)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// Pins the on-disk encoding of a boolean setting, not just the round-trip. The point is the
    /// middle assertion: the stored value is the bare string `"true"` / `"false"`. Several call
    /// sites used to hand-roll `set_setting(k, if v { "true" } else { "false" })`, and this is what
    /// makes replacing them with [`set_bool`] a checked refactor rather than an asserted one — if
    /// the helper ever changes its literals, every one of those settings flips meaning on upgrade.
    #[test]
    fn a_boolean_setting_is_stored_as_the_bare_word_true_or_false() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("bools.sqlite"), KEY).unwrap();

        set_bool(&conn, "probe", true).unwrap();
        assert_eq!(
            get_setting(&conn, "probe").unwrap().as_deref(),
            Some("true")
        );
        assert!(get_bool(&conn, "probe", false).unwrap());

        set_bool(&conn, "probe", false).unwrap();
        assert_eq!(
            get_setting(&conn, "probe").unwrap().as_deref(),
            Some("false")
        );
        assert!(!get_bool(&conn, "probe", true).unwrap());

        // Absent and unparseable both fall back to the caller's default — the documented contract
        // that `backup::schedule::setting_bool` does NOT share (it reads a non-canonical value as
        // false). Left as two readers deliberately; see the note on that function.
        assert!(get_bool(&conn, "never-written", true).unwrap());
        set_setting(&conn, "probe", "yes").unwrap();
        assert!(get_bool(&conn, "probe", true).unwrap());
    }

    #[test]
    fn encrypted_store_supports_vectors_and_keyword_search() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");

        {
            let conn = open(&path, KEY).unwrap();

            // Vector search (sqlite-vec): create, insert, and run a KNN query.
            conn.execute_batch("CREATE VIRTUAL TABLE vec_demo USING vec0(embedding float[4]);")
                .unwrap();
            conn.execute(
                "INSERT INTO vec_demo(rowid, embedding) VALUES (1, ?1)",
                params!["[1.0, 2.0, 3.0, 4.0]"],
            )
            .unwrap();
            let nearest: i64 = conn
                .query_row(
                    "SELECT count(*) FROM (SELECT rowid FROM vec_demo \
                     WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1)",
                    params!["[1.0, 2.0, 3.0, 4.0]"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(nearest, 1, "vec0 KNN query should return a match");

            // Keyword search (FTS5).
            conn.execute_batch("CREATE VIRTUAL TABLE fts_demo USING fts5(body);")
                .unwrap();
            conn.execute(
                "INSERT INTO fts_demo(body) VALUES ('the quick brown fox is searchable')",
                [],
            )
            .unwrap();
            let hits: i64 = conn
                .query_row(
                    "SELECT count(*) FROM fts_demo WHERE fts_demo MATCH 'searchable'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(hits, 1, "fts5 MATCH should find the row");

            // App schema from migrations is present and writable.
            conn.execute("INSERT INTO conversations(title) VALUES ('hello')", [])
                .unwrap();

            // The Archivist schema (migration v2): a document, a chunk, its
            // 384-d embedding in chunk_vec, and its text in the FTS index — the
            // exact shape ingestion writes. Prove each can be read back.
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash, ext) \
                 VALUES ('note-abc123.md', 'A note', 'abc123', 'md')",
                [],
            )
            .unwrap();
            let doc_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count) \
                 VALUES (?1, 0, 'the quick brown fox is searchable', 33)",
                params![doc_id],
            )
            .unwrap();
            let chunk_id = conn.last_insert_rowid();

            // A 384-d embedding, stored against the chunk's rowid.
            let embedding = format!("[{}]", vec!["0.01"; 384].join(", "));
            conn.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, embedding],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks_fts(rowid, content) VALUES (?1, ?2)",
                params![chunk_id, "the quick brown fox is searchable"],
            )
            .unwrap();

            // KNN over the real chunk vectors returns our chunk.
            let nearest_chunk: i64 = conn
                .query_row(
                    "SELECT rowid FROM chunk_vec WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                    params![embedding],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(nearest_chunk, chunk_id);

            // Keyword search over the chunk FTS finds it too.
            let fts_hits: i64 = conn
                .query_row(
                    "SELECT count(*) FROM chunks_fts WHERE chunks_fts MATCH 'searchable'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(fts_hits, 1);

            // Organisation + learning columns (migration v4): the document took
            // the defaults, and they round-trip; a correction logs against it.
            let (project, tags, importance, reviewed): (String, String, Option<String>, i64) = conn
                .query_row(
                    "SELECT project, tags, importance, reviewed FROM documents WHERE id = ?1",
                    params![doc_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert_eq!(
                (project.as_str(), tags.as_str(), importance, reviewed),
                ("Unsorted", "[]", None, 0)
            );

            conn.execute(
                "UPDATE documents SET project = 'Finances', importance = 'high', reviewed = 1 WHERE id = ?1",
                params![doc_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO corrections(document_id, field, before_val, after_val, title) \
                 VALUES (?1, 'project', '\"Unsorted\"', '\"Finances\"', 'A note')",
                params![doc_id],
            )
            .unwrap();
            let corr: i64 = conn
                .query_row(
                    "SELECT count(*) FROM corrections WHERE document_id = ?1",
                    params![doc_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(corr, 1);
        }

        // Reopening with the correct key succeeds and the data persisted.
        {
            let conn = open(&path, KEY).unwrap();
            let count: i64 = conn
                .query_row("SELECT count(*) FROM conversations", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
        }

        // Reopening with the wrong key fails — the file really is encrypted.
        {
            let wrong = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
            assert!(open(&path, wrong).is_err(), "wrong key must not decrypt");
        }
    }

    #[test]
    fn rekey_swaps_the_key_and_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rekey.sqlite");
        let new_key = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
        {
            let conn = open(&path, KEY).unwrap();
            conn.execute("INSERT INTO conversations(title) VALUES ('keep me')", [])
                .unwrap();
        }
        {
            let conn = open(&path, KEY).unwrap();
            rekey(&conn, new_key).unwrap();
        }
        // The old key no longer opens it; the new key does, with the row intact.
        assert!(
            open(&path, KEY).is_err(),
            "old key must stop working after rekey"
        );
        let conn = open(&path, new_key).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conversations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn retrieval_k_defaults_roundtrips_and_clamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.sqlite");
        let conn = open(&path, KEY).unwrap();

        // Unset ⇒ the retriever's default depth, so an upgraded vault behaves exactly as before.
        assert_eq!(retrieval_k(&conn), crate::retrieval::DEFAULT_TOP_K);

        // A normal value round-trips.
        set_retrieval_k(&conn, 12).unwrap();
        assert_eq!(retrieval_k(&conn), 12);

        // Out-of-range writes are clamped to the bounds, not rejected — the panel can't persist a
        // pool that's empty or unbounded even if the frontend sent a wild value.
        set_retrieval_k(&conn, 0).unwrap();
        assert_eq!(retrieval_k(&conn), RETRIEVAL_K_MIN);
        set_retrieval_k(&conn, 9999).unwrap();
        assert_eq!(retrieval_k(&conn), RETRIEVAL_K_MAX);

        // A garbage stored value (hand-edited) also falls back to the default rather than panicking.
        set_setting(&conn, RETRIEVAL_K_KEY, "not-a-number").unwrap();
        assert_eq!(retrieval_k(&conn), crate::retrieval::DEFAULT_TOP_K);
    }

    #[test]
    fn confidence_gate_defaults_on_with_off_sentinel_and_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.sqlite");
        let conn = open(&path, KEY).unwrap();

        // Unset ⇒ the gate is ON at the calibrated default, so every vault is protected out of the box.
        assert_eq!(
            retrieval_confidence_threshold(&conn),
            Some(DEFAULT_CONFIDENCE_THRESHOLD)
        );

        // A dev override round-trips.
        set_retrieval_confidence_threshold(&conn, Some(-6.0)).unwrap();
        assert_eq!(retrieval_confidence_threshold(&conn), Some(-6.0));

        // Disabling writes the "off" sentinel (NOT a delete), which resolves to None — a state distinct
        // from "absent", so it survives and does NOT fall back to the default.
        set_retrieval_confidence_threshold(&conn, None).unwrap();
        assert_eq!(retrieval_confidence_threshold(&conn), None);
        assert_eq!(
            get_setting(&conn, RETRIEVAL_CONFIDENCE_THRESHOLD_KEY)
                .unwrap()
                .as_deref(),
            Some(CONFIDENCE_GATE_OFF)
        );

        // A non-finite request is treated as "off".
        set_retrieval_confidence_threshold(&conn, Some(f32::NAN)).unwrap();
        assert_eq!(retrieval_confidence_threshold(&conn), None);

        // A garbage stored value (hand-edited) falls back to the default rather than silently disabling
        // the safety gate.
        set_setting(&conn, RETRIEVAL_CONFIDENCE_THRESHOLD_KEY, "not-a-number").unwrap();
        assert_eq!(
            retrieval_confidence_threshold(&conn),
            Some(DEFAULT_CONFIDENCE_THRESHOLD)
        );
    }

    #[test]
    fn chunk_schema_v9_parents_are_structural_only() {
        // The retrieval-foundation schema (migration v9): a structural parent spanning two
        // embedded leaves. The parent lives in `chunks` but is NOT in chunk_vec/chunks_fts, so
        // the "rowid mirrors chunks.id" invariant holds and a KNN never returns it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v9.sqlite");
        let conn = open(&path, KEY).unwrap();

        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('doc.md','Doc','h9')",
            [],
        )
        .unwrap();
        let doc = conn.last_insert_rowid();

        // Parent: stored, with a uid + offsets + kind, but never embedded / FTS-indexed.
        conn.execute(
            "INSERT INTO chunks(document_id, ordinal, content, char_count, uid, kind, start_offset, end_offset) \
             VALUES (?1, 0, 'parent section text', 19, 'uid-parent', 'parent', 0, 40)",
            params![doc],
        )
        .unwrap();
        let parent_id = conn.last_insert_rowid();

        // Two leaves linked to the parent, each embedded + FTS-indexed on its own rowid.
        for (ord, uid, text, seed) in [
            (1i64, "uid-leaf-a", "alpha leaf about cats", "0.10"),
            (2i64, "uid-leaf-b", "beta leaf about cats", "0.20"),
        ] {
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count, uid, parent_id, kind, start_offset, end_offset) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'leaf', 0, 20)",
                params![doc, ord, text, text.len() as i64, uid, parent_id],
            )
            .unwrap();
            let leaf_id = conn.last_insert_rowid();
            let emb = format!("[{}]", vec![seed; 384].join(", "));
            conn.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![leaf_id, emb],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks_fts(rowid, content) VALUES (?1, ?2)",
                params![leaf_id, text],
            )
            .unwrap();
        }

        // New columns round-trip, and the parent/child linkage resolves.
        let (kind, uid): (String, String) = conn
            .query_row(
                "SELECT kind, uid FROM chunks WHERE id = ?1",
                params![parent_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), uid.as_str()), ("parent", "uid-parent"));
        let children: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE parent_id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(children, 2);

        // The invariant: a KNN over chunk_vec returns only leaves — never the parent (which has
        // no vector row), so a structural parent being a gap in chunk_vec is safe.
        let query = format!("[{}]", vec!["0.10"; 384].join(", "));
        let mut stmt = conn
            .prepare(
                "SELECT rowid FROM chunk_vec WHERE embedding MATCH ?1 ORDER BY distance LIMIT 10",
            )
            .unwrap();
        let hits: Vec<i64> = stmt
            .query_map(params![query], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(hits.len(), 2, "only the two leaves are in the vector index");
        assert!(
            !hits.contains(&parent_id),
            "KNN must never return a structural parent"
        );
    }

    #[test]
    fn embedder_selection_and_reranking_toggle_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.sqlite");
        let conn = open(&path, KEY).unwrap();

        // Fresh vault: migration v2 seeds the English embedder; reranking defaults on.
        assert_eq!(
            selected_embedder(&conn).unwrap().id,
            "BAAI/bge-small-en-v1.5"
        );
        assert!(reranking_enabled(&conn).unwrap());

        // Select the multilingual embedder; only the model id is recorded. The physical vec width
        // (embedding_dim) is owned by ensure_vec_dim, so it stays the seeded 384 until a resize —
        // a selection without a re-index can never desync the recorded width from the real one.
        set_selected_embedder(&conn, "intfloat/multilingual-e5-large").unwrap();
        assert_eq!(
            selected_embedder(&conn).unwrap().id,
            "intfloat/multilingual-e5-large"
        );
        assert_eq!(selected_embedder(&conn).unwrap().dimension, 1024);
        assert_eq!(
            get_setting(&conn, "embedding_dim").unwrap().as_deref(),
            Some("384"),
            "set_selected_embedder must not touch the physical width"
        );

        // An unknown stored id resolves to the English default rather than breaking ingest.
        set_setting(&conn, EMBEDDING_MODEL_KEY, "nope/not-a-model").unwrap();
        assert_eq!(
            selected_embedder(&conn).unwrap().id,
            "BAAI/bge-small-en-v1.5"
        );

        // Reranking toggles off and back on (stateless).
        set_reranking(&conn, false).unwrap();
        assert!(!reranking_enabled(&conn).unwrap());
        set_reranking(&conn, true).unwrap();
        assert!(reranking_enabled(&conn).unwrap());
    }

    #[test]
    fn parse_vec_width_tolerates_spacing_and_case() {
        assert_eq!(
            parse_vec_width("CREATE VIRTUAL TABLE chunk_vec USING vec0(embedding float[384])"),
            Some(384)
        );
        assert_eq!(parse_vec_width("... FLOAT[ 1024 ] ..."), Some(1024));
        assert_eq!(parse_vec_width("no vector column here"), None);
    }

    #[test]
    fn vec0_dim_reads_the_live_table_width() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vecdim.sqlite");
        let conn = open(&path, KEY).unwrap();

        // Fresh vault: migration v2 built chunk_vec at float[384].
        assert_eq!(vec0_dim(&conn).unwrap(), 384);

        // Recreate at a different width by hand — vec0_dim follows the actual table, not a setting.
        conn.execute_batch(
            "DROP TABLE chunk_vec; CREATE VIRTUAL TABLE chunk_vec USING vec0(embedding float[512]);",
        )
        .unwrap();
        assert_eq!(vec0_dim(&conn).unwrap(), 512);
    }

    #[test]
    fn ensure_vec_dim_resizes_an_empty_table_and_is_idempotent() {
        // Use 512 — a real, model-independent width — to prove the SQL machinery without any model.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resize.sqlite");
        let conn = open(&path, KEY).unwrap();
        assert_eq!(vec0_dim(&conn).unwrap(), 384);

        // Same width: a no-op that leaves the table untouched.
        ensure_vec_dim(&conn, 384).unwrap();
        assert_eq!(vec0_dim(&conn).unwrap(), 384);

        // Resize the (empty) table to 512: the width flips and the embedding_dim mirror follows.
        ensure_vec_dim(&conn, 512).unwrap();
        assert_eq!(vec0_dim(&conn).unwrap(), 512);
        assert_eq!(
            get_setting(&conn, "embedding_dim").unwrap().as_deref(),
            Some("512")
        );

        // The recreated table really works: a 512-d KNN round-trips.
        let v = format!("[{}]", vec!["0.1"; 512].join(", "));
        conn.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (1, ?1)",
            params![v],
        )
        .unwrap();
        let hit: i64 = conn
            .query_row(
                "SELECT rowid FROM chunk_vec WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                params![v],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);

        // Idempotent at the new width.
        ensure_vec_dim(&conn, 512).unwrap();
        assert_eq!(vec0_dim(&conn).unwrap(), 512);
    }

    #[test]
    fn ensure_vec_dim_refuses_a_populated_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refuse.sqlite");
        let conn = open(&path, KEY).unwrap();

        // One 384-d vector in the table, then try to resize — the safety guard must refuse.
        let v = format!("[{}]", vec!["0.1"; 384].join(", "));
        conn.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (1, ?1)",
            params![v],
        )
        .unwrap();

        let err = ensure_vec_dim(&conn, 1024).unwrap_err();
        assert!(err.to_string().contains("refusing to resize"), "got: {err}");
        // Untouched — still 384-d and still holding its vector.
        assert_eq!(vec0_dim(&conn).unwrap(), 384);
        let n: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn ensure_vec_dim_is_the_sole_writer_of_embedding_dim() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.sqlite");
        let conn = open(&path, KEY).unwrap();

        // Selecting an embedder does NOT move embedding_dim (migration seeded "384").
        set_selected_embedder(&conn, "intfloat/multilingual-e5-large").unwrap();
        assert_eq!(
            get_setting(&conn, "embedding_dim").unwrap().as_deref(),
            Some("384")
        );

        // Only ensure_vec_dim advances it (the empty table resizes cleanly).
        ensure_vec_dim(&conn, 1024).unwrap();
        assert_eq!(
            get_setting(&conn, "embedding_dim").unwrap().as_deref(),
            Some("1024")
        );
        assert_eq!(vec0_dim(&conn).unwrap(), 1024);
    }

    #[test]
    fn migration_v10_backfills_one_entity_per_project_no_auto_merge() {
        // The canonical-entity backfill, exercised over a realistic v9→v10 upgrade: tear the v10
        // tables back down to a v9-shaped store, seed it with documents carrying name variants,
        // then re-run migrations so the real backfill processes them. Because `migrations::run`
        // re-applies EVERY step from the reset `user_version` forward, every later step must be
        // reverted too — otherwise v11's/v12's ADD COLUMNs and v13's/v19's CREATE TABLEs would collide
        // with schema still on disk. Drop order respects FKs: `preferences` (v13) drops before the
        // `entities` it references, and `shared_drive_access` (v19) before its `connector_sources`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v10.sqlite");
        let conn = open(&path, KEY).unwrap();

        conn.execute_batch(
            // v43/v44: `corrections` and `messages` both predate the rewind point, so unlike
            // `project_milestones` (dropped wholesale below) their added columns have to come off
            // explicitly — and their indexes first, since SQLite won't drop an indexed column.
            // v46: the join drops before the tags it points at — `document_tags.tag_id REFERENCES
            // tags(id)` under `PRAGMA foreign_keys = ON`, so the parent cannot go first (the same
            // rule the `preferences`/`entities` pair below follows).
            "DROP TABLE document_tags; \
             DROP TABLE tags; \
             DROP TABLE retrieval_feedback; \
             ALTER TABLE messages DROP COLUMN retrieved_chunk_ids; \
             ALTER TABLE messages DROP COLUMN retrieved_chunk_uids; \
             ALTER TABLE messages DROP COLUMN retrieved_config_stamp; \
             DROP INDEX idx_corrections_pipeline; \
             ALTER TABLE corrections DROP COLUMN pipeline_version; \
             ALTER TABLE usage_log DROP COLUMN provider; \
             ALTER TABLE usage_log DROP COLUMN latency_ms; \
             ALTER TABLE usage_log DROP COLUMN fallback_reason; \
             ALTER TABLE documents DROP COLUMN rebuild_pass; \
             DROP TABLE flags; \
             ALTER TABLE chunks DROP COLUMN chat_turn_id; \
             ALTER TABLE chunks DROP COLUMN chunk_at; \
             DROP TABLE chat_sessions; \
             DROP TABLE photos; \
             DROP TABLE project_activity; \
             DROP TABLE project_activity_daily; \
             DROP TABLE spreadsheets; \
             DROP TABLE project_milestones; \
             ALTER TABLE projects DROP COLUMN last_touched; \
             ALTER TABLE projects DROP COLUMN importance; \
             DROP INDEX idx_calendar_events_uid; \
             DROP INDEX idx_calendar_events_entity; \
             ALTER TABLE calendar_events DROP COLUMN kind_override; \
             DROP INDEX idx_calendar_events_calendar; \
             DROP TABLE calendars; \
             ALTER TABLE calendar_events DROP COLUMN uid; \
             ALTER TABLE calendar_events DROP COLUMN entity_id; \
             ALTER TABLE calendar_events DROP COLUMN show_as; \
             ALTER TABLE calendar_events DROP COLUMN organizer; \
             ALTER TABLE calendar_events DROP COLUMN attendees; \
             ALTER TABLE calendar_events DROP COLUMN conference_url; \
             ALTER TABLE calendar_events DROP COLUMN recurring; \
             ALTER TABLE calendar_events DROP COLUMN recurrence_summary; \
             ALTER TABLE calendar_events DROP COLUMN status; \
             ALTER TABLE calendar_events DROP COLUMN visibility; \
             ALTER TABLE calendar_events DROP COLUMN created; \
             ALTER TABLE calendar_events DROP COLUMN updated; \
             DROP TABLE doc_layout; \
             DROP TABLE document_proposals; \
             DROP TABLE shared_drive_access; \
             DROP TABLE shared_with_me_access; \
             DROP TABLE connector_sources; \
             DROP TABLE preferences; \
             DROP INDEX idx_documents_source_id; \
             DROP INDEX idx_documents_source_type; \
             ALTER TABLE documents DROP COLUMN source_type; \
             ALTER TABLE documents DROP COLUMN source_state; \
             ALTER TABLE documents DROP COLUMN source_id; \
             ALTER TABLE documents DROP COLUMN external_ref; \
             ALTER TABLE documents DROP COLUMN source_modified_at; \
             ALTER TABLE documents DROP COLUMN source_content_hash; \
             ALTER TABLE documents DROP COLUMN stored_summary; \
             ALTER TABLE documents DROP COLUMN source_parent_folder_id; \
             ALTER TABLE documents DROP COLUMN source_parent_folder_name; \
             ALTER TABLE documents DROP COLUMN source_account; \
             DROP INDEX IF EXISTS idx_documents_entity; \
             DROP TABLE entity_aliases; \
             DROP TABLE entities; \
             ALTER TABLE documents DROP COLUMN entity_id; \
             ALTER TABLE projects  DROP COLUMN entity_id; \
             ALTER TABLE usage_log DROP COLUMN cost_usd; \
             PRAGMA user_version = 9;",
        )
        .unwrap();

        // Two "PM" docs, one "Atlas - PM" variant of the same project, one "Research"; plus a
        // triage row and a stray chunk vector (to prove the metadata migration leaves vectors be).
        for (vp, hash, project) in [
            ("a.md", "ha", "PM"),
            ("b.md", "hb", "Atlas - PM"),
            ("c.md", "hc", "Research"),
            ("d.md", "hd", "PM"),
        ] {
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash, project) VALUES (?1,'T',?2,?3)",
                params![vp, hash, project],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO projects(name) VALUES ('Research')", [])
            .unwrap();
        let vec = format!("[{}]", vec!["0.1"; 384].join(", "));
        conn.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (1, ?1)",
            params![vec],
        )
        .unwrap();

        migrations::run(&conn).unwrap();

        // No document is left unresolved.
        let null_ids: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents WHERE entity_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_ids, 0, "every document must resolve to an entity");

        // One entity per distinct project string + the always-seeded 'Unsorted' (NO auto-merge of
        // the variant into PM — that is the user's call via the Teach tab).
        let names: Vec<String> = {
            let mut s = conn
                .prepare("SELECT canonical_name FROM entities WHERE type='project' ORDER BY canonical_name")
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(names, vec!["Atlas - PM", "PM", "Research", "Unsorted"]);

        // Each canonical is its own self-alias; the two PM docs share one entity, the variant has
        // its own, and the triage row was attached to the same entity by name.
        let pm: i64 = conn
            .query_row(
                "SELECT entity_id FROM entity_aliases WHERE alias='PM'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let variant: i64 = conn
            .query_row(
                "SELECT entity_id FROM entity_aliases WHERE alias='Atlas - PM'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(pm, variant);
        let pm_docs: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents WHERE entity_id=?1",
                params![pm],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pm_docs, 2);
        let research_entity: Option<i64> = conn
            .query_row(
                "SELECT entity_id FROM projects WHERE name='Research'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            research_entity.is_some(),
            "triage row attached to its entity"
        );

        // The vector index is untouched — a metadata migration must never disturb embeddings.
        let vecs: i64 = conn
            .query_row("SELECT count(*) FROM chunk_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vecs, 1, "chunk vectors must survive the entity migration");
    }

    #[test]
    fn migration_v11_defaults_existing_rows_to_stored_vault_documents() {
        // Tear the store back down to a v10-shaped store, seed a document the way v10 wrote them (no
        // source_* columns), then re-run migrations so the real v11 step processes a pre-existing
        // row. Every migration ABOVE v10 must be reverted — v11's source_* columns, v12's
        // `entities.confidence`/`user_confirmed`, v13's `preferences` table, AND v19's
        // `shared_drive_access` — otherwise their re-run ADD COLUMNs / CREATE TABLEs collide with
        // schema still on disk from the initial `open`. FK order: `preferences` drops before
        // `entities`, and `shared_drive_access` before `connector_sources`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v11.sqlite");
        let conn = open(&path, KEY).unwrap();

        conn.execute_batch(
            // v43/v44: `corrections` and `messages` both predate the rewind point, so unlike
            // `project_milestones` (dropped wholesale below) their added columns have to come off
            // explicitly — and their indexes first, since SQLite won't drop an indexed column.
            // v46: the join drops before the tags it points at — `document_tags.tag_id REFERENCES
            // tags(id)` under `PRAGMA foreign_keys = ON`, so the parent cannot go first (the same
            // rule the `preferences`/`entities` pair below follows).
            "DROP TABLE document_tags; \
             DROP TABLE tags; \
             DROP TABLE retrieval_feedback; \
             ALTER TABLE messages DROP COLUMN retrieved_chunk_ids; \
             ALTER TABLE messages DROP COLUMN retrieved_chunk_uids; \
             ALTER TABLE messages DROP COLUMN retrieved_config_stamp; \
             DROP INDEX idx_corrections_pipeline; \
             ALTER TABLE corrections DROP COLUMN pipeline_version; \
             ALTER TABLE usage_log DROP COLUMN provider; \
             ALTER TABLE usage_log DROP COLUMN latency_ms; \
             ALTER TABLE usage_log DROP COLUMN fallback_reason; \
             ALTER TABLE documents DROP COLUMN rebuild_pass; \
             DROP TABLE flags; \
             ALTER TABLE chunks DROP COLUMN chat_turn_id; \
             ALTER TABLE chunks DROP COLUMN chunk_at; \
             DROP TABLE chat_sessions; \
             DROP TABLE photos; \
             DROP TABLE project_activity; \
             DROP TABLE project_activity_daily; \
             DROP TABLE spreadsheets; \
             DROP TABLE project_milestones; \
             ALTER TABLE projects DROP COLUMN last_touched; \
             ALTER TABLE projects DROP COLUMN importance; \
             DROP INDEX idx_calendar_events_uid; \
             DROP INDEX idx_calendar_events_entity; \
             ALTER TABLE calendar_events DROP COLUMN kind_override; \
             DROP INDEX idx_calendar_events_calendar; \
             DROP TABLE calendars; \
             ALTER TABLE calendar_events DROP COLUMN uid; \
             ALTER TABLE calendar_events DROP COLUMN entity_id; \
             ALTER TABLE calendar_events DROP COLUMN show_as; \
             ALTER TABLE calendar_events DROP COLUMN organizer; \
             ALTER TABLE calendar_events DROP COLUMN attendees; \
             ALTER TABLE calendar_events DROP COLUMN conference_url; \
             ALTER TABLE calendar_events DROP COLUMN recurring; \
             ALTER TABLE calendar_events DROP COLUMN recurrence_summary; \
             ALTER TABLE calendar_events DROP COLUMN status; \
             ALTER TABLE calendar_events DROP COLUMN visibility; \
             ALTER TABLE calendar_events DROP COLUMN created; \
             ALTER TABLE calendar_events DROP COLUMN updated; \
             DROP TABLE doc_layout; \
             DROP TABLE document_proposals; \
             DROP TABLE shared_drive_access; \
             DROP TABLE shared_with_me_access; \
             DROP TABLE connector_sources; \
             DROP TABLE preferences; \
             DROP INDEX idx_documents_source_id; \
             DROP INDEX idx_documents_source_type; \
             ALTER TABLE documents DROP COLUMN source_type; \
             ALTER TABLE documents DROP COLUMN source_state; \
             ALTER TABLE documents DROP COLUMN source_id; \
             ALTER TABLE documents DROP COLUMN external_ref; \
             ALTER TABLE documents DROP COLUMN source_modified_at; \
             ALTER TABLE documents DROP COLUMN source_content_hash; \
             ALTER TABLE documents DROP COLUMN stored_summary; \
             ALTER TABLE documents DROP COLUMN source_parent_folder_id; \
             ALTER TABLE documents DROP COLUMN source_parent_folder_name; \
             ALTER TABLE documents DROP COLUMN source_account; \
             ALTER TABLE entities DROP COLUMN confidence; \
             ALTER TABLE entities DROP COLUMN user_confirmed; \
             ALTER TABLE usage_log DROP COLUMN cost_usd; \
             PRAGMA user_version = 10;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('a.md','A','ha')",
            [],
        )
        .unwrap();

        migrations::run(&conn).unwrap();

        // The pre-existing row reads as a fully-stored vault document with an empty pointer — no
        // backfill needed (rule #3).
        let (st, state, sid): (String, String, Option<String>) = conn
            .query_row(
                "SELECT source_type, source_state, source_id FROM documents WHERE vault_path='a.md'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(st, "vault");
        assert_eq!(state, "ok");
        assert_eq!(sid, None);

        // An index-only row keyed by a stable source id coexists, using the synthetic vault_path
        // sentinel to satisfy NOT NULL UNIQUE without a real Markdown file.
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id) \
             VALUES ('idx://src-1','Pointer','hp','index_only','src-1')",
            [],
        )
        .unwrap();

        // The partial unique index permits many NULL-source_id vault rows but rejects a duplicate
        // stable id.
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('b.md','B','hb')",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id) \
             VALUES ('idx://src-1-dup','Dup','hd','index_only','src-1')",
            [],
        );
        assert!(
            dup.is_err(),
            "a duplicate stable source_id must be rejected"
        );

        // The CHECK constraints reject an out-of-range discriminator / state.
        assert!(
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash, source_type) \
                 VALUES ('z.md','Z','hz','bogus')",
                [],
            )
            .is_err(),
            "source_type CHECK must reject an unknown value"
        );
    }
}
