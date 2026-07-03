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
use crate::model_gateway::ModelGateway;
use crate::photos::{self, PhotoRecord, PhotoSourceType};
use crate::registry::{self, ModelEntry};
use crate::retrieval_config::RetrievalConfig;
use crate::splitter::{self, ChunkKind, SplitMeta, Splitter};
use crate::spreadsheets::{self, SpreadsheetRecord};
use crate::vault::MarkdownCipher;
use crate::AppState;

/// Extensions MarkItDown handles well. Anything else is skipped (still findable
/// on disk, just not ingested). Lower-case, no dot. Spreadsheet types (`xlsx`/`xls`/`csv`) are
/// DELIBERATELY absent — they route to [`ingest_spreadsheet`] instead (a dedicated processor that
/// bypasses MarkItDown), the same way `PHOTO_EXTS` routes images to the photo pipeline.
const SUPPORTED: &[&str] = &[
    "pdf", "docx", "pptx", "doc", "ppt", "html", "htm", "json", "xml", "txt", "md", "markdown",
    "rtf", "epub", "png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp",
];

/// Image extensions routed to the dedicated photo pipeline ([`ingest_photo`]) instead of the
/// MarkItDown document path — they get OCR + EXIF, a `photos` row, and the synthetic photo body.
/// Deliberately the spec's set; gif/bmp/tiff stay on the (no-op) document path. `heic` is here but
/// NOT in `SUPPORTED`, so a HEIC only ingests via this branch.
const PHOTO_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "heic"];

/// Spreadsheet extensions routed to the dedicated spreadsheet processor ([`ingest_spreadsheet`])
/// instead of MarkItDown — the sidecar parses them values-only into a metadata chunk + self-describing
/// row chunks (see [`crate::spreadsheets`]). Like `PHOTO_EXTS`, these are NOT in `SUPPORTED`, so a
/// spreadsheet only ingests via this branch and can never fall back to a MarkItDown pipe-table dump.
const SPREADSHEET_EXTS: &[&str] = &["xlsx", "xls", "csv"];

/// Per-run ingest options threaded from the command. `copy_photos_to_vault` is the drag-drop opt-in
/// to save an original image into `vault/photos/` (default off).
#[derive(Clone, Copy, Default)]
pub struct IngestOpts {
    pub copy_photos_to_vault: bool,
}

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
    /// `"vault"` (a fully-stored document) or `"index_only"` (a pointer we index but don't hold) —
    /// the UI badges the difference. `external_ref` is the source URL/id shown for an index-only
    /// item in place of a local `source_path`; `source_state` flags whether its body is reachable.
    pub source_type: String,
    pub source_state: String,
    pub external_ref: Option<String>,
    /// The stable source id for an index-only item (`None` for a vault document) — its manifest key
    /// and the handle the observe-and-react layer targets.
    pub source_id: Option<String>,
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
    /// The number of files this run will work through — sent once, right before the
    /// first `Started`, so the UI can show a determinate bar. Setup + model download
    /// happen before this and stay indeterminate (no known total yet).
    Counted {
        total: usize,
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
pub fn run(
    app: &AppHandle,
    inputs: Vec<String>,
    opts: IngestOpts,
    on_event: Channel<IngestEvent>,
) -> Result<()> {
    let state = app.state::<AppState>();

    // Indexing is active use — hold the idle chat-indexer (card 7B) off so it doesn't contend with it.
    state.mark_user_activity();

    let _ = on_event.send(IngestEvent::Preparing {
        message: "Preparing the document engine…".into(),
    });
    state.sidecar.ensure_installed()?;

    // Resolve this vault's embedder once for the whole run and refuse a width the live vector index
    // can't hold: incremental ingest can't resize the table, so a vault whose selected language no
    // longer matches its index (switched but not yet re-indexed) is sent to the Re-index flow
    // rather than producing wrong-width vectors. Build the gateway so every chunk is sized and
    // embedded with the vault's chosen model.
    let (embedder, embed_batch) = {
        let conn = state.conn()?;
        let embedder = crate::db::selected_embedder(&conn)?;
        guard_dimension(&conn, &embedder)?;
        // Resolve the gentle-mode embedding batch cap (the memory lever) once for the run.
        (embedder, crate::db::indexing_embed_batch(&conn))
    };
    let gateway = ModelGateway::new(
        &state.sidecar,
        embedder.clone(),
        registry::reranker_for(&embedder),
    )
    .with_embed_batch(embed_batch);

    // The vault's Markdown dir + cipher for this whole run (they don't change mid-run).
    // Snapshotting up front means we never hold the vault lock across a sidecar call.
    let (vault, cipher) = state.markdown_io()?;
    let files = collect_files(&inputs);
    let _ = on_event.send(IngestEvent::Counted { total: files.len() });

    // If the vault has no documents yet, everything this run indexes is produced under the
    // current retrieval config, so we can stamp it at the end and spare the user a rebuild
    // prompt for an already-correct index. With pre-existing docs the index may be mixed
    // (e.g. a pre-stamp vault), so we leave the stamp untouched until a full Rebuild.
    let was_empty = {
        let conn = state.conn()?;
        let n: i64 = conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0))?;
        n == 0
    };

    let (mut ingested, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    for path in files {
        let name = file_name(&path);
        let _ = on_event.send(IngestEvent::Started {
            path: path.to_string_lossy().into(),
            name,
        });

        match ingest_one(&state, &gateway, &vault, &cipher, &path, opts) {
            Ok(Outcome::Indexed(document)) => {
                ingested += 1;
                let _ = on_event.send(IngestEvent::Done { document });
                // Gentle mode: breathe between files so indexing doesn't pin the CPU continuously.
                // Re-read each file (cheap) so flipping Fast/Gentle mid-import takes effect at once.
                let pause_ms = {
                    let conn = state.conn()?;
                    crate::db::indexing_pause_ms(&conn)
                };
                if pause_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(pause_ms));
                }
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

    if was_empty {
        if let Ok(conn) = state.conn() {
            let _ = crate::db::set_retrieval_stamp(&conn, &RetrievalConfig::current_for(&embedder));
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
    gateway: &ModelGateway<'_>,
    vault: &Path,
    cipher: &MarkdownCipher,
    path: &Path,
    opts: IngestOpts,
) -> Result<Outcome> {
    let ext = extension(path);
    // Images go through the photo pipeline (OCR + EXIF + a `photos` row), not MarkItDown.
    if matches!(&ext, Some(e) if PHOTO_EXTS.contains(&e.as_str())) {
        return ingest_photo(state, gateway, vault, cipher, path, ext.as_deref(), opts);
    }
    // Spreadsheets go through the dedicated processor (values-only parse + a `spreadsheets` row + the
    // synthetic sheet body), not MarkItDown — same bypass shape as photos.
    if matches!(&ext, Some(e) if SPREADSHEET_EXTS.contains(&e.as_str())) {
        return ingest_spreadsheet(state, gateway, vault, cipher, path, ext.as_deref());
    }
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

    let chunks = split_document(gateway, &markdown, &title, &content_hash)?;
    let texts = leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;

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
        photo: None,
        spreadsheet: None,
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
        source: SourceMeta::default(),
    };
    let document = index_document(state, &meta, &chunks, &embeddings, None, None)?;
    Ok(Outcome::Indexed(document))
}

/// Ingest one image: OCR + EXIF via the sidecar, then the SAME chunk/embed/index pipeline as a
/// document, via a synthetic Markdown body (a metadata chunk + the OCR text). The image's bytes are
/// the identity (SHA-256 = both the dedupe `content_hash` and `photos.file_hash`), so a moved/renamed
/// file still dedupes. OCR is requested only when the optional component is installed; declining it
/// still ingests the photo with its EXIF metadata chunk. With `copy_photos_to_vault`, the original is
/// copied into `vault/photos/` first (following the vault cipher) and recorded on the `photos` row —
/// this also applies on a dedupe hit (a re-drop with the opt-in newly checked saves the copy without
/// re-indexing).
fn ingest_photo(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault: &Path,
    cipher: &MarkdownCipher,
    path: &Path,
    ext: Option<&str>,
    opts: IngestOpts,
) -> Result<Outcome> {
    // The image bytes are read once: they are the identity hash, the dedupe key, and (if opted in)
    // what gets copied into the vault.
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Ok(Outcome::Skipped("empty image file".into()));
    }
    let file_hash = hex_digest(&bytes);
    let byte_size = bytes.len() as i64;

    // Dedupe + ingest timestamp in one short lock; release before the slow OCR/embed.
    let ingested_at = {
        let conn = state.conn()?;
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM documents WHERE content_hash = ?1",
                params![file_hash],
                |_| Ok(()),
            )
            .optional_exists()?;
        if exists {
            // A dedupe hit, but the user may have re-dropped this image with "save a copy" now
            // checked (e.g. a screenshot they want to keep before deleting it). Honor that opt-in
            // even though we skip re-indexing: copy the original (idempotent — named by hash) and
            // flip the existing photos row's saved_to_vault/vault_path so the record reflects it.
            if opts.copy_photos_to_vault {
                let rel = copy_original_to_vault(vault, cipher, &bytes, &file_hash, ext)?;
                conn.execute(
                    "UPDATE photos SET saved_to_vault = 1, vault_path = ?1 WHERE file_hash = ?2",
                    params![rel, file_hash],
                )?;
                return Ok(Outcome::Skipped(
                    "already ingested — saved a copy to the vault".into(),
                ));
            }
            return Ok(Outcome::Skipped("already ingested".into()));
        }
        iso_now(&conn)?
    };

    // OCR only if the optional component is installed (the UI prompts to enable it); EXIF/dimensions
    // come back either way. No DB lock held across this call.
    let run_ocr = state.sidecar.optional_ocr_ready();
    let analysis = state.sidecar.analyze_image(path, run_ocr)?;

    let capture_date =
        photos::resolve_capture_date(analysis.capture_date.as_deref(), path, &ingested_at);
    let has_camera_exif = analysis.lat.is_some() || analysis.capture_date.is_some();
    let source_type = photos::infer_source_type(path, has_camera_exif);
    let ocr_text = {
        let t = analysis.ocr_text.trim();
        (!t.is_empty()).then(|| t.to_string())
    };

    let title = photos::photo_title(source_type, &capture_date);
    let body = photos::photo_markdown(
        source_type,
        &capture_date,
        analysis.lat,
        analysis.lon,
        ocr_text.as_deref().unwrap_or(""),
    );

    // Opt-in: copy the original into vault/photos/ (per the vault cipher) before writing the record.
    let (saved_to_vault, image_vault_path) = if opts.copy_photos_to_vault {
        let rel = copy_original_to_vault(vault, cipher, &bytes, &file_hash, ext)?;
        (true, Some(rel))
    } else {
        (false, None)
    };

    let chunks = split_document(gateway, &body, &title, &file_hash)?;
    let texts = leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;

    let photo = PhotoRecord {
        source_path: Some(path.to_string_lossy().into()),
        source_type,
        capture_date: capture_date.clone(),
        file_hash: file_hash.clone(),
        ocr_text,
        saved_to_vault,
        vault_path: image_vault_path,
        width: analysis.width,
        height: analysis.height,
        lat: analysis.lat,
        lon: analysis.lon,
    };

    // Write the vault truth (frontmatter + synthetic body) before indexing, like a document. The
    // capture date doubles as `created_at` so it round-trips on rebuild; the photo block carries the
    // rest. last_activity is seeded from ingest time.
    let vault_name = cipher.on_disk_name(&vault_filename(&title, &file_hash));
    let front = Frontmatter {
        title: &title,
        source_path: &path.to_string_lossy(),
        ext,
        content_hash: &file_hash,
        created_at: &capture_date,
        ingested_at: &ingested_at,
        project: "Unsorted",
        tags: &[],
        importance: None,
        last_activity: &ingested_at,
        reviewed: false,
        photo: Some(&photo),
        spreadsheet: None,
    };
    cipher.write_to(&vault.join(&vault_name), &render_markdown(&front, &body))?;

    let meta = DocMeta {
        source_path: Some(path.to_string_lossy().into()),
        vault_path: vault_name,
        title,
        content_hash: file_hash,
        ext: ext.map(str::to_string),
        byte_size: Some(byte_size),
        created_at: Some(capture_date),
        last_activity: Some(ingested_at.clone()),
        ingested_at,
        project: "Unsorted".into(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        source: SourceMeta::photo(),
    };
    let document = index_document(state, &meta, &chunks, &embeddings, Some(&photo), None)?;
    Ok(Outcome::Indexed(document))
}

/// Ingest one spreadsheet: parse it values-only via the sidecar, shape it into a synthetic Markdown
/// body (a metadata chunk + self-describing row chunks per sheet — [`crate::spreadsheets`]), then the
/// SAME chunk/embed/index pipeline as a document — bypassing MarkItDown, exactly as [`ingest_photo`]
/// bypasses it for images. The synthetic body is the vault truth, so a Rebuild reconstructs the
/// spreadsheet (and its `spreadsheets` satellite row, via the frontmatter block) without re-parsing the
/// original file. `content_hash` is the hash of the synthetic body, so an edited re-drop re-ingests.
fn ingest_spreadsheet(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault: &Path,
    cipher: &MarkdownCipher,
    path: &Path,
    ext: Option<&str>,
) -> Result<Outcome> {
    // Parse values-only in the sidecar (no DB lock held), then shape to Markdown Rust-side.
    let sheets = state.sidecar.analyze_spreadsheet(path, ext.unwrap_or(""))?;
    let Some((body, record)) = spreadsheets::to_markdown(&sheets) else {
        return Ok(Outcome::Skipped("no extractable rows".into()));
    };

    let title = pick_title("", path);
    let content_hash = hex_digest(body.as_bytes());
    let byte_size = std::fs::metadata(path).map(|m| m.len() as i64).ok();

    // Dedupe + timestamps in one short lock; release before embedding.
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

    let chunks = split_document(gateway, &body, &title, &content_hash)?;
    let texts = leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;

    // Write the vault truth (frontmatter + synthetic body) before indexing, like a document. The
    // spreadsheet block carries the satellite counts so a Rebuild reconstructs the `spreadsheets` row.
    let vault_name = cipher.on_disk_name(&vault_filename(&title, &content_hash));
    let front = Frontmatter {
        title: &title,
        source_path: &path.to_string_lossy(),
        ext,
        content_hash: &content_hash,
        created_at: &created_at,
        ingested_at: &ingested_at,
        project: "Unsorted",
        tags: &[],
        importance: None,
        last_activity: &ingested_at,
        reviewed: false,
        photo: None,
        spreadsheet: Some(&record),
    };
    cipher.write_to(&vault.join(&vault_name), &render_markdown(&front, &body))?;

    let meta = DocMeta {
        source_path: Some(path.to_string_lossy().into()),
        vault_path: vault_name,
        title,
        content_hash,
        ext: ext.map(str::to_string),
        byte_size,
        created_at: Some(created_at),
        last_activity: Some(ingested_at.clone()),
        ingested_at,
        project: "Unsorted".into(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        source: SourceMeta::spreadsheet(),
    };
    let document = index_document(state, &meta, &chunks, &embeddings, None, Some(&record))?;
    Ok(Outcome::Indexed(document))
}

/// Copy a photo's original bytes into `vault/photos/<hash>.<ext>` following the vault cipher (so an
/// encrypted vault keeps the image encrypted at rest). Returns the vault-relative on-disk path stored
/// on the `photos` row. Named by content hash so re-saving the same image is idempotent.
fn copy_original_to_vault(
    vault: &Path,
    cipher: &MarkdownCipher,
    bytes: &[u8],
    file_hash: &str,
    ext: Option<&str>,
) -> Result<String> {
    let dir = vault.join("photos");
    std::fs::create_dir_all(&dir)?;
    let on_disk = cipher.on_disk_name(&format!("{file_hash}.{}", ext.unwrap_or("img")));
    cipher.write_bytes_to(&dir.join(&on_disk), bytes)?;
    Ok(format!("photos/{on_disk}"))
}

/// Drop the derived index and rebuild it from the Markdown vault. Proves the
/// store is reconstructable from disk (spec §3 acceptance).
pub fn rebuild(app: &AppHandle, on_event: Channel<IngestEvent>) -> Result<()> {
    let state = app.state::<AppState>();

    // Indexing is active use — hold the idle chat-indexer (card 7B) off so it doesn't contend with it.
    state.mark_user_activity();

    let _ = on_event.send(IngestEvent::Preparing {
        message: "Preparing the document engine…".into(),
    });
    state.sidecar.ensure_installed()?;

    // Resolve the embedder + its paired reranker before touching the store, plus the gentle-mode
    // embedding batch cap — a full rebuild is the heaviest index-time op, so the memory lever matters
    // most here.
    let (embedder, embed_batch) = {
        let conn = state.conn()?;
        (
            crate::db::selected_embedder(&conn)?,
            crate::db::indexing_embed_batch(&conn),
        )
    };
    let gateway = ModelGateway::new(
        &state.sidecar,
        embedder.clone(),
        registry::reranker_for(&embedder),
    )
    .with_embed_batch(embed_batch);

    // WARMUP-BEFORE-DESTROY: load the embedder — downloading a non-bundled model on first use — and
    // prove it actually emits the expected width *before* clearing the old index. If the user is
    // offline, or the model's ONNX export is the wrong dimension, this fails here with the existing
    // index fully intact, never after the store is wiped. A non-bundled model can be ~1 GB, so flag
    // the one-time download; the bundled English path (model_file: None) stays silent.
    if embedder.model_file.is_some() {
        let _ = on_event.send(IngestEvent::Preparing {
            message: "Downloading the multilingual model (~1 GB, one time)…".into(),
        });
    }
    let warm = gateway.embed_documents(&["warm".to_string()])?;
    let got = warm.first().map(|v| v.len());
    if !warmup_ok(got, embedder.dimension) {
        return Err(Error::Other(format!(
            "the '{}' search model returned {}-dimensional vectors, expected {}; your vault was \
             left unchanged",
            embedder.id,
            got.unwrap_or(0),
            embedder.dimension
        )));
    }

    // The model is confirmed ready — only now is it safe to be destructive. Clear the store
    // (chunk_vec / chunks_fts are cleared explicitly; documents → chunks cascades by FK) and resize
    // the now-empty vector column to this embedder's width before re-embedding.
    {
        let conn = state.conn()?;
        conn.execute_batch(
            "DELETE FROM chunks_fts; DELETE FROM chunk_vec; DELETE FROM chunks; DELETE FROM documents;",
        )?;
        crate::db::ensure_vec_dim(&conn, embedder.dimension)?;
    }
    let (vault, cipher) = state.markdown_io()?;
    // Collect the vault-markdown files up front so we know the total before the loop — the UI
    // shows a determinate bar from this count. Accept both plaintext (`.md`) and encrypted
    // (`.md.pmenc`) files; the cipher decides per file how to read them (read-by-magic). An
    // unreadable dir entry is skipped rather than aborting the whole rebuild.
    let files: Vec<PathBuf> = std::fs::read_dir(&vault)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| is_vault_markdown(path))
        .collect();
    let _ = on_event.send(IngestEvent::Counted { total: files.len() });
    let (mut ingested, mut failed) = (0usize, 0usize);
    for path in files {
        let name = file_name(&path);
        let _ = on_event.send(IngestEvent::Started {
            path: path.to_string_lossy().into(),
            name,
        });
        // A chat session's `.md` must round-trip its chat IDENTITY (source_type/source_id, per-chunk
        // turn pointer + timestamp, and the session→document link), not re-index as a plain document —
        // otherwise citations lose their jump-to-turn and a later idle sweep births a duplicate. Route it
        // through the live chat engine, which reads the (un-wiped) messages table and rebuilds all of that.
        let outcome = if is_chat_vault_file(&cipher, &path) {
            rebuild_chat(&state, &cipher, &path)
        } else {
            rebuild_one(&state, &gateway, &cipher, &path).map(Some)
        };
        match outcome {
            Ok(Some(document)) => {
                ingested += 1;
                let _ = on_event.send(IngestEvent::Done { document });
            }
            // A chat that only ever exchanged small talk indexes no substantive turns, so it (correctly)
            // births no document — count it done, but there is nothing to surface.
            Ok(None) => {
                ingested += 1;
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

    // The index now reflects the current retrieval config end-to-end, so stamp the vault —
    // this clears the one-time "Rebuild recommended" prompt.
    {
        let conn = state.conn()?;
        crate::db::set_retrieval_stamp(&conn, &RetrievalConfig::current_for(&embedder))?;
    }

    // Rebuild re-resolved every document's entity from its frontmatter canonical; push any
    // resulting entity change out to the portable rules file (a no-op when nothing changed).
    state.sync_entity_rules();

    // Index-only documents have no Markdown file, so the walk above skipped them; restore them from
    // the encrypted manifest, re-embedded from their stored summaries (their bodies are remote). The
    // gateway is already warmed and `chunk_vec` already sized, so this reuses both.
    if let Ok((vault_root, manifest_cipher)) = state.manifest_io() {
        match crate::index_only::rebuild_from_manifest(
            &state,
            &gateway,
            &vault_root,
            &manifest_cipher,
        ) {
            Ok((restored, idx_failed)) => {
                ingested += restored;
                failed += idx_failed;
            }
            Err(e) => eprintln!("index_only: manifest rebuild skipped ({e})"),
        }
    }

    let _ = on_event.send(IngestEvent::Finished {
        ingested,
        skipped: 0,
        failed,
    });
    Ok(())
}

fn rebuild_one(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    cipher: &MarkdownCipher,
    vault_file: &Path,
) -> Result<Document> {
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

    let chunks = split_document(gateway, body, &title, &content_hash)?;
    let texts = leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;

    let ingested_at = match fields.get("ingested_at").cloned() {
        Some(value) => value,
        None => {
            let conn = state.conn()?;
            iso_now(&conn).unwrap_or_default()
        }
    };
    // A photo round-trips its satellite row from the front-matter photo block (OCR is never re-run —
    // the text is already in the vault body); a spreadsheet likewise round-trips its counts, so a
    // Rebuild reconstructs the `spreadsheets` row without re-parsing the original file.
    let photo = photo_from_fields(&fields, &content_hash, body);
    let spreadsheet = spreadsheet_from_fields(&fields);

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
        source: if photo.is_some() {
            SourceMeta::photo()
        } else if spreadsheet.is_some() {
            SourceMeta::spreadsheet()
        } else {
            SourceMeta::default()
        },
    };
    index_document(
        state,
        &meta,
        &chunks,
        &embeddings,
        photo.as_ref(),
        spreadsheet.as_ref(),
    )
}

/// Does this vault file carry a chat session (`source_type: chat`)? Cheap front-matter peek so the rebuild
/// loop can route chats through the chat engine (which preserves their identity) rather than the generic
/// document path. An unreadable/headerless file is treated as non-chat (it falls to `rebuild_one`, which
/// reports the real error).
fn is_chat_vault_file(cipher: &MarkdownCipher, vault_file: &Path) -> bool {
    cipher
        .read(vault_file)
        .ok()
        .and_then(|raw| parse_frontmatter(&raw).map(|(fields, _)| fields))
        .and_then(|fields| fields.get("source_type").cloned())
        .as_deref()
        == Some(SOURCE_TYPE_CHAT)
}

/// Rebuild a chat session from its vault `.md`. Rebuild wiped `documents` + `chunks` (FK-nulling
/// `chat_sessions.document_id`), but left `conversations` / `messages` / `chat_sessions` intact — so the
/// authored turns are still the source of truth. We reset the index cursor to NULL and re-run the SAME
/// engine the live/idle indexer uses ([`chat_index::index_session`]): it re-births the `documents` row with
/// the chat's `source_type`/`source_id`, re-appends every completed turn-pair stamping per-chunk
/// `chat_turn_id` + `chunk_at`, and re-links `chat_sessions.document_id`. That is what keeps chat citations
/// (jump-to-turn) and per-chunk recency intact across a Rebuild, and stops the next idle sweep from birthing
/// a duplicate document. Returns the re-birthed [`Document`], or `None` for a chat with no substantive turns
/// (small-talk-only ⇒ no document, by design).
fn rebuild_chat(
    state: &AppState,
    cipher: &MarkdownCipher,
    vault_file: &Path,
) -> Result<Option<Document>> {
    let raw = cipher.read(vault_file)?;
    let (fields, _body) = parse_frontmatter(&raw)
        .ok_or_else(|| Error::Other("chat vault file missing front-matter".into()))?;
    let conversation_id: i64 = fields
        .get("chat_conversation_id")
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| Error::Other("chat vault file missing chat_conversation_id".into()))?;

    {
        let conn = state.conn()?;
        conn.execute(
            "UPDATE chat_sessions SET document_id = NULL, last_indexed_turn_id = NULL \
             WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
    }
    crate::chat_index::index_session(state, conversation_id)?;

    // The session row exists (its vault file implies an earlier `record_turn_pair` upsert). document_id is
    // NULL again only if index_session found nothing substantive to index (small-talk-only chat).
    let conn = state.conn()?;
    let doc_id: Option<i64> = conn.query_row(
        "SELECT document_id FROM chat_sessions WHERE conversation_id = ?1",
        params![conversation_id],
        |r| r.get(0),
    )?;
    let Some(id) = doc_id else {
        return Ok(None);
    };

    // index_session re-births the row classified from the chat's ORIGIN scope (`chat_doc_meta`), which would
    // discard a user's later filing/archiving. The vault front-matter is a chat's organisational TRUTH (card
    // F, like any document), so restore project/tags/importance/reviewed from the file — and re-resolve the
    // entity from the project name exactly as `insert_document_row` does, since the ids are an index detail.
    let project = fields
        .get("project")
        .cloned()
        .unwrap_or_else(|| "Unsorted".into());
    let tags: Vec<String> = fields
        .get("tags")
        .map(|s| parse_yaml_list(s))
        .unwrap_or_default();
    let importance = nullable(fields.get("importance"));
    let reviewed = fields
        .get("reviewed")
        .map(|v| v.trim() == "true")
        .unwrap_or(false);
    let entity_id = crate::entities::resolve_project(&conn, &project, true)?;
    conn.execute(
        "UPDATE documents SET project = ?1, tags = ?2, importance = ?3, reviewed = ?4, entity_id = ?5 \
         WHERE id = ?6",
        params![
            project,
            serde_json::to_string(&tags).map_err(|e| Error::Other(format!("encode tags: {e}")))?,
            importance,
            reviewed as i64,
            entity_id,
            id
        ],
    )?;
    Ok(Some(load_document(&conn, id)?))
}

/// Insert a document and its chunks/vectors/FTS rows in one transaction. `embeddings` are the
/// leaf embeddings in leaf order (parents are structural-only and carry none); the loop pairs
/// them with leaves as it walks the chunk list. The splitter emits parents before their
/// children, so a single ordered pass resolves `parent_uid` → row id from a uid map.
pub(crate) fn index_document(
    state: &AppState,
    meta: &DocMeta,
    chunks: &[splitter::Chunk],
    embeddings: &[Vec<f32>],
    photo: Option<&PhotoRecord>,
    spreadsheet: Option<&SpreadsheetRecord>,
) -> Result<Document> {
    let mut conn = state.conn()?;
    let tx = conn.transaction()?;

    let doc_id = insert_document_row(&tx, meta)?;

    // A photo carries an extra satellite row (its capture/OCR/copy truth), written in the SAME
    // transaction so a document and its photo row are always consistent. `visual_description` is left
    // to its NULL default (reserved for Stage-4 image understanding — no writer this stage).
    if let Some(p) = photo {
        tx.execute(
            "INSERT INTO photos \
             (document_id, source_path, source_type, capture_date, file_hash, ocr_text, \
              saved_to_vault, vault_path, width, height, lat, lon) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                doc_id,
                p.source_path,
                p.source_type.as_str(),
                p.capture_date,
                p.file_hash,
                p.ocr_text,
                p.saved_to_vault as i64,
                p.vault_path,
                p.width,
                p.height,
                p.lat,
                p.lon,
            ],
        )?;
    }

    // A spreadsheet carries an extra satellite row (its sheet/row counts + the truncation record),
    // written in the SAME transaction so a document and its spreadsheet row are always consistent.
    // `structured_data_summary` is left to its NULL default — reserved for later column-type/aggregate
    // enrichment, with no writer this card (parallel to `photos.visual_description`).
    if let Some(sp) = spreadsheet {
        tx.execute(
            "INSERT INTO spreadsheets (document_id, sheet_count, total_rows, chunked_rows) \
             VALUES (?1, ?2, ?3, ?4)",
            params![doc_id, sp.sheet_count, sp.total_rows, sp.chunked_rows],
        )?;
    }

    insert_chunks(
        &tx,
        doc_id,
        chunks,
        embeddings,
        meta.source.is_index_only(),
        meta.source.stored_summary.as_deref(),
    )?;

    tx.commit()?;
    load_document(&conn, doc_id)
}

/// Insert just the `documents` row from a [`DocMeta`] (resolving its entity from the canonical project
/// name) and return its id. The row-creation half of [`index_document`], extracted so a source whose
/// chunks land separately — the append-only chat indexer (card B), which births the row on first index
/// then appends chunks turn-pair by turn-pair — can reuse the exact same INSERT and entity resolution.
/// Caller owns the transaction.
pub(crate) fn insert_document_row(tx: &Connection, meta: &DocMeta) -> Result<i64> {
    let tags_json =
        serde_json::to_string(&meta.tags).map_err(|e| Error::Other(format!("encode tags: {e}")))?;
    // Resolve the document's entity from its canonical project name (creating one only if it is a
    // genuinely new project — "Unsorted" and any reviewed canonical already exist). On a rebuild
    // this is what reassigns `entity_id` from the frontmatter name (the ids are an index detail).
    let entity_id = crate::entities::resolve_project(tx, &meta.project, true)?;
    // `source_account` is the owning account promoted out of `source_id`'s inline
    // `gdrive:<email>:<fileId>` encoding into its own filterable column, derived here at the single
    // insert seam so every insert path (fresh sync + Rebuild-from-manifest) fills it identically and it
    // self-heals whenever the row is re-created. NULL where the id carries no account (vault,
    // shared-drive, OneDrive, chat); `source_id` itself is left untouched.
    let source_account = meta
        .source
        .source_id
        .as_deref()
        .and_then(crate::drive::account_of);
    tx.execute(
        "INSERT INTO documents \
         (source_path, vault_path, title, content_hash, ext, byte_size, created_at, ingested_at, \
          project, tags, importance, reviewed, last_activity, entity_id, \
          source_type, source_state, source_id, external_ref, source_modified_at, \
          source_content_hash, stored_summary, \
          source_parent_folder_id, source_parent_folder_name, source_account) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
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
            entity_id,
            meta.source.source_type,
            meta.source.source_state,
            meta.source.source_id,
            meta.source.external_ref,
            meta.source.source_modified_at,
            meta.source.source_content_hash,
            meta.source.stored_summary,
            meta.source.source_parent_folder_id,
            meta.source.source_parent_folder_name,
            source_account,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Append one turn-pair's freshly-split chunks to an existing chat document, **continuing** its ordinal
/// sequence rather than replacing anything — the append-only model card B requires (old chunks are never
/// re-split or re-embedded). Mirrors [`insert_chunks`]'s rowid invariants (`chunk_vec`/`chunks_fts` keyed
/// by `chunks.id`, only leaves embedded/indexed) but stamps every row with this pair's `chat_turn_id`
/// (the navigation/citation pointer) and `chunk_at` (the per-chunk recency timestamp). Returns the next
/// free ordinal so a caller can append several pairs in one transaction. Caller owns the transaction.
pub(crate) fn append_chat_chunks(
    tx: &Connection,
    doc_id: i64,
    start_ordinal: i64,
    chunks: &[splitter::Chunk],
    embeddings: &[Vec<f32>],
    chat_turn_id: i64,
    chunk_at: &str,
) -> Result<i64> {
    let mut uid_to_id: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut leaf_idx = 0usize;
    let mut ordinal = start_ordinal;
    for chunk in chunks {
        let parent_id = chunk
            .parent_uid
            .as_deref()
            .and_then(|uid| uid_to_id.get(uid).copied());
        tx.execute(
            "INSERT INTO chunks \
             (document_id, ordinal, heading, content, char_count, uid, parent_id, kind, \
              start_offset, end_offset, chat_turn_id, chunk_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                doc_id,
                ordinal,
                chunk.heading,
                chunk.display_content,
                chunk.display_content.chars().count() as i64,
                chunk.uid,
                parent_id,
                chunk.kind.as_str(),
                chunk.start_offset as i64,
                chunk.end_offset as i64,
                chat_turn_id,
                chunk_at,
            ],
        )?;
        let chunk_id = tx.last_insert_rowid();
        uid_to_id.insert(&chunk.uid, chunk_id);
        ordinal += 1;

        if chunk.kind == ChunkKind::Leaf {
            let vector = serde_json::to_string(&embeddings[leaf_idx])
                .map_err(|e| Error::Other(format!("encode embedding: {e}")))?;
            tx.execute(
                "INSERT INTO chunk_vec (rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, vector],
            )?;
            tx.execute(
                "INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)",
                params![chunk_id, chunk.embed_content],
            )?;
            leaf_idx += 1;
        }
    }
    Ok(ordinal)
}

/// Insert a document's chunk rows + leaf vectors + FTS rows (the shared body of [`index_document`]
/// and [`replace_chunks`]). `embeddings` are the leaf embeddings in leaf order; the splitter emits
/// parents before their children, so one ordered pass resolves `parent_uid` → row id. Only leaves
/// get a vector + FTS row, on the same rowid as their `chunks.id`, so the rowid-mirrors-chunks.id
/// invariant holds (parents are gaps). An index-only document keeps its leaf embeddings
/// (vector-findable) but never the body bytes: every chunk row stores a placeholder, and keyword
/// search is served by `stored_summary` (indexed once on the first leaf rowid) rather than the body.
fn insert_chunks(
    tx: &Connection,
    doc_id: i64,
    chunks: &[splitter::Chunk],
    embeddings: &[Vec<f32>],
    index_only: bool,
    stored_summary: Option<&str>,
) -> Result<()> {
    let mut uid_to_id: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut leaf_idx = 0usize;
    let mut first_leaf_id: Option<i64> = None;
    for (ordinal, chunk) in chunks.iter().enumerate() {
        let parent_id = chunk
            .parent_uid
            .as_deref()
            .and_then(|uid| uid_to_id.get(uid).copied());
        let content: &str = if index_only {
            INDEX_ONLY_BODY_PLACEHOLDER
        } else {
            &chunk.display_content
        };
        tx.execute(
            "INSERT INTO chunks \
             (document_id, ordinal, heading, content, char_count, uid, parent_id, kind, start_offset, end_offset) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                doc_id,
                ordinal as i64,
                chunk.heading,
                content,
                content.chars().count() as i64,
                chunk.uid,
                parent_id,
                chunk.kind.as_str(),
                chunk.start_offset as i64,
                chunk.end_offset as i64,
            ],
        )?;
        let chunk_id = tx.last_insert_rowid();
        uid_to_id.insert(&chunk.uid, chunk_id);

        if chunk.kind == ChunkKind::Leaf {
            let vector = serde_json::to_string(&embeddings[leaf_idx])
                .map_err(|e| Error::Other(format!("encode embedding: {e}")))?;
            tx.execute(
                "INSERT INTO chunk_vec (rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, vector],
            )?;
            // Vault docs index the heading-prepended text so keyword search benefits from the
            // breadcrumb, while `chunks.content` stays clean for display + citations. Index-only
            // docs index nothing here — keeping the body out of the index — and become
            // keyword-findable by their summary, attached to the first leaf rowid below.
            if !index_only {
                tx.execute(
                    "INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)",
                    params![chunk_id, chunk.embed_content],
                )?;
            }
            first_leaf_id.get_or_insert(chunk_id);
            leaf_idx += 1;
        }
    }

    if index_only {
        if let (Some(rowid), Some(summary)) = (first_leaf_id, stored_summary) {
            if !summary.trim().is_empty() {
                tx.execute(
                    "INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)",
                    params![rowid, summary],
                )?;
            }
        }
    }
    Ok(())
}

/// Replace an existing document's chunks/vectors/FTS rows in place (keeping the `documents` row, so
/// its id + classification + entity link survive), then re-insert from new `chunks`/`embeddings`.
/// The re-embed path for an index-only item whose source content changed — `chunk_vec` and
/// `chunks_fts` are keyed by `chunks.id`, so the stale rows for this doc are cleared explicitly
/// before [`insert_chunks`] repopulates them. Caller owns the transaction.
pub(crate) fn replace_chunks(
    tx: &Connection,
    doc_id: i64,
    chunks: &[splitter::Chunk],
    embeddings: &[Vec<f32>],
    index_only: bool,
    stored_summary: Option<&str>,
) -> Result<()> {
    tx.execute(
        "DELETE FROM chunk_vec WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
        params![doc_id],
    )?;
    tx.execute(
        "DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
        params![doc_id],
    )?;
    tx.execute("DELETE FROM chunks WHERE document_id = ?1", params![doc_id])?;
    insert_chunks(tx, doc_id, chunks, embeddings, index_only, stored_summary)
}

/// Purge one document entirely: its `chunk_vec` + `chunks_fts` mirror rows, its `chunks`, and the
/// `documents` row itself. This is the exact cascade a "delete document" uses (card 7G deletes a chat
/// through it); it also matches the global teardown order at the top of [`rebuild`].
///
/// Order matters. `chunk_vec` (sqlite-vec vec0) and `chunks_fts` (FTS5) are NOT FK targets — they are
/// keyed by `chunks.id` (rowid mirror), so their rows MUST be deleted while the `chunks` rows they key
/// off still exist (exactly as [`replace_chunks`] does). Then `chunks`, then the `documents` row.
/// Deleting `documents` is explicit: `chunks.document_id` cascades in the *delete-documents* direction,
/// but we delete bottom-up, so without this the `documents` row would be left orphaned. Caller owns the
/// transaction.
pub(crate) fn delete_document(tx: &Connection, doc_id: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM chunk_vec WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
        params![doc_id],
    )?;
    tx.execute(
        "DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
        params![doc_id],
    )?;
    tx.execute("DELETE FROM chunks WHERE document_id = ?1", params![doc_id])?;
    tx.execute("DELETE FROM documents WHERE id = ?1", params![doc_id])?;
    Ok(())
}

/// Split a document body into chunks with the active splitter, sizing by tokens through the given
/// counter (the gateway → the selected embedder's tokenizer). The title + content hash feed the
/// heading breadcrumb and the stable, rebuild-reproducible chunk uids.
pub(crate) fn split_document(
    counter: &dyn splitter::TokenCounter,
    body: &str,
    title: &str,
    content_hash: &str,
) -> Result<Vec<splitter::Chunk>> {
    let splitter = splitter::RecursiveSplitter::default();
    let meta = SplitMeta {
        title,
        content_hash,
    };
    splitter.split(body, &meta, counter)
}

/// The text to embed + keyword-index for each leaf chunk (heading-prepended). Parents are
/// structural-only and contribute none, so this lines up 1:1 with the leaf rows.
pub(crate) fn leaf_embed_texts(chunks: &[splitter::Chunk]) -> Vec<String> {
    chunks
        .iter()
        .filter(|c| c.kind == ChunkKind::Leaf)
        .map(|c| c.embed_content.clone())
        .collect()
}

/// The SELECT column list backing `row_to_document` — shared by the list and
/// single-document loads so the two never drift.
const DOCUMENT_COLUMNS: &str = "d.id, d.title, d.source_path, d.ext, d.byte_size, \
     (SELECT count(*) FROM chunks c WHERE c.document_id = d.id), \
     d.created_at, d.ingested_at, d.project, d.tags, d.importance, d.reviewed, d.last_activity, \
     d.source_type, d.source_state, d.external_ref, d.source_id";

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
        source_type: row.get(13)?,
        source_state: row.get(14)?,
        external_ref: row.get(15)?,
        source_id: row.get(16)?,
    })
}

/// Where a document keeps its *organisational truth* — the canonical project + tags / importance it
/// was filed under. A fully-stored vault document keeps it in its Markdown front-matter; an
/// index-only document (board card 3) has no Markdown file, so its truth lives in the encrypted
/// index-only manifest at the data-home root. This seam is the ONE place that dispatch lives, so
/// every metadata-write site (the review commit, the single edit, the entity merge/rename/reassign)
/// routes to the right writer instead of hard-coding front-matter.
enum TruthSource {
    /// The document's truth is its Markdown vault file's front-matter (every fully-stored document).
    VaultFrontmatter,
    /// The document is index-only (no Markdown file); its truth is the encrypted manifest.
    IndexManifest,
}

/// Pick where a document keeps its truth, by its `source_type` discriminator.
fn truth_source(tx: &Connection, doc_id: i64) -> Result<TruthSource> {
    let source_type: String = tx.query_row(
        "SELECT source_type FROM documents WHERE id = ?1",
        params![doc_id],
        |r| r.get(0),
    )?;
    Ok(match source_type.as_str() {
        SOURCE_TYPE_INDEX_ONLY => TruthSource::IndexManifest,
        _ => TruthSource::VaultFrontmatter,
    })
}

/// Persist a document's organisational truth (canonical project + tags/importance/reviewed/
/// last_activity) to wherever its source keeps it, returning the file snapshot for rollback. The
/// single indirection point every metadata write goes through — so hard-coding front-matter can't
/// creep back in: a vault document rewrites its Markdown front-matter, an index-only document
/// rewrites the encrypted manifest (see [`TruthSource`]). `vault`/`cipher` reach the Markdown layer;
/// `vault_root`/`manifest_cipher` reach the manifest — a caller passes both and the dispatch picks.
/// Pass a `rusqlite::Transaction` (it derefs to `&Connection`); commit it only once the whole batch
/// is written. The returned `(path, prior_bytes)` rolls back via [`restore_vault_files`] for either
/// arm (an empty prior means the file was freshly created — it gets removed, not zeroed).
#[allow(clippy::too_many_arguments)]
pub fn write_document_truth(
    tx: &Connection,
    vault: &Path,
    cipher: &MarkdownCipher,
    doc_id: i64,
    project: &str,
    tags: &[String],
    importance: Option<&str>,
    reviewed: bool,
    last_activity: &str,
    vault_root: &Path,
    manifest_cipher: &crate::index_only::ManifestCipher,
) -> Result<(std::path::PathBuf, Vec<u8>)> {
    match truth_source(tx, doc_id)? {
        TruthSource::VaultFrontmatter => rewrite_vault_metadata(
            tx,
            vault,
            cipher,
            doc_id,
            project,
            tags,
            importance,
            reviewed,
            last_activity,
        ),
        TruthSource::IndexManifest => rewrite_manifest_metadata(
            tx,
            vault_root,
            manifest_cipher,
            doc_id,
            project,
            tags,
            importance,
            reviewed,
            last_activity,
        ),
    }
}

/// Reconstruct a photo's satellite record from parsed front-matter fields + body, or `None` if this
/// isn't a photo document. Shared by the rebuild walk and the metadata-edit rewrite so both preserve
/// the photo block identically. `content_hash` is the photo's `file_hash`; the OCR text comes from
/// the body, the rest from the `photo_*` lines.
fn photo_from_fields(
    fields: &std::collections::HashMap<String, String>,
    content_hash: &str,
    body: &str,
) -> Option<PhotoRecord> {
    if fields.get("source_type").map(String::as_str) != Some(SOURCE_TYPE_PHOTO) {
        return None;
    }
    Some(PhotoRecord {
        source_path: fields.get("source_path").cloned(),
        source_type: PhotoSourceType::from_db(
            fields
                .get("photo_source_type")
                .map(String::as_str)
                .unwrap_or("dragged_file"),
        ),
        capture_date: fields.get("created_at").cloned().unwrap_or_default(),
        file_hash: content_hash.to_string(),
        ocr_text: photos::ocr_text_from_body(body),
        saved_to_vault: fields
            .get("photo_saved_to_vault")
            .map(|v| v.trim() == "true")
            .unwrap_or(false),
        vault_path: fields.get("photo_vault_path").cloned(),
        width: fields
            .get("photo_width")
            .and_then(|v| v.trim().parse().ok()),
        height: fields
            .get("photo_height")
            .and_then(|v| v.trim().parse().ok()),
        lat: fields.get("photo_lat").and_then(|v| v.trim().parse().ok()),
        lon: fields.get("photo_lon").and_then(|v| v.trim().parse().ok()),
    })
}

/// Reconstruct a spreadsheet's satellite record from parsed front-matter fields, or `None` if this
/// isn't a spreadsheet document. Shared by the rebuild walk and the metadata-edit rewrite so both
/// preserve the block identically. Missing counts default to 0, so a hand-edited file still rebuilds.
fn spreadsheet_from_fields(
    fields: &std::collections::HashMap<String, String>,
) -> Option<SpreadsheetRecord> {
    if fields.get("source_type").map(String::as_str) != Some(SOURCE_TYPE_SPREADSHEET) {
        return None;
    }
    let count = |k: &str| {
        fields
            .get(k)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    };
    Some(SpreadsheetRecord {
        sheet_count: count("spreadsheet_sheet_count"),
        total_rows: count("spreadsheet_total_rows"),
        chunked_rows: count("spreadsheet_chunked_rows"),
    })
}

/// Rewrite a document's organisation metadata in place, *inside a caller-owned
/// transaction*: update the vault file's front-matter (preserving the body) and
/// the `documents` row. No re-chunk / re-embed — the body and `content_hash` are
/// unchanged, so the existing chunks and vectors stay valid. The Markdown-vault arm of
/// [`write_document_truth`]; external callers go through that seam, never here directly.
///
/// Returns `(vault file, its prior raw on-disk bytes)` so the caller can restore the
/// file (via [`restore_vault_files`]) if a later step in the batch fails; the DB side
/// rolls back with `tx`. The snapshot is the *raw* bytes, not the decoded text — for an
/// encrypted vault the file is ciphertext, so restoring decoded text would corrupt it.
// The arguments are the metadata columns being rewritten, not a sign this should
// be split into smaller functions.
#[allow(clippy::too_many_arguments)]
fn rewrite_vault_metadata(
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

    // Preserve a photo's or spreadsheet's block across an organisation edit, so a later Rebuild still
    // reconstructs its satellite row (a plain document has neither → `None`, unchanged behaviour).
    let content_hash = fields.get("content_hash").map(String::as_str).unwrap_or("");
    let photo = photo_from_fields(&fields, content_hash, body);
    let spreadsheet = spreadsheet_from_fields(&fields);
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
        content_hash,
        created_at: fields.get("created_at").map(String::as_str).unwrap_or(""),
        ingested_at: fields.get("ingested_at").map(String::as_str).unwrap_or(""),
        project,
        tags,
        importance,
        last_activity,
        reviewed,
        photo: photo.as_ref(),
        spreadsheet: spreadsheet.as_ref(),
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

/// Rewrite an index-only document's organisation metadata, *inside a caller-owned transaction*:
/// update the `documents` row (the mirror), then regenerate the encrypted manifest from that
/// uncommitted mirror so the portable truth tracks the edit. The manifest arm of
/// [`write_document_truth`] — the analog of [`rewrite_vault_metadata`] for a document with no
/// Markdown file. No re-chunk / re-embed: the body and pointer are unchanged, so the existing chunks
/// and vectors stay valid. Returns `(manifest file, its prior raw bytes)` for rollback via
/// [`restore_vault_files`]; an empty prior (the manifest's first write in this batch) restores by
/// removing the file. The per-write snapshot stack unwinds a mixed batch — several vault files plus
/// the single manifest — correctly newest-first, because each manifest write reads-current then
/// writes-new.
#[allow(clippy::too_many_arguments)]
fn rewrite_manifest_metadata(
    tx: &Connection,
    vault_root: &Path,
    manifest_cipher: &crate::index_only::ManifestCipher,
    doc_id: i64,
    project: &str,
    tags: &[String],
    importance: Option<&str>,
    reviewed: bool,
    last_activity: &str,
) -> Result<(std::path::PathBuf, Vec<u8>)> {
    let tags_json =
        serde_json::to_string(tags).map_err(|e| Error::Other(format!("encode tags: {e}")))?;
    tx.execute(
        "UPDATE documents SET project = ?1, tags = ?2, importance = ?3, reviewed = ?4, \
         last_activity = ?5 WHERE id = ?6 AND source_type = 'index_only'",
        params![
            project,
            tags_json,
            importance,
            reviewed as i64,
            last_activity,
            doc_id
        ],
    )?;
    let prior = crate::index_only::write_synced(tx, vault_root, manifest_cipher)?;
    Ok((crate::index_only::manifest_path(vault_root), prior))
}

/// Restore truth files overwritten during an abandoned metadata batch to their prior raw bytes — the
/// file half of the rollback (the DB half rolls back by dropping the uncommitted transaction).
/// Writing back the exact on-disk bytes keeps an encrypted file's ciphertext intact. An EMPTY prior
/// means the file did not exist when the snapshot was taken (e.g. the first write to the index-only
/// manifest in this batch), so restoring it means REMOVING the file, not leaving a 0-byte one a
/// reader would choke on — a vault-frontmatter rewrite always has non-empty prior bytes, so this only
/// ever fires for a freshly-created truth file. Best-effort per file; applied newest-first.
pub fn restore_vault_files(written: Vec<(std::path::PathBuf, Vec<u8>)>) {
    for (file, original) in written.into_iter().rev() {
        if original.is_empty() {
            let _ = std::fs::remove_file(&file);
        } else {
            let _ = std::fs::write(&file, original);
        }
    }
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
    /// Present only for a photo: appends `source_type: photo` + the photo-specific fields so a
    /// Rebuild reconstructs the `photos` satellite row from the vault. `None` for a plain document.
    photo: Option<&'a PhotoRecord>,
    /// Present only for a spreadsheet: appends `source_type: spreadsheet` + the satellite counts so a
    /// Rebuild reconstructs the `spreadsheets` row. Mutually exclusive with `photo`; `None` otherwise.
    spreadsheet: Option<&'a SpreadsheetRecord>,
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
         {}{}---\n\n{}\n",
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
        f.photo.map(render_photo_block).unwrap_or_default(),
        f.spreadsheet
            .map(render_spreadsheet_block)
            .unwrap_or_default(),
        body,
    )
}

/// The photo-specific front-matter lines (only present for a photo). `source_type: photo` is the
/// marker `rebuild_one` keys on; `capture_date`/`file_hash` are NOT repeated (they round-trip via
/// `created_at`/`content_hash`) and `ocr_text` lives in the body. Optional fields are emitted only
/// when set. The flat parser reads each line back as a field with zero parser changes.
fn render_photo_block(p: &PhotoRecord) -> String {
    let mut s = format!(
        "source_type: photo\n\
         photo_source_type: {}\n\
         photo_saved_to_vault: {}\n",
        p.source_type.as_str(),
        p.saved_to_vault,
    );
    if let Some(vp) = &p.vault_path {
        s.push_str(&format!("photo_vault_path: {}\n", yaml_quote(vp)));
    }
    if let Some(w) = p.width {
        s.push_str(&format!("photo_width: {w}\n"));
    }
    if let Some(h) = p.height {
        s.push_str(&format!("photo_height: {h}\n"));
    }
    if let (Some(lat), Some(lon)) = (p.lat, p.lon) {
        s.push_str(&format!("photo_lat: {lat:.6}\nphoto_lon: {lon:.6}\n"));
    }
    s
}

/// The spreadsheet-specific front-matter lines (only present for a spreadsheet). `source_type:
/// spreadsheet` is the marker `spreadsheet_from_fields` keys on; the counts round-trip the
/// `spreadsheets` satellite row so a Rebuild reconstructs it without re-parsing the file. The flat
/// parser reads each line back as a field with zero parser changes.
fn render_spreadsheet_block(sp: &SpreadsheetRecord) -> String {
    format!(
        "source_type: spreadsheet\n\
         spreadsheet_sheet_count: {}\n\
         spreadsheet_total_rows: {}\n\
         spreadsheet_chunked_rows: {}\n",
        sp.sheet_count, sp.total_rows, sp.chunked_rows,
    )
}

/// Serialize a list of strings as a YAML flow sequence on one line.
pub(crate) fn render_yaml_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| yaml_quote(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// Parse our own front-matter back out: returns the simple key→value fields and
/// the body. Only the flat scalar fields we wrote are read (enough to rebuild).
/// Shared with the chat-session layer (`chat.rs`), whose vault files use the same
/// flat front-matter so a Rebuild reads them with this one parser.
pub(crate) fn parse_frontmatter(
    raw: &str,
) -> Option<(std::collections::HashMap<String, String>, &str)> {
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

pub(crate) fn yaml_quote(value: &str) -> String {
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

/// `documents.source_type` for a fully-stored vault document (its body lives in a Markdown file).
pub(crate) const SOURCE_TYPE_VAULT: &str = "vault";
/// `documents.source_type` for an index-only document (body lives at the remote source; we keep a
/// pointer + embedding + summary, never the bytes — board card 3 / spec §8.1).
pub(crate) const SOURCE_TYPE_INDEX_ONLY: &str = "index_only";
/// `documents.source_type` for a photo/screenshot (board card #135). Like a vault document its body
/// lives in a Markdown file (the synthetic photo body), so it rebuilds from disk; the `photos`
/// satellite row carries its image-specific truth.
pub(crate) const SOURCE_TYPE_PHOTO: &str = "photo";
/// `documents.source_type` for an indexed chat session (board card 7B, #141). Like a vault document
/// its body lives in a Markdown file (the appended turn-pairs written by card A), so it rebuilds from
/// disk and the deletion cascade covers it; the `chat_sessions` satellite carries its chat-specific
/// truth (scope + the index/summary cursors).
pub(crate) const SOURCE_TYPE_CHAT: &str = "chat";
/// `documents.source_type` for a spreadsheet (board card: Spreadsheet Processing). Like a vault
/// document its body lives in a Markdown file (the synthetic sheet body), so it rebuilds from disk and
/// the deletion cascade covers it; the `spreadsheets` satellite row carries its sheet/row counts (and a
/// reserved `structured_data_summary`).
pub(crate) const SOURCE_TYPE_SPREADSHEET: &str = "spreadsheet";
/// `documents.source_state` for a reachable source (the default for every document).
pub(crate) const SOURCE_STATE_OK: &str = "ok";
/// `documents.source_state` for an index-only item whose source was deleted: a soft state — the
/// metadata + embedding stay (so it's still findable) but the body is flagged unretrievable. Never a
/// hard drop.
pub(crate) const SOURCE_STATE_MISSING: &str = "source_missing";
/// `documents.source_state` for an index-only item whose whole source can't be reached (expired
/// OAuth, an unmounted drive). First-class, never masquerading as mass deletion.
pub(crate) const SOURCE_STATE_UNREACHABLE: &str = "unreachable";
/// Stand-in stored for an index-only chunk's body text: we hold the embedding (vector-findable) but
/// never persist the bytes, so the readable body is a live fetch. The short `stored_summary` on the
/// document is what stays legible offline.
pub(crate) const INDEX_ONLY_BODY_PLACEHOLDER: &str = "(body available at the source)";

/// The source discriminator + live pointer for a document. `Default` describes a fully-stored vault
/// document; pointer-ingest overrides these for an index-only item whose body stays at the source.
#[derive(Clone)]
pub(crate) struct SourceMeta {
    pub source_type: String,
    pub source_state: String,
    pub source_id: Option<String>,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    pub stored_summary: Option<String>,
    /// The source folder this item was found in (Drive today) — sorting-review context only, never
    /// chunked or embedded. `None` for vault imports and any source without a folder concept.
    pub source_parent_folder_id: Option<String>,
    pub source_parent_folder_name: Option<String>,
}

impl Default for SourceMeta {
    fn default() -> Self {
        Self {
            source_type: SOURCE_TYPE_VAULT.into(),
            source_state: SOURCE_STATE_OK.into(),
            source_id: None,
            external_ref: None,
            source_modified_at: None,
            source_content_hash: None,
            stored_summary: None,
            source_parent_folder_id: None,
            source_parent_folder_name: None,
        }
    }
}

impl SourceMeta {
    fn is_index_only(&self) -> bool {
        self.source_type == SOURCE_TYPE_INDEX_ONLY
    }

    /// Source metadata for a photo: a fully-stored, reachable document discriminated as `'photo'`.
    fn photo() -> Self {
        Self {
            source_type: SOURCE_TYPE_PHOTO.into(),
            ..Self::default()
        }
    }

    /// Source metadata for a spreadsheet: a fully-stored, reachable document discriminated as
    /// `'spreadsheet'` (its synthetic sheet body lives in a Markdown vault file).
    fn spreadsheet() -> Self {
        Self {
            source_type: SOURCE_TYPE_SPREADSHEET.into(),
            ..Self::default()
        }
    }

    /// Source metadata for an indexed chat: a fully-stored, reachable document discriminated as
    /// `'chat'`, carrying the stable chat identity (`chat:<conversation_id>`) so an append-growing
    /// session keeps one UNIQUE `source_id`/`content_hash` across every re-index (card B).
    pub(crate) fn chat(source_id: String) -> Self {
        Self {
            source_type: SOURCE_TYPE_CHAT.into(),
            source_id: Some(source_id),
            ..Self::default()
        }
    }
}

pub(crate) struct DocMeta {
    pub source_path: Option<String>,
    pub vault_path: String,
    pub title: String,
    pub content_hash: String,
    pub ext: Option<String>,
    pub byte_size: Option<i64>,
    pub created_at: Option<String>,
    pub ingested_at: String,
    pub project: String,
    pub tags: Vec<String>,
    pub importance: Option<String>,
    pub reviewed: bool,
    pub last_activity: Option<String>,
    /// Source discriminator + pointer. Vault ingest leaves this at `SourceMeta::default()`.
    pub source: SourceMeta,
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
        // Already in the exact target form? Nothing to do (idempotent). "Exact" means the
        // same name, the same encryption state, AND the same key — a passphrase change
        // keeps the name but moves the subkey, so those files must still be re-encoded.
        if new_name == old_name
            && crate::vault::crypto::is_encrypted(&raw) == write_with.encryption_on()
            && read_with.same_key_as(write_with)
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

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Refuse an embedder whose vector width the vault's **live** `chunk_vec` can't hold. Used by
/// incremental ingest (`run`) — it can't resize the table, so a populated vault whose selected
/// language no longer matches its index (switched but not yet re-indexed) is sent to the Re-index
/// flow with a clear message, rather than hitting a cryptic vec0 insert failure. `rebuild` does not
/// call this: it resizes the table to fit ([`crate::db::ensure_vec_dim`]) after warming the model.
pub(crate) fn guard_dimension(conn: &rusqlite::Connection, embedder: &ModelEntry) -> Result<()> {
    let vec_dim = crate::db::vec0_dim(conn)?;
    if embedder.dimension != vec_dim {
        return Err(Error::Other(format!(
            "the selected search language ('{}', {}-dimensional) doesn't match this vault's index \
             ({}-dimensional) — re-index the vault from Settings → Search to switch it",
            embedder.id, embedder.dimension, vec_dim
        )));
    }
    Ok(())
}

/// Whether a warmup embed produced the width we expect. Pure, so the model-free decision is
/// unit-tested here; the real download+embed (the first live e5-large 1024-d exercise) stays a
/// documented hardware verification, not a CI test. A `None` (the model returned no vector at all)
/// is a failure — the warmup proved nothing.
fn warmup_ok(got_width: Option<usize>, expected: usize) -> bool {
    got_width == Some(expected)
}

/// Guard the index against a model mismatch: one vector per leaf chunk, each of the selected
/// embedder's dimension (passed in, never a hardcoded 384).
pub(crate) fn check_embeddings(
    embeddings: &[Vec<f32>],
    leaves: usize,
    expected_dim: usize,
) -> Result<()> {
    if embeddings.len() != leaves {
        return Err(Error::Other(
            "embedding count did not match leaf-chunk count".into(),
        ));
    }
    if embeddings.iter().any(|v| v.len() != expected_dim) {
        return Err(Error::Other(format!(
            "embedding dimension mismatch (expected {expected_dim}); wrong model?"
        )));
    }
    Ok(())
}

pub(crate) fn iso_now(conn: &Connection) -> Result<String> {
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
    fn warmup_ok_requires_the_exact_expected_width() {
        // The rebuild warmup only proceeds to wipe the index when the model emits the exact width
        // we expect — this guards against an offline/absent model (None) and a wrong-dimension
        // export (e.g. a 384-d export sneaking into the 1024-d slot).
        assert!(warmup_ok(Some(1024), 1024));
        assert!(!warmup_ok(Some(384), 1024));
        assert!(!warmup_ok(None, 1024));
    }

    const TEST_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// Hand-insert a vault row and an index-only row (no embedder needed) and return their ids.
    fn store_with_one_of_each() -> (tempfile::TempDir, Connection, i64, i64) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('v.md','V','hv')",
            [],
        )
        .unwrap();
        let vault_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id, \
                    project, stored_summary) \
             VALUES ('idx://s1','Pointer','hp','index_only','s1','Unsorted','a short summary')",
            [],
        )
        .unwrap();
        let index_id = conn.last_insert_rowid();
        (dir, conn, vault_id, index_id)
    }

    #[test]
    fn truth_source_discriminates_on_source_type() {
        let (_d, conn, vault_id, index_id) = store_with_one_of_each();
        assert!(matches!(
            truth_source(&conn, vault_id).unwrap(),
            TruthSource::VaultFrontmatter
        ));
        assert!(matches!(
            truth_source(&conn, index_id).unwrap(),
            TruthSource::IndexManifest
        ));
    }

    #[test]
    fn delete_document_purges_all_four_tables() {
        // The cascade card 7G's chat-delete rides on: one document's rows must vanish from
        // `documents`, `chunks`, and the two rowid-keyed mirrors (`chunk_vec`, `chunks_fts`), while a
        // second document is left completely untouched.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        // Seed a document with one leaf chunk mirrored into chunk_vec (a 384-d zero vector, encoded as
        // vec0 expects — a JSON array string) and chunks_fts.
        let seed = |vp: &str, hash: &str| -> i64 {
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash) VALUES (?1,'T',?2)",
                params![vp, hash],
            )
            .unwrap();
            let doc = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count) VALUES (?1, 0, 'body', 4)",
                params![doc],
            )
            .unwrap();
            let chunk = conn.last_insert_rowid();
            let vector = serde_json::to_string(&vec![0f32; 384]).unwrap();
            conn.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk, vector],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks_fts(rowid, content) VALUES (?1, 'body')",
                params![chunk],
            )
            .unwrap();
            doc
        };
        let a = seed("a.md", "ha");
        let b = seed("b.md", "hb");

        let tx = conn.unchecked_transaction().unwrap();
        delete_document(&tx, a).unwrap();
        tx.commit().unwrap();

        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        // Document a is gone from documents + chunks...
        assert_eq!(
            count(&format!("SELECT count(*) FROM documents WHERE id={a}")),
            0
        );
        assert_eq!(
            count(&format!(
                "SELECT count(*) FROM chunks WHERE document_id={a}"
            )),
            0
        );
        // ...and — the whole point — its mirror rows are gone too. The mirrors are keyed by chunks.id
        // with no FK, so the only surviving rows must be document b's single chunk.
        assert_eq!(
            count("SELECT count(*) FROM chunk_vec"),
            1,
            "only b's vector"
        );
        assert_eq!(count("SELECT count(*) FROM chunks_fts"), 1, "only b's fts");
        // Document b is completely untouched.
        assert_eq!(
            count(&format!("SELECT count(*) FROM documents WHERE id={b}")),
            1
        );
        assert_eq!(
            count(&format!(
                "SELECT count(*) FROM chunks WHERE document_id={b}"
            )),
            1
        );
    }

    #[test]
    fn is_vault_markdown_matches_chat_filenames() {
        // Card 7G invariant: a Rebuild's flat-vault sweep must keep collecting chat files so their
        // chunks are re-embedded on a tier switch. Chats are named `chat-<date>-<hash>.md[.pmenc]` and
        // live in the same flat `vault/` dir as documents — guard the extension predicate against anyone
        // narrowing it (e.g. to only `.md`, or to a `documents/` prefix).
        assert!(is_vault_markdown(Path::new(
            "chat-28-06-2026-abc123def456.md"
        )));
        assert!(is_vault_markdown(Path::new(
            "chat-28-06-2026-abc123def456.md.pmenc"
        )));
        // A plain document file still matches (shared predicate), a stray non-markdown file does not.
        assert!(is_vault_markdown(Path::new("report-01-07-2026-ff00.md")));
        assert!(!is_vault_markdown(Path::new("chat-28-06-2026-abc.json")));
    }

    #[test]
    fn rebuild_sweep_collects_a_chat_vault_file() {
        // Guards the sweep ROOT + predicate together: the exact `read_dir(&vault).filter(is_vault_markdown)`
        // expression rebuild uses must pick up chat files sitting in the flat vault dir, plaintext and
        // encrypted alike. If someone re-scopes rebuild to a documents subdir, this fails.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join("report-01-07-2026-ff00.md"), "doc").unwrap();
        std::fs::write(vault.join("chat-28-06-2026-abc123def456.md"), "chat").unwrap();
        std::fs::write(vault.join("chat-28-06-2026-def456abc123.md.pmenc"), b"enc").unwrap();
        std::fs::write(vault.join("scratch.tmp"), "ignore").unwrap();

        let collected: Vec<String> = std::fs::read_dir(&vault)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| is_vault_markdown(path))
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(collected
            .iter()
            .any(|n| n.starts_with("chat-") && n.ends_with(".md")));
        assert!(collected.iter().any(|n| n.ends_with(".md.pmenc")));
        assert!(!collected.iter().any(|n| n.ends_with(".tmp")));
        assert_eq!(collected.len(), 3, "two chats + one document, not the .tmp");
    }

    #[test]
    fn is_chat_vault_file_routes_only_chat_frontmatter() {
        // Card 7G / H2 fix: a Rebuild must route a chat `.md` through the chat engine (which preserves its
        // identity — source_type='chat', per-chunk turn pointer, and the session→document link), not the
        // generic document path. This predicate is the router; guard it so a chat file is never mistaken for
        // a plain document (which would drop the chat identity and orphan the session link), and vice-versa.
        let dir = tempfile::tempdir().unwrap();
        let cipher = MarkdownCipher::plaintext("vault-test");
        let chat = dir.path().join("chat-28-06-2026-abc.md");
        let doc = dir.path().join("report-01-07-2026-ff00.md");
        std::fs::write(
            &chat,
            "---\nsource_type: chat\nchat_conversation_id: 7\ntitle: Hi\n---\n\nhello",
        )
        .unwrap();
        std::fs::write(&doc, "---\ntitle: Report\n---\n\nbody").unwrap();
        assert!(is_chat_vault_file(&cipher, &chat));
        assert!(!is_chat_vault_file(&cipher, &doc));
        // A missing / headerless file is treated as non-chat: it falls to rebuild_one, which surfaces the
        // real read/parse error instead of silently taking the chat path.
        assert!(!is_chat_vault_file(&cipher, &dir.path().join("nope.md")));
    }

    #[test]
    fn write_document_truth_routes_an_index_only_doc_to_the_manifest() {
        use crate::index_only::{manifest_path, read_manifest, ManifestCipher};
        let (dir, mut conn, _vault_id, index_id) = store_with_one_of_each();
        let cipher = ManifestCipher::from_master("vault-test", &[9u8; 32]);
        let vault_dir = dir.path().join("vault"); // never written for an index-only doc

        let tx = conn.transaction().unwrap();
        let (path, prior) = write_document_truth(
            &tx,
            &vault_dir,
            // The Markdown cipher is unused on the manifest arm; a plaintext one satisfies the
            // signature without touching the vault.
            &crate::vault::MarkdownCipher::plaintext("vault-test"),
            index_id,
            "Project X",
            &["urgent".to_string()],
            Some("high"),
            true,
            "2026-06-26T00:00:00Z",
            dir.path(),
            &cipher,
        )
        .unwrap();
        tx.commit().unwrap();

        // It wrote the manifest (first write → empty prior), not a vault file.
        assert_eq!(path, manifest_path(dir.path()));
        assert!(prior.is_empty());
        assert!(
            !vault_dir.exists(),
            "no Markdown vault file for an index-only doc"
        );

        // The manifest carries the new classification, and the documents row (mirror) matches.
        let manifest = read_manifest(dir.path(), &cipher).unwrap().unwrap();
        let item = manifest
            .items
            .iter()
            .find(|i| i.source_id == "s1")
            .expect("the index-only item is in the manifest");
        assert_eq!(item.project, "Project X");
        assert_eq!(item.tags, vec!["urgent".to_string()]);
        assert_eq!(item.importance.as_deref(), Some("high"));
        assert!(item.reviewed);

        let row_project: String = conn
            .query_row(
                "SELECT project FROM documents WHERE id = ?1",
                params![index_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_project, "Project X");
    }

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
            photo: None,
            spreadsheet: None,
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
    fn photo_frontmatter_round_trips_for_rebuild() {
        // The rebuild-determinism guarantee for photos: a photo's synthetic body + front-matter block
        // must reconstruct the SAME `photos` record on a Rebuild (and survive a metadata edit, which
        // re-renders through the same path) — without re-running OCR. No sidecar needed.
        let rec = PhotoRecord {
            source_path: Some("/imgs/Screenshot 2026-03-12.png".into()),
            source_type: PhotoSourceType::Screenshot,
            capture_date: "2026-03-12".into(),
            file_hash: "deadbeefcafe".into(),
            ocr_text: Some("Total due £42.00".into()),
            saved_to_vault: true,
            vault_path: Some("photos/deadbeefcafe.png".into()),
            width: Some(1170),
            height: Some(2532),
            lat: Some(55.95),
            lon: Some(-3.19),
        };
        let body = photos::photo_markdown(
            rec.source_type,
            &rec.capture_date,
            rec.lat,
            rec.lon,
            rec.ocr_text.as_deref().unwrap_or(""),
        );
        let title = photos::photo_title(rec.source_type, &rec.capture_date);
        let front = Frontmatter {
            title: &title,
            source_path: rec.source_path.as_deref().unwrap(),
            ext: Some("png"),
            content_hash: &rec.file_hash, // file_hash round-trips via content_hash
            created_at: &rec.capture_date, // capture_date round-trips via created_at
            ingested_at: "2026-06-28T00:00:00.000Z",
            project: "Unsorted",
            tags: &[],
            importance: None,
            last_activity: "2026-06-28T00:00:00.000Z",
            reviewed: false,
            photo: Some(&rec),
            spreadsheet: None,
        };
        let rendered = render_markdown(&front, &body);
        let (fields, parsed_body) = parse_frontmatter(&rendered).unwrap();

        // The marker drives rebuild's source_type='photo' branch.
        assert_eq!(fields.get("source_type").map(String::as_str), Some("photo"));
        let recovered = photo_from_fields(&fields, &rec.file_hash, parsed_body)
            .expect("a photo block reconstructs a record");
        assert_eq!(recovered, rec, "the photos record round-trips exactly");

        // A plain document carries no block → no record (unchanged behaviour).
        let plain = render_markdown(
            &Frontmatter {
                title: "Doc",
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
                photo: None,
                spreadsheet: None,
            },
            "just text",
        );
        let (pf, pb) = parse_frontmatter(&plain).unwrap();
        assert!(photo_from_fields(&pf, "h", pb).is_none());
    }

    #[test]
    fn spreadsheet_frontmatter_round_trips_for_rebuild() {
        // The rebuild-determinism guarantee for spreadsheets: the front-matter block must reconstruct
        // the SAME `spreadsheets` satellite record on a Rebuild — without re-parsing the original file
        // (the synthetic sheet body is already in the vault). The truncation record (chunked < total)
        // must survive too.
        let rec = SpreadsheetRecord {
            sheet_count: 3,
            total_rows: 5000,
            chunked_rows: 4200,
        };
        let front = Frontmatter {
            title: "budget",
            source_path: "/x/budget.xlsx",
            ext: Some("xlsx"),
            content_hash: "abc123",
            created_at: "2026-07-01",
            ingested_at: "2026-07-01T00:00:00.000Z",
            project: "Unsorted",
            tags: &[],
            importance: None,
            last_activity: "2026-07-01T00:00:00.000Z",
            reviewed: false,
            photo: None,
            spreadsheet: Some(&rec),
        };
        let rendered = render_markdown(&front, "## Sheet: Budget\n\n### Overview\n\ntext\n");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();

        // The marker drives rebuild's source_type='spreadsheet' branch.
        assert_eq!(
            fields.get("source_type").map(String::as_str),
            Some("spreadsheet")
        );
        assert_eq!(
            spreadsheet_from_fields(&fields),
            Some(rec),
            "the spreadsheets record round-trips exactly"
        );

        // A plain document carries no block → no record (unchanged behaviour).
        let plain = render_markdown(
            &Frontmatter {
                title: "Doc",
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
                photo: None,
                spreadsheet: None,
            },
            "just text",
        );
        let (pf, _) = parse_frontmatter(&plain).unwrap();
        assert!(spreadsheet_from_fields(&pf).is_none());
    }

    #[test]
    fn photo_dedupe_save_flips_saved_to_vault_by_file_hash() {
        // The dedupe-hit path of `ingest_photo`: when the user re-drops an already-ingested image
        // with "save a copy" newly checked, we skip re-indexing but still flip the existing photos
        // row's saved_to_vault flag and record the vault path, keyed by file_hash. This guards that
        // exact UPDATE against the real migrated schema (a column rename would break it silently).
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type) \
             VALUES ('photos/h.md','Screenshot','imghash','photo')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO photos(document_id, source_type, file_hash, saved_to_vault) \
             VALUES (?1,'screenshot','imghash',0)",
            params![doc_id],
        )
        .unwrap();

        let n = conn
            .execute(
                "UPDATE photos SET saved_to_vault = 1, vault_path = ?1 WHERE file_hash = ?2",
                params!["photos/imghash.png", "imghash"],
            )
            .unwrap();
        assert_eq!(n, 1, "exactly the matching photo row is updated");

        let (saved, path): (i64, Option<String>) = conn
            .query_row(
                "SELECT saved_to_vault, vault_path FROM photos WHERE file_hash = 'imghash'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(saved, 1, "the opt-in flag is now set");
        assert_eq!(path.as_deref(), Some("photos/imghash.png"));
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
            photo: None,
            spreadsheet: None,
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
            photo: None,
            spreadsheet: None,
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

    // Chunking behaviour (token-sizing, structure-awareness, parent tiers, determinism) is
    // now covered by `crate::splitter`'s own unit tests.

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
            photo: None,
            spreadsheet: None,
        };
        let rendered = render_markdown(&front, "body");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();
        // The forged lines stayed inside the title value; they didn't take effect.
        assert_eq!(fields.get("project").map(String::as_str), Some("Unsorted"));
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("false"));
    }
}
