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

/// Upsert a value into the `settings` key/value table.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
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

/// Open the encrypted store, unlock it with the raw 256-bit key, and bring the
/// schema up to date. Returns an error if the key is wrong or the file is
/// corrupt.
pub fn open(path: &Path, key: &str) -> Result<Connection> {
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
                _ => Error::Other("could not open database (wrong key or corrupt file)".into()),
            }
        })?;

    // journal_mode returns a row, so consume it via query rather than execute.
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    migrations::run(&conn)?;
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
}
