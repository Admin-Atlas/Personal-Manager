// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ingestion: turn a pile of files into a searchable store. For each file the
//! flow is convert → hash → (dedupe) → chunk → embed → write the Markdown vault
//! → index in SQLite (spec §8.2). The Markdown vault is the source of truth, so
//! the whole index is rebuildable from it (`rebuild`).
//!
//! All of this is synchronous and runs off the async runtime via
//! `spawn_blocking` (see `commands::ingest_paths`). The DB lock is held only for
//! the final insert transaction — never across a convert/embed sidecar call.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::vault::MarkdownCipher;
use crate::AppState;

/// Roughly one embedding's worth of text, with a little overlap so meaning that
/// straddles a boundary is still retrievable.
const CHUNK_TARGET: usize = 1500;
const CHUNK_OVERLAP: usize = 150;

/// Dimension of the pinned embedding model (bge-small-en-v1.5). Must match the
/// `chunk_vec` column and the `embedding_dim` setting.
const EMBED_DIM: usize = 384;

/// Extensions MarkItDown handles well. Anything else is skipped (still findable
/// on disk, just not ingested). Lower-case, no dot.
const SUPPORTED: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "doc", "ppt", "xls", "html", "htm", "csv", "json", "xml", "txt",
    "md", "markdown", "rtf", "epub", "png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp",
];

/// A document as shown in the Documents view.
#[derive(Clone, Serialize)]
pub struct Document {
    pub id: i64,
    pub title: String,
    pub source_path: Option<String>,
    pub ext: Option<String>,
    pub byte_size: Option<i64>,
    pub chunk_count: i64,
    pub created_at: Option<String>,
    pub ingested_at: String,
    /// Organisation metadata (Step 4). `reviewed` is false until the user has
    /// confirmed the sorting; `last_activity` drives retrieval recency decay.
    pub project: String,
    pub tags: Vec<String>,
    pub importance: Option<String>,
    pub reviewed: bool,
    pub last_activity: Option<String>,
}

/// Streamed to the UI over a Tauri channel as ingestion proceeds (mirrors the
/// chat `ChatEvent` pattern).
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// These stream as occasional UI progress events; the `Done` variant's `Document`
// makes it larger, but boxing it (serde-transparent) isn't worth the churn here.
#[allow(clippy::large_enum_variant)]
pub enum IngestEvent {
    /// Long first-run setup (installing the engine / downloading the model).
    Preparing {
        message: String,
    },
    Started {
        path: String,
        name: String,
    },
    Skipped {
        path: String,
        reason: String,
    },
    Done {
        document: Document,
    },
    Failed {
        path: String,
        error: String,
    },
    Finished {
        ingested: usize,
        skipped: usize,
        failed: usize,
    },
}

/// Convert + index every file under `inputs` (folders are walked). Blocking.
pub fn run(app: &AppHandle, inputs: Vec<String>, on_event: Channel<IngestEvent>) -> Result<()> {
    let state = app.state::<AppState>();

    let _ = on_event.send(IngestEvent::Preparing {
        message: "Preparing the document engine…".into(),
    });
    state.sidecar.ensure_installed()?;

    // The vault's Markdown dir + cipher for this whole run (they don't change mid-run).
    // Snapshotting up front means we never hold the vault lock across a sidecar call.
    let (vault, cipher) = state.markdown_io()?;
    let files = collect_files(&inputs);

    let (mut ingested, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    for path in files {
        let name = file_name(&path);
        let _ = on_event.send(IngestEvent::Started {
            path: path.to_string_lossy().into(),
            name,
        });

        match ingest_one(&state, &vault, &cipher, &path) {
            Ok(Outcome::Indexed(document)) => {
                ingested += 1;
                let _ = on_event.send(IngestEvent::Done { document });
            }
            Ok(Outcome::Skipped(reason)) => {
                skipped += 1;
                let _ = on_event.send(IngestEvent::Skipped {
                    path: path.to_string_lossy().into(),
                    reason,
                });
            }
            Err(e) => {
                failed += 1;
                let _ = on_event.send(IngestEvent::Failed {
                    path: path.to_string_lossy().into(),
                    error: e.to_string(),
                });
            }
        }
    }

    let _ = on_event.send(IngestEvent::Finished {
        ingested,
        skipped,
        failed,
    });
    Ok(())
}

// A short-lived per-document result; not copied in bulk, so the size gap between
// `Indexed(Document)` and `Skipped(String)` is not worth boxing.
#[allow(clippy::large_enum_variant)]
enum Outcome {
    Indexed(Document),
    Skipped(String),
}

fn ingest_one(
    state: &AppState,
    vault: &Path,
    cipher: &MarkdownCipher,
    path: &Path,
) -> Result<Outcome> {
    let ext = extension(path);
    match &ext {
        Some(e) if SUPPORTED.contains(&e.as_str()) => {}
        _ => return Ok(Outcome::Skipped("unsupported file type".into())),
    }

    // Convert to Markdown (sidecar; no DB lock held).
    let (markdown, title) = state.sidecar.convert(path)?;
    let markdown = markdown.trim().to_string();
    // A file that renders to no text (an image with no OCR, a blank document) has
    // nothing to index — and would otherwise hash to sha256("") and collide with
    // every other empty file, so the second one is wrongly skipped as a duplicate.
    // Skip it outright rather than store an unsearchable phantom row.
    if markdown.is_empty() {
        return Ok(Outcome::Skipped("no extractable text".into()));
    }
    let title = pick_title(&title, path);
    let content_hash = hex_digest(markdown.as_bytes());
    let byte_size = std::fs::metadata(path).map(|m| m.len() as i64).ok();

    // Dedupe + format timestamps in one short lock; release before embedding.
    let (created_at, ingested_at) = {
        let conn = state.conn()?;
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM documents WHERE content_hash = ?1",
                params![content_hash],
                |_| Ok(()),
            )
            .optional_exists()?;
        if exists {
            return Ok(Outcome::Skipped("already ingested".into()));
        }
        let created_at = iso_from_mtime(&conn, path)?;
        let ingested_at = iso_now(&conn)?;
        (created_at, ingested_at)
    };

    let chunks = chunk_markdown(&markdown);
    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = state.sidecar.embed(&texts)?;
    check_embeddings(&embeddings, chunks.len())?;

    // Write the vault file (source of truth) before indexing. A freshly ingested
    // document enters the review queue: project Unsorted, no tags/importance,
    // `reviewed = false`, and `last_activity` seeded from ingest time. The on-disk
    // name (and whether the bytes are ciphertext) follows the vault's policy.
    let vault_name = cipher.on_disk_name(&vault_filename(&title, &content_hash));
    let front = Frontmatter {
        title: &title,
        source_path: &path.to_string_lossy(),
        ext: ext.as_deref(),
        content_hash: &content_hash,
        created_at: &created_at,
        ingested_at: &ingested_at,
        project: "Unsorted",
        tags: &[],
        importance: None,
        last_activity: &ingested_at,
        reviewed: false,
    };
    cipher.write_to(
        &vault.join(&vault_name),
        &render_markdown(&front, &markdown),
    )?;

    let meta = DocMeta {
        source_path: Some(path.to_string_lossy().into()),
        vault_path: vault_name,
        title,
        content_hash,
        ext,
        byte_size,
        created_at: Some(created_at),
        last_activity: Some(ingested_at.clone()),
        ingested_at,
        project: "Unsorted".into(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
    };
    let document = index_document(state, &meta, &chunks, &embeddings)?;
    Ok(Outcome::Indexed(document))
}

/// Drop the derived index and rebuild it from the Markdown vault. Proves the
/// store is reconstructable from disk (spec §3 acceptance).
pub fn rebuild(app: &AppHandle, on_event: Channel<IngestEvent>) -> Result<()> {
    let state = app.state::<AppState>();

    let _ = on_event.send(IngestEvent::Preparing {
        message: "Preparing the document engine…".into(),
    });
    state.sidecar.ensure_installed()?;

    {
        let conn = state.conn()?;
        // chunk_vec / chunks_fts cascade from chunks via our own inserts, so
        // clear them explicitly. documents → chunks cascades by FK.
        conn.execute_batch(
            "DELETE FROM chunks_fts; DELETE FROM chunk_vec; DELETE FROM chunks; DELETE FROM documents;",
        )?;
    }

    let (vault, cipher) = state.markdown_io()?;
    let (mut ingested, mut failed) = (0usize, 0usize);
    for entry in std::fs::read_dir(&vault)? {
        let path = entry?.path();
        // Accept both plaintext (`.md`) and encrypted (`.md.pmenc`) vault files; the
        // cipher decides per file how to read them (read-by-magic).
        if !is_vault_markdown(&path) {
            continue;
        }
        let name = file_name(&path);
        let _ = on_event.send(IngestEvent::Started {
            path: path.to_string_lossy().into(),
            name,
        });
        match rebuild_one(&state, &cipher, &path) {
            Ok(document) => {
                ingested += 1;
                let _ = on_event.send(IngestEvent::Done { document });
            }
            Err(e) => {
                failed += 1;
                let _ = on_event.send(IngestEvent::Failed {
                    path: path.to_string_lossy().into(),
                    error: e.to_string(),
                });
            }
        }
    }

    let _ = on_event.send(IngestEvent::Finished {
        ingested,
        skipped: 0,
        failed,
    });
    Ok(())
}

fn rebuild_one(state: &AppState, cipher: &MarkdownCipher, vault_file: &Path) -> Result<Document> {
    let raw = cipher.read(vault_file)?;
    let (fields, body) = parse_frontmatter(&raw)
        .ok_or_else(|| Error::Other("vault file missing front-matter".into()))?;

    let content_hash = fields
        .get("content_hash")
        .cloned()
        .unwrap_or_else(|| hex_digest(body.as_bytes()));
    let title = fields
        .get("title")
        .cloned()
        .unwrap_or_else(|| "Untitled".into());

    let chunks = chunk_markdown(body);
    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = state.sidecar.embed(&texts)?;
    check_embeddings(&embeddings, chunks.len())?;

    let ingested_at = match fields.get("ingested_at").cloned() {
        Some(value) => value,
        None => {
            let conn = state.conn()?;
            iso_now(&conn).unwrap_or_default()
        }
    };
    // Organisation metadata round-trips from the vault so a rebuild reproduces
    // the organised store (spec §3 acceptance). Missing fields fall back to the
    // fresh-ingest defaults, so pre-Step-4 vault files rebuild cleanly.
    let meta = DocMeta {
        source_path: fields.get("source_path").cloned(),
        vault_path: file_name(vault_file),
        title,
        content_hash,
        ext: fields.get("ext").cloned(),
        byte_size: None,
        created_at: fields.get("created_at").cloned(),
        project: fields
            .get("project")
            .cloned()
            .unwrap_or_else(|| "Unsorted".into()),
        tags: fields
            .get("tags")
            .map(|s| parse_yaml_list(s))
            .unwrap_or_default(),
        importance: nullable(fields.get("importance")),
        reviewed: fields
            .get("reviewed")
            .map(|v| v.trim() == "true")
            .unwrap_or(false),
        last_activity: fields
            .get("last_activity")
            .cloned()
            .or_else(|| Some(ingested_at.clone())),
        ingested_at,
    };
    index_document(state, &meta, &chunks, &embeddings)
}

/// Insert a document and its chunks/vectors/FTS rows in one transaction.
fn index_document(
    state: &AppState,
    meta: &DocMeta,
    chunks: &[Chunk],
    embeddings: &[Vec<f32>],
) -> Result<Document> {
    let mut conn = state.conn()?;
    let tx = conn.transaction()?;

    let tags_json =
        serde_json::to_string(&meta.tags).map_err(|e| Error::Other(format!("encode tags: {e}")))?;
    tx.execute(
        "INSERT INTO documents \
         (source_path, vault_path, title, content_hash, ext, byte_size, created_at, ingested_at, \
          project, tags, importance, reviewed, last_activity) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            meta.source_path,
            meta.vault_path,
            meta.title,
            meta.content_hash,
            meta.ext,
            meta.byte_size,
            meta.created_at,
            meta.ingested_at,
            meta.project,
            tags_json,
            meta.importance,
            meta.reviewed as i64,
            meta.last_activity,
        ],
    )?;
    let doc_id = tx.last_insert_rowid();

    for (i, chunk) in chunks.iter().enumerate() {
        tx.execute(
            "INSERT INTO chunks (document_id, ordinal, heading, content, char_count) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                doc_id,
                i as i64,
                chunk.heading,
                chunk.content,
                chunk.content.chars().count() as i64
            ],
        )?;
        let chunk_id = tx.last_insert_rowid();

        let vector = serde_json::to_string(&embeddings[i])
            .map_err(|e| Error::Other(format!("encode embedding: {e}")))?;
        tx.execute(
            "INSERT INTO chunk_vec (rowid, embedding) VALUES (?1, ?2)",
            params![chunk_id, vector],
        )?;
        tx.execute(
            "INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)",
            params![chunk_id, chunk.content],
        )?;
    }

    tx.commit()?;
    load_document(&conn, doc_id)
}

/// The SELECT column list backing `row_to_document` — shared by the list and
/// single-document loads so the two never drift.
const DOCUMENT_COLUMNS: &str = "d.id, d.title, d.source_path, d.ext, d.byte_size, \
     (SELECT count(*) FROM chunks c WHERE c.document_id = d.id), \
     d.created_at, d.ingested_at, d.project, d.tags, d.importance, d.reviewed, d.last_activity";

/// All documents, most-recent first, with their chunk counts.
pub fn list_documents(conn: &Connection) -> Result<Vec<Document>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d ORDER BY d.ingested_at DESC, d.id DESC"
    ))?;
    let rows = stmt
        .query_map([], row_to_document)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Documents still awaiting the sorting review (`reviewed = 0`), newest first.
pub fn review_queue(conn: &Connection) -> Result<Vec<Document>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.reviewed = 0 \
         ORDER BY d.ingested_at DESC, d.id DESC"
    ))?;
    let rows = stmt
        .query_map([], row_to_document)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn load_document(conn: &Connection, id: i64) -> Result<Document> {
    Ok(conn.query_row(
        &format!("SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.id = ?1"),
        params![id],
        row_to_document,
    )?)
}

fn row_to_document(row: &rusqlite::Row) -> rusqlite::Result<Document> {
    let tags_json: String = row.get(9)?;
    let reviewed: i64 = row.get(11)?;
    Ok(Document {
        id: row.get(0)?,
        title: row.get(1)?,
        source_path: row.get(2)?,
        ext: row.get(3)?,
        byte_size: row.get(4)?,
        chunk_count: row.get(5)?,
        created_at: row.get(6)?,
        ingested_at: row.get(7)?,
        project: row.get(8)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        importance: row.get(10)?,
        reviewed: reviewed != 0,
        last_activity: row.get(12)?,
    })
}

/// Rewrite a document's organisation metadata in place, *inside a caller-owned
/// transaction*: update the vault file's front-matter (preserving the body) and
/// the `documents` row. No re-chunk / re-embed — the body and `content_hash` are
/// unchanged, so the existing chunks and vectors stay valid.
///
/// Returns `(vault file, its prior raw on-disk bytes)` so the caller can restore the
/// file (via [`restore_vault_files`]) if a later step in the batch fails; the DB side
/// rolls back with `tx`. The snapshot is the *raw* bytes, not the decoded text — for an
/// encrypted vault the file is ciphertext, so restoring decoded text would corrupt it.
/// This is the building block for an all-or-nothing review commit — pass a
/// `rusqlite::Transaction` (it derefs to `&Connection`) and only commit it once every
/// document in the batch has been rewritten.
// The arguments are the metadata columns being rewritten, not a sign this should
// be split into smaller functions.
#[allow(clippy::too_many_arguments)]
pub fn rewrite_vault_metadata(
    tx: &Connection,
    vault: &Path,
    cipher: &MarkdownCipher,
    doc_id: i64,
    project: &str,
    tags: &[String],
    importance: Option<&str>,
    reviewed: bool,
    last_activity: &str,
) -> Result<(std::path::PathBuf, Vec<u8>)> {
    let vault_path: String = tx.query_row(
        "SELECT vault_path FROM documents WHERE id = ?1",
        params![doc_id],
        |r| r.get(0),
    )?;

    let file = vault.join(&vault_path);
    let original = cipher.read_raw(&file)?;
    let decoded = cipher.decode(&original, &file)?;
    let (fields, body) = parse_frontmatter(&decoded)
        .ok_or_else(|| Error::Other("vault file missing front-matter".into()))?;

    let front = Frontmatter {
        title: fields
            .get("title")
            .map(String::as_str)
            .unwrap_or("Untitled"),
        source_path: fields.get("source_path").map(String::as_str).unwrap_or(""),
        ext: fields
            .get("ext")
            .map(String::as_str)
            .filter(|s| !s.is_empty()),
        content_hash: fields.get("content_hash").map(String::as_str).unwrap_or(""),
        created_at: fields.get("created_at").map(String::as_str).unwrap_or(""),
        ingested_at: fields.get("ingested_at").map(String::as_str).unwrap_or(""),
        project,
        tags,
        importance,
        last_activity,
        reviewed,
    };
    cipher.write_to(&file, &render_markdown(&front, body))?;

    let tags_json =
        serde_json::to_string(tags).map_err(|e| Error::Other(format!("encode tags: {e}")))?;
    tx.execute(
        "UPDATE documents SET project = ?1, tags = ?2, importance = ?3, reviewed = ?4, \
         last_activity = ?5 WHERE id = ?6",
        params![
            project,
            tags_json,
            importance,
            reviewed as i64,
            last_activity,
            doc_id
        ],
    )?;
    Ok((file, original))
}

/// Restore vault files overwritten by [`rewrite_vault_metadata`] to their prior raw
/// bytes — the vault half of rolling back an abandoned metadata batch (the DB half
/// rolls back by dropping the uncommitted transaction). Writing back the exact
/// on-disk bytes keeps an encrypted file's ciphertext intact. Best-effort per file
/// (a failed restore on one shouldn't stop the rest); applied newest-first.
pub fn restore_vault_files(written: Vec<(std::path::PathBuf, Vec<u8>)>) {
    for (file, original) in written.into_iter().rev() {
        let _ = std::fs::write(&file, original);
    }
}

// --- chunking ---

pub struct Chunk {
    pub heading: Option<String>,
    pub content: String,
}

/// Split Markdown into overlapping chunks, greedily packing blocks (separated by
/// blank lines) up to `CHUNK_TARGET` chars and carrying `CHUNK_OVERLAP` chars of
/// context into the next chunk. Each chunk remembers the heading in force when
/// it started. Deterministic, so a rebuild reproduces identical chunks.
pub fn chunk_markdown(markdown: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut buf = String::new();
    let mut buf_heading: Option<String> = None;
    let mut current_heading: Option<String> = None;

    for block in blocks(markdown) {
        if let Some(h) = heading_text(&block) {
            current_heading = Some(h);
        }
        if buf.is_empty() {
            buf_heading = current_heading.clone();
        } else {
            buf.push_str("\n\n");
        }
        buf.push_str(&block);

        // Flush while the buffer is at/over the target. A loop (not a single
        // push) so a block larger than the target is split into target-sized
        // chunks rather than emitted whole — an oversized chunk gets silently
        // truncated by the embedder, dropping content. Measured in chars, not
        // bytes, so multi-byte text (e.g. CJK) isn't packed into ~1/3-size chunks.
        while char_count(&buf) >= CHUNK_TARGET {
            let head = take_chars(&mut buf, CHUNK_TARGET);
            push_chunk(&mut chunks, buf_heading.clone(), &head);
            // Carry an overlap tail of the emitted chunk into the next one.
            buf = format!("{}{}", overlap_tail(&head), buf);
            buf_heading = current_heading.clone();
        }
    }
    push_chunk(&mut chunks, buf_heading, &buf);
    chunks
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Remove and return the first `n` chars of `buf` (char-safe); the rest stays.
fn take_chars(buf: &mut String, n: usize) -> String {
    let end = buf
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(buf.len());
    let head = buf[..end].to_string();
    buf.replace_range(..end, "");
    head
}

fn push_chunk(chunks: &mut Vec<Chunk>, heading: Option<String>, buf: &str) {
    let content = buf.trim();
    if !content.is_empty() {
        chunks.push(Chunk {
            heading,
            content: content.to_string(),
        });
    }
}

/// Split into blocks on blank lines, trimming each.
fn blocks(markdown: &str) -> Vec<String> {
    markdown
        .split("\n\n")
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .map(String::from)
        .collect()
}

/// If a block is a heading line, return its text without the `#` markers.
fn heading_text(block: &str) -> Option<String> {
    let first = block.lines().next()?;
    let trimmed = first.trim_start();
    if trimmed.starts_with('#') {
        Some(trimmed.trim_start_matches('#').trim().to_string())
    } else {
        None
    }
}

/// The last `CHUNK_OVERLAP` chars of `buf` (char-counted, to match CHUNK_TARGET).
fn overlap_tail(buf: &str) -> String {
    let total = char_count(buf);
    buf.chars()
        .skip(total.saturating_sub(CHUNK_OVERLAP))
        .collect()
}

// --- front-matter ---

struct Frontmatter<'a> {
    title: &'a str,
    source_path: &'a str,
    ext: Option<&'a str>,
    content_hash: &'a str,
    created_at: &'a str,
    ingested_at: &'a str,
    /// Organisation metadata (Step 4). The vault is the source of truth, so these
    /// round-trip: `rebuild` reads them back to reproduce the organised store.
    project: &'a str,
    tags: &'a [String],
    importance: Option<&'a str>,
    last_activity: &'a str,
    reviewed: bool,
}

/// Render YAML front-matter + body. `tags` is written flow-style on one line
/// (`["a", "b"]`) so the flat parser reads it back as a single field.
fn render_markdown(f: &Frontmatter, body: &str) -> String {
    let tags = render_yaml_list(f.tags);
    let importance = f.importance.unwrap_or("null");
    format!(
        "---\n\
         title: {}\n\
         source_path: {}\n\
         ext: {}\n\
         content_hash: {}\n\
         created_at: {}\n\
         ingested_at: {}\n\
         project: {}\n\
         tags: {}\n\
         importance: {}\n\
         last_activity: {}\n\
         reviewed: {}\n\
         ---\n\n{}\n",
        yaml_quote(f.title),
        yaml_quote(f.source_path),
        f.ext.unwrap_or(""),
        f.content_hash,
        f.created_at,
        f.ingested_at,
        yaml_quote(f.project),
        tags,
        importance,
        f.last_activity,
        f.reviewed,
        body,
    )
}

/// Serialize a list of strings as a YAML flow sequence on one line.
fn render_yaml_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| yaml_quote(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// Parse our own front-matter back out: returns the simple key→value fields and
/// the body. Only the flat scalar fields we wrote are read (enough to rebuild).
fn parse_frontmatter(raw: &str) -> Option<(std::collections::HashMap<String, String>, &str)> {
    let rest = raw.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let header = &rest[..end];
    // Body starts after the closing fence and its trailing newline(s).
    let after = &rest[end + 4..];
    let body = after.trim_start_matches(['\r', '\n']);

    let mut fields = std::collections::HashMap::new();
    for line in header.lines() {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_string(), yaml_unquote(value.trim()));
        }
    }
    Some((fields, body))
}

/// Parse a YAML flow list (`["a", "b"]`) back into its elements. Tolerant: a
/// non-list value (or `[]`) yields an empty vec. Tags must not contain commas
/// (they're short labels) — the naive split assumes that.
fn parse_yaml_list(value: &str) -> Vec<String> {
    let v = value.trim();
    let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Vec::new();
    };
    inner
        .split(',')
        .map(|s| yaml_unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Read a front-matter scalar that may be the literal `null` → `None`.
fn nullable(value: Option<&String>) -> Option<String> {
    value
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "null")
        .map(String::from)
}

fn yaml_quote(value: &str) -> String {
    // Collapse control characters (notably CR/LF) to a space first. Our
    // front-matter parser is line-based, so a newline inside a value — e.g. a
    // document title taken from untrusted file content (an HTML <title>, PDF
    // metadata) — would otherwise inject extra YAML lines that `parse_frontmatter`
    // reads back as real fields on the next rebuild (rule #6). Then escape for the
    // quoted scalar.
    let single_line: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!(
        "\"{}\"",
        single_line.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn yaml_unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

// --- helpers ---

struct DocMeta {
    source_path: Option<String>,
    vault_path: String,
    title: String,
    content_hash: String,
    ext: Option<String>,
    byte_size: Option<i64>,
    created_at: Option<String>,
    ingested_at: String,
    project: String,
    tags: Vec<String>,
    importance: Option<String>,
    reviewed: bool,
    last_activity: Option<String>,
}

/// Bounds on the directory walk so a deep, huge, or symlink-looped tree can't
/// recurse without end — even though inputs come only from the user's own dialog
/// (a self-targeted, trusted source). Generous: far above any real drop.
const MAX_WALK_DEPTH: usize = 32;
const MAX_COLLECTED_FILES: usize = 100_000;

/// Recursively collect files from the given paths (folders are walked).
fn collect_files(inputs: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for input in inputs {
        collect_into(Path::new(input), &mut files, 0);
    }
    files
}

fn collect_into(path: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if out.len() >= MAX_COLLECTED_FILES {
        return;
    }
    // Below the top level, don't descend into directory symlinks: that avoids
    // cycles and escaping the chosen tree. A symlinked *file* still ingests, and
    // the top-level path the user explicitly picked is always honoured.
    if depth > 0 {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() && !path.is_file() {
                return;
            }
        }
    }
    if path.is_dir() {
        if depth >= MAX_WALK_DEPTH {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                collect_into(&entry.path(), out, depth + 1);
                if out.len() >= MAX_COLLECTED_FILES {
                    break;
                }
            }
        }
    } else if path.is_file() {
        out.push(path.to_path_buf());
    }
}

fn extension(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

/// Whether a file in the vault folder is a Markdown document to index: a plaintext
/// `.md` or an encrypted `.md.pmenc`. Anything else (temp files, stray) is skipped.
/// Shared with the plaintext-export command so both agree on what a vault file is.
pub(crate) fn is_vault_markdown(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("md") | Some("pmenc"))
}

/// Bring every Markdown file in `dir` into `write_with`'s policy, in place: decode each
/// file with `read_with` (read-by-magic, so it handles plaintext or the prior key),
/// re-encode with `write_with`, rename to the target on-disk name (`.md` <-> `.md.pmenc`),
/// and update its `documents.vault_path`. Returns how many files changed.
///
/// Idempotent — a file already in the target form is skipped — so it is safe to re-run
/// after an interruption (the mixed plaintext/ciphertext folder reads fine meanwhile).
/// For a plaintext -> encrypted conversion the two ciphers can be the same (decoding
/// plaintext needs no key). This is the Markdown half of a sharing/key migration; the
/// caller owns the DB transaction.
pub(crate) fn convert_markdown(
    conn: &Connection,
    dir: &Path,
    read_with: &MarkdownCipher,
    write_with: &MarkdownCipher,
) -> Result<usize> {
    let mut changed = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_file() || !is_vault_markdown(&path) {
            continue;
        }
        let old_name = file_name(&path);
        let new_name = write_with.on_disk_name(&MarkdownCipher::logical_name(&old_name));
        let raw = std::fs::read(&path)?;
        // Already in the target name AND encryption state? Nothing to do (idempotent).
        if new_name == old_name
            && crate::vault::crypto::is_encrypted(&raw) == write_with.encryption_on()
        {
            continue;
        }
        let content = read_with.decode(&raw, &path)?;
        write_with.write_to(&dir.join(&new_name), &content)?;
        if new_name != old_name {
            std::fs::remove_file(&path)?;
            conn.execute(
                "UPDATE documents SET vault_path = ?1 WHERE vault_path = ?2",
                params![new_name, old_name],
            )?;
        }
        changed += 1;
    }
    Ok(changed)
}

/// Export every Markdown file in `vault` to `dest` as plaintext `.md`, decrypting
/// encrypted files with `cipher` and dropping the `.pmenc` suffix. Returns the count
/// written. The core of the "never locked in" escape hatch — kept here (next to the
/// rebuild walk it mirrors) so it is unit-testable without a running app.
pub(crate) fn export_plaintext(
    vault: &Path,
    cipher: &MarkdownCipher,
    dest: &Path,
) -> Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut written = 0usize;
    for entry in std::fs::read_dir(vault)? {
        let path = entry?.path();
        if !path.is_file() || !is_vault_markdown(&path) {
            continue;
        }
        // Decrypt-if-needed, then write under the logical `.md` name (no `.pmenc`).
        let content = cipher.read(&path)?;
        let out_name = MarkdownCipher::logical_name(&file_name(&path));
        std::fs::write(dest.join(out_name), content)?;
        written += 1;
    }
    Ok(written)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into())
        .unwrap_or_default()
}

/// Prefer the converter's title; fall back to the file's stem.
fn pick_title(title: &str, path: &Path) -> String {
    let t = title.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().into())
        .unwrap_or_else(|| "Untitled".into())
}

/// `<slug>-<short hash>.md`, collision-resistant via the content hash.
fn vault_filename(title: &str, content_hash: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "untitled" } else { slug };
    // Char-safe short hash: in practice content_hash is a 64-char hex digest, but
    // take the first 12 chars defensively so a shorter/non-ASCII value can't panic.
    let short: String = content_hash.chars().take(12).collect();
    format!("{slug}-{short}.md")
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Guard the index against a model mismatch: one vector per chunk, each of the
/// pinned dimension.
fn check_embeddings(embeddings: &[Vec<f32>], chunks: usize) -> Result<()> {
    if embeddings.len() != chunks {
        return Err(Error::Other(
            "embedding count did not match chunk count".into(),
        ));
    }
    if embeddings.iter().any(|v| v.len() != EMBED_DIM) {
        return Err(Error::Other(format!(
            "embedding dimension mismatch (expected {EMBED_DIM}); wrong model?"
        )));
    }
    Ok(())
}

fn iso_now(conn: &Connection) -> Result<String> {
    Ok(
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?,
    )
}

fn iso_from_mtime(conn: &Connection, path: &Path) -> Result<String> {
    let secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    match secs {
        Some(s) => Ok(conn.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1, 'unixepoch')",
            params![s],
            |r| r.get(0),
        )?),
        None => iso_now(conn),
    }
}

/// Tiny helper so the dedupe check reads as a boolean.
trait OptionalExists {
    fn optional_exists(self) -> Result<bool>;
}
impl OptionalExists for std::result::Result<(), rusqlite::Error> {
    fn optional_exists(self) -> Result<bool> {
        match self {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(Error::from(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_round_trips_organisation_metadata() {
        // What the user confirms in review must survive a rebuild from disk.
        let tags = vec!["tax".to_string(), "2026".to_string()];
        let front = Frontmatter {
            title: "Q2 Report",
            source_path: "C:\\docs\\q2.pdf",
            ext: Some("pdf"),
            content_hash: "abc123def456",
            created_at: "2026-06-01T00:00:00.000Z",
            ingested_at: "2026-06-17T00:00:00.000Z",
            project: "Finances",
            tags: &tags,
            importance: Some("high"),
            last_activity: "2026-06-17T00:00:00.000Z",
            reviewed: true,
        };
        let rendered = render_markdown(&front, "Body text here.");
        let (fields, body) = parse_frontmatter(&rendered).unwrap();

        assert_eq!(fields.get("title").map(String::as_str), Some("Q2 Report"));
        assert_eq!(fields.get("project").map(String::as_str), Some("Finances"));
        assert_eq!(parse_yaml_list(fields.get("tags").unwrap()), tags);
        assert_eq!(nullable(fields.get("importance")).as_deref(), Some("high"));
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("true"));
        assert_eq!(body.trim_end(), "Body text here.");
    }

    #[test]
    fn frontmatter_handles_null_importance_and_empty_tags() {
        let front = Frontmatter {
            title: "Note",
            source_path: "",
            ext: None,
            content_hash: "hash",
            created_at: "",
            ingested_at: "",
            project: "Unsorted",
            tags: &[],
            importance: None,
            last_activity: "",
            reviewed: false,
        };
        let rendered = render_markdown(&front, "x");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();
        assert!(parse_yaml_list(fields.get("tags").unwrap()).is_empty());
        assert_eq!(nullable(fields.get("importance")), None);
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("false"));
    }

    #[test]
    fn metadata_batch_rolls_back_vault_and_db_together() {
        // A batch that fails partway must leave the vault file AND the DB row of
        // the already-processed doc exactly as they were (F2).
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let mut conn = crate::db::open(&dir.path().join("t.sqlite"), key).unwrap();

        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let front = Frontmatter {
            title: "T",
            source_path: "",
            ext: None,
            content_hash: "h",
            created_at: "",
            ingested_at: "",
            project: "Unsorted",
            tags: &[],
            importance: None,
            last_activity: "",
            reviewed: false,
        };
        let original = render_markdown(&front, "body");
        std::fs::write(vault.join("doc.md"), &original).unwrap();
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, tags, reviewed) \
             VALUES (1, 'doc.md', 'T', 'h', 'Unsorted', '[]', 0)",
            [],
        )
        .unwrap();

        // A plaintext (device) cipher: the rollback path must work the same either way.
        let cipher = MarkdownCipher::plaintext("test-vault");

        // Rewrite doc 1, then fail on a non-existent doc 2 → roll the batch back.
        let mut written = Vec::new();
        {
            let tx = conn.transaction().unwrap();
            written.push(
                rewrite_vault_metadata(
                    &tx,
                    &vault,
                    &cipher,
                    1,
                    "Finances",
                    &["tax".into()],
                    Some("high"),
                    true,
                    "2026-06-20",
                )
                .unwrap(),
            );
            assert!(
                rewrite_vault_metadata(&tx, &vault, &cipher, 2, "X", &[], None, true, "2026-06-20")
                    .is_err(),
                "missing doc should error"
            );
            // Caller abandons the batch: drop the tx (DB rollback) + restore files.
        }
        restore_vault_files(written);

        let project: String = conn
            .query_row("SELECT project FROM documents WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(project, "Unsorted", "DB row should be rolled back");
        assert_eq!(
            std::fs::read_to_string(vault.join("doc.md")).unwrap(),
            original,
            "vault file should be restored"
        );
    }

    #[test]
    fn oversized_block_is_split_to_target() {
        // A single block with no blank lines must not become one giant chunk the
        // embedder would truncate — it's split into target-sized pieces.
        let big = "x".repeat(CHUNK_TARGET * 3 + 50);
        let chunks = chunk_markdown(&big);
        assert!(chunks.len() > 1, "expected the giant block to be split");
        for c in &chunks {
            assert!(
                c.content.chars().count() <= CHUNK_TARGET,
                "chunk exceeds target: {} chars",
                c.content.chars().count()
            );
        }
    }

    #[test]
    fn multibyte_chunks_sized_by_chars_not_bytes() {
        // CJK chars are ~3 bytes each; sizing by bytes would chop chunks to ~1/3
        // the intended length. Size by chars instead.
        let text = "あ".repeat(CHUNK_TARGET + 200);
        let chunks = chunk_markdown(&text);
        let first = chunks[0].content.chars().count();
        assert!(
            first > CHUNK_TARGET / 2,
            "first chunk too small: {first} chars"
        );
        for c in &chunks {
            assert!(c.content.chars().count() <= CHUNK_TARGET);
        }
    }

    #[test]
    fn malicious_title_cannot_inject_frontmatter_fields() {
        // A title carrying a newline + a forged field (an attacker-controlled
        // value derived from ingested file content) must not become real fields
        // on rebuild — rule #6.
        let front = Frontmatter {
            title: "Pwned\"\nreviewed: true\nproject: Secret",
            source_path: "",
            ext: None,
            content_hash: "hash",
            created_at: "",
            ingested_at: "",
            project: "Unsorted",
            tags: &[],
            importance: None,
            last_activity: "",
            reviewed: false,
        };
        let rendered = render_markdown(&front, "body");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();
        // The forged lines stayed inside the title value; they didn't take effect.
        assert_eq!(fields.get("project").map(String::as_str), Some("Unsorted"));
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("false"));
    }
}
