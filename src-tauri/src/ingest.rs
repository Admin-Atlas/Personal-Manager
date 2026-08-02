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

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

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
/// on disk, just not ingested). Lower-case, no dot. Spreadsheet types (`xlsx`/`csv`) are
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
///
/// `pub(crate)` so the READ side shares this one list: `commands::photo_original` bounds
/// `read_document_image`'s original-file fallback to the same extensions that could have created
/// the `photos` row in the first place. One list, not two — an extension added here must widen both
/// sides together or the reader would refuse a type ingest happily accepts.
pub(crate) const PHOTO_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "heic"];

/// Spreadsheet extensions routed to the dedicated spreadsheet processor ([`ingest_spreadsheet`])
/// instead of MarkItDown — the sidecar parses them values-only into a metadata chunk + self-describing
/// row chunks (see [`crate::spreadsheets`]). Like `PHOTO_EXTS`, these are NOT in `SUPPORTED`, so a
/// spreadsheet only ingests via this branch and can never fall back to a MarkItDown pipe-table dump.
/// Legacy `.xls` was dropped with the xlrd parser surface (H-1 subset) — only modern `.xlsx` and `.csv`.
const SPREADSHEET_EXTS: &[&str] = &["xlsx", "csv"];

/// Saved photo originals: opt-in, content-addressed, written with the same cipher as the Markdown
/// around them. **Byte** files, not Markdown — and because encryption suffixes them (`h.png.pmenc`),
/// [`is_vault_markdown`] says yes to every one of them. That is precisely why [`MARKDOWN_SUBDIRS`]
/// is an allow-list rather than a blind recursion: a walk that descended everywhere would hand a
/// JPEG to the document pipeline. The photo half of a key migration ([`convert_photo_originals`])
/// and of the plaintext export stay separate, explicit steps for the same reason.
const PHOTOS_SUBDIR: &str = "photos";

/// Where every chat transcript lives (#281). Chats outnumber documents in an active store and read
/// as noise beside them, so they get one folder of their own — flat, with no project in the path.
///
/// **No project in the path is the load-bearing part.** A document belongs to many projects (#577),
/// so "which folder" would have no answer; and filing by project would mean renaming files inside
/// `merge_projects` / `delete_project` / a project rename — file moves in the riskiest phase of an
/// entity transaction, to buy nothing the DB does not already answer. Membership lives in the DB;
/// the vault just keeps chats together.
pub(crate) const CHATS_SUBDIR: &str = "chats";

/// The subfolders of the Markdown vault that hold **indexable Markdown**, and the reason every walk
/// in this module can be recursive without becoming dangerous. An allow-list, deliberately: see
/// [`PHOTOS_SUBDIR`] for what a blind recursion would swallow. Adding a folder here opts it into the
/// rebuild sweep, the key migration and the plaintext export in one move — which is the point, since
/// those three are exactly the walks that silently strand files when they disagree.
const MARKDOWN_SUBDIRS: &[&str] = &[CHATS_SUBDIR];

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
    /// The OTHER projects this document belongs to (#275) — never including `project`, which is the
    /// home. Filled by a single join pass in the list readers rather than by a column, so
    /// `row_to_document`'s positional `row.get(n)` indices stay exactly where they are.
    pub linked_projects: Vec<String>,
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
    /// The immediate parent folder of a connector-synced item: `_id` is the stable, connector-unique
    /// key (a Drive/OneDrive folder id, or a local full path) and `_name` the leaf name for display.
    /// Both `None` for a vault / chat / photo document. Powers the Review "apply this filing to the
    /// rest of the folder" action, which groups by (source_type, source_parent_folder_id).
    pub source_parent_folder_id: Option<String>,
    pub source_parent_folder_name: Option<String>,
    /// What the SOURCE says about the document, as opposed to what PM measured at ingest (#701).
    /// `None` everywhere means the provider did not say — rendered as "Unknown", never blank and
    /// never attributed to the user. Only the two cloud connectors and the local folder can fill any
    /// of these; a vault document, chat, photo or spreadsheet has no provider to ask.
    pub source_author: Option<String>,
    pub source_last_modified_by: Option<String>,
    /// The source's own creation time (ISO-8601), distinct from `created_at`, which is PM's.
    pub source_created_at: Option<String>,
    /// The source file's size in bytes, distinct from `byte_size`, which measures the file PM
    /// ingested — an index-only pointer has no such file. `None` for a Google-native Doc/Sheet/Slide,
    /// which has no byte size at all.
    pub source_size_bytes: Option<i64>,
    /// When the SOURCE last changed — Drive/OneDrive's own modified time, or the file's mtime for a
    /// local folder. Distinct from every other timestamp here: `created_at` and `ingested_at` are
    /// PM's, and `source_created_at` is when the thing was made. Stored since v11 and written on
    /// every re-sync; it simply had no way of reaching the UI until #707.
    pub source_modified_at: Option<String>,
    /// When PM last had something new to write down about this document (v53) — NOT when PM last
    /// looked. A file nobody has touched keeps an old stamp, which is the honest answer and is what
    /// tells "unedited since March" apart from "this connector stopped working in March". The
    /// projection falls back to `ingested_at`, since first sight is a refresh too.
    pub pm_refreshed_at: Option<String>,
}

/// The global event a rebuild's progress is broadcast on, alongside the caller's `Channel`.
/// A `Channel` is minted by whoever invokes the command, so only that caller can hear it — and
/// the caller is a component that unmounts. This event reaches whatever view is mounted now.
pub const REBUILD_EVENT: &str = "ingest://progress";

/// Broadcast once per document that has just come into existence, carrying the committed
/// [`Document`] itself so a listening view can insert the row rather than re-query for it.
///
/// Distinct from [`REBUILD_EVENT`], which reports *progress* through a job. This reports an
/// *arrival*, and only for rows that are genuinely new and unreviewed — the Documents list and the
/// Review queue can therefore fill in live during a sync instead of waiting for it to finish.
///
/// A rebuild deliberately does NOT emit this: its rows are not new, and a large one would fire tens
/// of thousands of events for documents already on screen.
pub const DOCUMENT_LANDED: &str = "documents://landed";

/// Where a rebuild's progress goes: the global [`REBUILD_EVENT`] plus the `AppState::ingest_job`
/// snapshot, together.
///
/// The pair is what makes a rebuild watchable after a tab switch. The event carries live progress to
/// whichever view is mounted *now*; the snapshot answers "what's happening?" for a view that mounts
/// later and missed the events entirely. This mirrors `cloud_sync::emit_progress`, which solved
/// exactly this for the connectors.
///
/// Deliberately **not** a per-call `Channel`: a channel is minted by whoever invokes the command, so
/// only that caller hears it — and that caller is a component that unmounts. Emitting globally also
/// means the starting view must NOT keep a channel as well, or it would count every file twice.
/// A plain drag-and-drop ingest still uses a channel (`ingest::run`); only rebuild moved.
///
/// `Clone` because the rebuild's blocking phase runs under `spawn_blocking` ('static), so it takes
/// its own clone; every clone addresses the same snapshot and event, so progress stays continuous
/// across the phase boundary.
#[derive(Clone)]
pub struct ProgressSink {
    app: AppHandle,
}

impl ProgressSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    /// Send one event. Best-effort by design: a failed emit must never abort the run — the whole
    /// point is that the work outlives its audience.
    pub fn send(&self, ev: IngestEvent) {
        self.mirror(&ev);
        let _ = self.app.emit(REBUILD_EVENT, ev);
    }

    /// Fold an event into the shared snapshot. Best-effort: a poisoned lock is skipped rather than
    /// failing the run. Binding the guard to a named local first sidesteps the `if let`
    /// temporary-lifetime pitfall (as `cloud_sync::with_cloud_snap` does).
    fn mirror(&self, ev: &IngestEvent) {
        let state = self.app.state::<AppState>();
        let guard = state.ingest_job.lock();
        let Ok(mut snap) = guard else { return };
        apply_event(&mut snap, ev);
    }
}

/// Fold one progress event into the rebuild snapshot. Pure so the counting rules — the thing that
/// decides what a returning tab actually shows — are unit-testable without an app handle.
pub(crate) fn apply_event(snap: &mut crate::IngestJobState, ev: &IngestEvent) {
    match ev {
        IngestEvent::Preparing { message } => snap.prep = Some(message.clone()),
        IngestEvent::Counted { total } => {
            // Setup is over once we have a count; drop the indeterminate label.
            snap.prep = None;
            snap.total = Some(*total);
            snap.processed = 0;
        }
        // `processed` counts *completed* files, so it advances on the terminal events, not on
        // `Started` — matching how the views count. Counting `Started` too would double-count.
        IngestEvent::Done { .. } | IngestEvent::Failed { .. } | IngestEvent::Skipped { .. } => {
            snap.processed += 1;
            finish_recent(snap, ev);
        }
        IngestEvent::Finished {
            ingested,
            skipped,
            failed,
            unreadable,
        } => {
            snap.last_report = Some(crate::IngestReport {
                ingested: *ingested,
                skipped: *skipped,
                failed: *failed,
                unreadable: *unreadable,
            });
        }
        IngestEvent::Started { name, .. } => push_recent(
            snap,
            crate::IngestItem {
                name: name.clone(),
                status: "working".into(),
                detail: None,
            },
        ),
    }
}

/// Append a row, dropping the oldest once the cap is reached.
///
/// `pop_front` rather than `remove(0)`: with the cap at 2,000 an O(n) shift on every file after the
/// buffer fills is real work on a large rebuild, and a deque makes eviction O(1). `VecDeque`
/// serialises to a JSON array exactly as `Vec` does, so nothing on the wire or in the frontend
/// changes.
fn push_recent(snap: &mut crate::IngestJobState, item: crate::IngestItem) {
    if snap.recent.len() >= crate::RECENT_ITEMS_CAP {
        snap.recent.pop_front();
        snap.recent_truncated = true;
    }
    snap.recent.push_back(item);
}

/// The Activity detail line for a file that landed: its chunk count, plus anything that went wrong
/// on the way. Pure, and deliberately shared with the frontend's own fold (`DocumentsView`) by
/// shape — a restored snapshot row and a live one must read identically.
pub(crate) fn done_detail(chunks: i64, warning: Option<&str>) -> String {
    let base = format!("{chunks} chunk{}", if chunks == 1 { "" } else { "s" });
    match warning {
        Some(w) => format!("{base} — {w}"),
        None => base,
    }
}

/// Resolve the open `working` row with a terminal event's outcome.
///
/// Amending the last working row (rather than appending) is what keeps one row per file. Doing this
/// in the BACKEND also fixes a bug the frontend could not: a view that mounted mid-file received a
/// terminal event whose `Started` it never heard, so it had no name to amend and pushed a nameless
/// "failed — …" row. Here the preceding `Started` is always in hand.
fn finish_recent(snap: &mut crate::IngestJobState, ev: &IngestEvent) {
    // Mirrors the view's own folding exactly, so a row restored from the snapshot is indistinguishable
    // from one the view built live: `done` adopts the indexed title + chunk count, the other two keep
    // the name from `Started` and carry the reason/error.
    let (status, name, detail) = match ev {
        IngestEvent::Done { document, warning } => (
            "done",
            Some(document.title.clone()),
            Some(done_detail(document.chunk_count, warning.as_deref())),
        ),
        IngestEvent::Skipped { reason, .. } => ("skipped", None, Some(reason.clone())),
        IngestEvent::Failed { error, .. } => ("failed", None, Some(error.clone())),
        _ => return,
    };
    if let Some(row) = snap.recent.iter_mut().rev().find(|r| r.status == "working") {
        row.status = status.into();
        if let Some(n) = name {
            row.name = n;
        }
        row.detail = detail;
    }
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
        /// Set when the file landed but something about it is not what the user would assume —
        /// today only a photo whose OCR was requested and did not run. Shown in place of the chunk
        /// count on that file's Activity row.
        warning: Option<String>,
    },
    Failed {
        path: String,
        error: String,
    },
    Finished {
        ingested: usize,
        skipped: usize,
        failed: usize,
        /// Entries the enumeration could not read — a folder that would not open, a directory entry
        /// that errored, a path whose stat was refused. Separate from `failed`, which counts files
        /// PM saw and could not index: these were never seen at all, so they are in none of the other
        /// three counters and are absent from `Counted { total }` as well. Without it the summary
        /// sentence claims a clean run over a walk that quietly lost most of the drop.
        ///
        /// Counted in *entries*, not files: one unopenable directory is 1, however much sits inside.
        unreadable: usize,
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
    let (files, unreadable) = collect_files(&inputs);
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
            Ok(Outcome::Indexed { document, warning }) => {
                ingested += 1;
                // The same arrival the connectors announce, for the drag-and-drop path. The channel
                // event below feeds the importing view's own Activity list and reaches only the
                // caller; this is global, so the Documents list and the Review queue fill in live
                // whichever screen the user is actually on.
                if !document.reviewed {
                    let _ = app.emit(DOCUMENT_LANDED, &document);
                }
                let _ = on_event.send(IngestEvent::Done { document, warning });
                // Gentle mode: breathe between files so indexing doesn't pin the CPU continuously.
                // Re-read each file (cheap) so flipping Fast/Gentle mid-import takes effect at once.
                // Best-effort: a transient lock failure here must not abort the whole import (and skip
                // the terminal Finished event) — just skip the pause for this file.
                let pause_ms = state
                    .conn()
                    .map(|conn| crate::db::indexing_pause_ms(&conn))
                    .unwrap_or(0);
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
        unreadable,
    });
    Ok(())
}

// A short-lived per-document result; not copied in bulk, so the size gap between the `Indexed`
// arm and `Skipped(String)` is not worth boxing.
#[allow(clippy::large_enum_variant)]
enum Outcome {
    /// Indexed, with an optional note about something that went wrong WITHOUT stopping the file
    /// landing — "OCR was asked for and did not run", and "the vault copy of the original could not
    /// be saved" (both [`ingest_photo`]; [`join_warnings`] folds them, since either can fire alone).
    /// It rides the terminal event rather than a log line because the document itself looks perfectly
    /// normal afterwards; nothing else would ever tell the user.
    Indexed {
        document: Document,
        warning: Option<String>,
    },
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
    // What the DOCUMENT states about itself — after the conversion, so a file that won't convert
    // never pays for it, and skipped entirely for the formats that state nothing (#709).
    let props = state.sidecar.file_properties(path);
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
        linked_projects: &[],
        tags: &[],
        importance: None,
        last_activity: &ingested_at,
        reviewed: false,
        photo: None,
        spreadsheet: None,
        chat: None,
        source_id: None,
        external_ref: None,
    };
    let vault_file = vault.join(&vault_name);
    cipher.write_to(&vault_file, &render_markdown(&front, &markdown))?;

    let source = imported_file_source(SourceMeta::default(), props, path, &created_at, byte_size);
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
        linked_projects: Vec::new(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        source,
    };
    let document =
        index_fresh_document(state, &vault_file, &meta, &chunks, &embeddings, None, None)?;
    Ok(Outcome::Indexed {
        document,
        warning: None,
    })
}

/// The source facts of a file the user handed PM directly — a drag-and-drop or folder import (#709).
///
/// The document IS the source here, so "what the source says" is what the FILE says, and a vault
/// import stops being the one path where every fact reads "Unknown" about a file PM has in its hands.
/// The document's own creation date wins over the filesystem's when it states one, for the reason it
/// wins everywhere: it describes the document rather than when this copy reached this disk. Where it
/// states nothing the filesystem's birth time stands, so a dropped .txt still answers "Created".
///
/// `mtime` doubles as `source_modified_at` — the same value PM already stores as its own `created_at`
/// for an import. Not a second opinion, just the one PM has, under the heading it belongs to.
/// `base` carries the discriminator — a plain import is a vault document, a workbook is a
/// `'spreadsheet'` — so this adds facts without ever changing what kind of thing the row is.
fn imported_file_source(
    base: SourceMeta,
    props: crate::sidecar::FileProperties,
    path: &Path,
    mtime: &str,
    byte_size: Option<i64>,
) -> SourceMeta {
    SourceMeta {
        source_author: props.author,
        source_last_modified_by: props.last_modified_by,
        // `or_else`, so a document that stated its own creation date costs no stat at all.
        source_created_at: props.created_at.or_else(|| file_birth_time(path)),
        source_modified_at: Some(mtime.to_string()),
        source_size_bytes: byte_size,
        ..base
    }
}

/// When the filesystem says this file came into being (ISO-8601), if it will say.
///
/// Genuinely unavailable on some platform/filesystem pairs — `Unsupported` on Linux kernels or
/// filesystems without `statx` birth-time support — so `None` is the ordinary case rather than a
/// failure, and reads as "Unknown" like any other source that will not say.
fn file_birth_time(path: &Path) -> Option<String> {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
}

/// Ingest one image: OCR + EXIF via the sidecar, then the SAME chunk/embed/index pipeline as a
/// document, via a synthetic Markdown body (a metadata chunk + the OCR text). The image's bytes are
/// the identity (SHA-256 = both the dedupe `content_hash` and `photos.file_hash`), so a moved/renamed
/// file still dedupes. OCR is requested only when the optional component is installed; declining it
/// still ingests the photo with its EXIF metadata chunk. With `copy_photos_to_vault`, the original is
/// copied into `vault/photos/` (following the vault cipher) and recorded on the `photos` row — this
/// also applies on a dedupe hit (a re-drop with the opt-in newly checked saves the copy without
/// re-indexing). On the fresh path that copy happens **after** the index commits and cannot fail the
/// import; see the comment at the call.
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
            // record it in BOTH places that describe a saved copy.
            if opts.copy_photos_to_vault {
                let rel = copy_original_to_vault(vault, cipher, &bytes, &file_hash, ext)?;
                record_saved_photo_copy(&conn, vault, cipher, &file_hash, &rel)?;
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
    // `ocr_ran` is the only thing that tells "OCR was asked for and broke" from "this image holds
    // no text" — the two produce a byte-identical document otherwise, so a receipt indexed with no
    // searchable text looked entirely normal. A cold model cache no longer lands here (the sidecar
    // reports that as a miss and the fetcher fills it), so what is left is a genuinely broken
    // component, and the user is the only one who can act on it.
    let ocr_warning = (run_ocr && !analysis.ocr_ran).then(|| {
        eprintln!(
            "ingest: photo indexed without OCR (the text-recognition component did not run): {}",
            path.display()
        );
        "text recognition did not run, so any text in this image is not searchable".to_string()
    });

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

    // Opt-in: PREDICT where the copy will go. No IO here — the copy itself happens after the index
    // commits, below. The name is deterministic from the hash, the extension and the cipher's policy,
    // and [`photo_copy_rel_path`] is the single expression that computes it for both.
    let (saved_to_vault, image_vault_path) = if opts.copy_photos_to_vault {
        (true, Some(photo_copy_rel_path(cipher, &file_hash, ext)))
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
        linked_projects: &[],
        tags: &[],
        importance: None,
        last_activity: &ingested_at,
        reviewed: false,
        photo: Some(&photo),
        spreadsheet: None,
        chat: None,
        source_id: None,
        external_ref: None,
    };
    let vault_file = vault.join(&vault_name);
    cipher.write_to(&vault_file, &render_markdown(&front, &body))?;

    let meta = DocMeta {
        source_path: Some(path.to_string_lossy().into()),
        vault_path: vault_name,
        title,
        // Cloned, not moved: the post-commit vault copy below still names the file by this hash.
        content_hash: file_hash.clone(),
        ext: ext.map(str::to_string),
        byte_size: Some(byte_size),
        created_at: Some(capture_date),
        last_activity: Some(ingested_at.clone()),
        ingested_at,
        project: "Unsorted".into(),
        linked_projects: Vec::new(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        source: SourceMeta::photo(),
    };
    let document = index_fresh_document(
        state,
        &vault_file,
        &meta,
        &chunks,
        &embeddings,
        Some(&photo),
        None,
    )?;

    // The copy runs LAST, after the document is committed, and deliberately without `?`.
    //
    // It used to run before the split/embed/vault-write/index above, any of which can fail — and on
    // that failure `index_fresh_document` removed the Markdown it had written while nothing removed
    // the photo blob. The blob is content-addressed, so after the rollback no document row, no photos
    // row and no vault file named its hash: `find_saved_photo_copy` could never adopt it, the document
    // walks exclude `photos/`, and no view could show it. PM reported a failed import and silently kept
    // an encrypted copy of the user's picture forever.
    //
    // The polarity this accepts, plainly: the database may now claim a copy that is not there. That is
    // the better direction and it is a decision, not an oversight. A leftover blob is adopted by
    // nothing and visible to no one; a dangling pointer is announced here as a warning on the file's
    // Activity row, degrades openly in every reader (`read_document_image` falls back to the source
    // file), and heals the moment the same image is re-dropped with the opt-in still ticked. Note it
    // does NOT self-heal on a Rebuild — `heal_photo_copy` early-returns while `saved_to_vault` is
    // true — which is exactly why the warning at the moment of failure has to be user-visible.
    //
    // `bytes` is still the buffer read at the top (the one that produced `file_hash`); do not
    // "optimise" this into a re-read, since the source file may have moved by now.
    let mut copy_warning = None;
    if saved_to_vault {
        if let Err(e) = copy_original_to_vault(vault, cipher, &bytes, &file_hash, ext) {
            eprintln!(
                "ingest: photo indexed but its vault copy could not be saved: {} ({e})",
                path.display()
            );
            copy_warning = Some(
                "the copy of the original could not be saved to the vault, so keep the file where \
                 it is"
                    .to_string(),
            );
        }
    }

    Ok(Outcome::Indexed {
        document,
        // `warning` is single-valued, so JOIN rather than assign: a photo whose OCR failed AND whose
        // copy failed must not have the first note silently overwritten by the second.
        warning: join_warnings([ocr_warning, copy_warning]),
    })
}

/// Fold several optional notes into the one `warning` slot [`Outcome::Indexed`] carries, dropping the
/// empties. Separate function so "both things went wrong" cannot be lost to a plain assignment.
fn join_warnings(parts: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let joined: Vec<String> = parts.into_iter().flatten().collect();
    (!joined.is_empty()).then(|| joined.join("; "))
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
    // A workbook is an OOXML container like any other, so it states an author too (#709). A .csv
    // states nothing and is never asked.
    let props = state.sidecar.file_properties(path);
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
        linked_projects: &[],
        tags: &[],
        importance: None,
        last_activity: &ingested_at,
        reviewed: false,
        photo: None,
        spreadsheet: Some(&record),
        chat: None,
        source_id: None,
        external_ref: None,
    };
    let vault_file = vault.join(&vault_name);
    cipher.write_to(&vault_file, &render_markdown(&front, &body))?;

    let source = imported_file_source(
        SourceMeta::spreadsheet(),
        props,
        path,
        &created_at,
        byte_size,
    );
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
        linked_projects: Vec::new(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        source,
    };
    let document = index_fresh_document(
        state,
        &vault_file,
        &meta,
        &chunks,
        &embeddings,
        None,
        Some(&record),
    )?;
    Ok(Outcome::Indexed {
        document,
        warning: None,
    })
}

/// Promote an index-only document to a **full local spreadsheet import** — the "import fully" flow. The
/// caller (the promote command) has already exported the source's full grid to `path` (an `.xlsx`
/// staged from Drive). This parses it values-only, shapes the synthetic sheet body exactly like a fresh
/// [`ingest_spreadsheet`], and then transforms the document **in place** (same `doc_id`): its chunks are
/// swapped from the index-only summary to the real leaves, a `spreadsheets` satellite row is added, and
/// `source_type` flips `index_only` → `spreadsheet`. The user's classification (project / tags /
/// importance / reviewed / entity link) is preserved, and the vault Markdown becomes the source of truth
/// (so it rebuilds from disk, and the truth-writer now routes to front-matter, not the manifest).
///
/// Two ghost-prevention invariants make this safe against re-duplication:
/// 1. The document **keeps its `source_id`** (+ `external_ref` + pointer hashes) as a *claim marker* —
///    round-tripped through the vault front-matter so it survives a Rebuild — so the connector sync sees
///    the still-present source as already-imported ([`crate::drive::read_item_state`]) and no-ops.
/// 2. The source is **stripped from the encrypted manifest** ([`crate::index_only::forget_source`]) so
///    `merged_manifest`'s DB-∪-file union can't resurrect it as an index-only pointer.
#[allow(clippy::too_many_arguments)]
pub fn promote_spreadsheet(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault: &Path,
    cipher: &MarkdownCipher,
    vault_root: &Path,
    manifest_cipher: &crate::index_only::ManifestCipher,
    doc_id: i64,
    path: &Path,
    ext: Option<&str>,
) -> Result<Document> {
    // 1. Read the existing row's identity + classification (short lock). Promotion is only valid on an
    //    index-only document that still carries a source pointer.
    #[allow(clippy::type_complexity)]
    let (
        source_id,
        external_ref,
        project,
        tags,
        importance,
        reviewed,
        title,
        created_at,
        now,
        linked_projects,
    ): (
        String,
        Option<String>,
        String,
        Vec<String>,
        Option<String>,
        bool,
        String,
        Option<String>,
        String,
        Vec<String>,
    ) = {
        let conn = state.conn()?;
        let (
            source_type,
            source_id,
            external_ref,
            project,
            tags_json,
            importance,
            reviewed,
            title,
            created_at,
        ): (
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            Option<String>,
            i64,
            String,
            Option<String>,
        ) = conn.query_row(
            "SELECT source_type, source_id, external_ref, project, tags, importance, reviewed, \
                    title, created_at \
             FROM documents WHERE id = ?1",
            params![doc_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )?;
        if source_type != SOURCE_TYPE_INDEX_ONLY {
            return Err(Error::Other(
                "This document is already imported locally.".into(),
            ));
        }
        // Promotion preserves the user's filing, and that now includes which OTHER projects the
        // item was linked into. Read here, beside the rest of it, while the connection is open.
        let linked_projects = crate::tags::linked_projects(&conn, doc_id, &project)?;
        let source_id = source_id.ok_or_else(|| {
            Error::Other("This indexed item has no source pointer to import from.".into())
        })?;
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        let now = iso_now(&conn)?;
        (
            source_id,
            external_ref,
            project,
            tags,
            importance,
            reviewed != 0,
            title,
            created_at,
            now,
            linked_projects,
        )
    };

    // 2. Parse the exported grid values-only (sidecar; no lock) and shape the synthetic sheet body — the
    //    exact same shaping a locally-dropped spreadsheet gets.
    let sheets = state.sidecar.analyze_spreadsheet(path, ext.unwrap_or(""))?;
    let Some((body, record)) = spreadsheets::to_markdown(&sheets) else {
        return Err(Error::Other(
            "This spreadsheet has no readable rows to import.".into(),
        ));
    };
    let content_hash = hex_digest(body.as_bytes());
    let byte_size = std::fs::metadata(path).map(|m| m.len() as i64).ok();

    // 3. Don't import content that already exists as another local document (e.g. the same file was also
    //    dropped in by hand) — that would violate the `content_hash` UNIQUE constraint at commit anyway.
    {
        let conn = state.conn()?;
        let clashes: bool = conn
            .query_row(
                "SELECT 1 FROM documents WHERE content_hash = ?1 AND id != ?2",
                params![content_hash, doc_id],
                |_| Ok(()),
            )
            .optional_exists()?;
        if clashes {
            return Err(Error::Other(
                "This spreadsheet is already imported locally.".into(),
            ));
        }
    }

    // 4. Chunk + embed the full body off the lock, like a fresh ingest.
    let chunks = split_document(gateway, &body, &title, &content_hash)?;
    let texts = leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;

    // 5. Write the vault truth BEFORE the swap, carrying the preserved classification + the connector
    //    pointer (so a Rebuild reproduces the promoted document, claim included).
    let created = created_at.as_deref().unwrap_or(now.as_str());
    let vault_name = cipher.on_disk_name(&vault_filename(&title, &content_hash));
    let front = Frontmatter {
        title: &title,
        source_path: "",
        ext,
        content_hash: &content_hash,
        created_at: created,
        ingested_at: &now,
        project: &project,
        linked_projects: &linked_projects,
        tags: &tags,
        importance: importance.as_deref(),
        last_activity: &now,
        reviewed,
        photo: None,
        spreadsheet: Some(&record),
        chat: None,
        source_id: Some(&source_id),
        external_ref: external_ref.as_deref(),
    };
    let vault_file = vault.join(&vault_name);
    cipher.write_to(&vault_file, &render_markdown(&front, &body))?;

    // 6. Flip the row in place (same doc_id → keeps classification, entity link, and any citations): swap
    //    the index-only summary chunk for the real leaves, add the satellite row, and clear the
    //    index-only-only columns — but KEEP `source_id`/`external_ref` (the claim marker) and set the
    //    state to a reachable, fully-stored spreadsheet.
    let swap = (|| -> Result<()> {
        let mut conn = state.conn()?;
        let tx = conn.transaction()?;
        replace_chunks(&tx, doc_id, &chunks, &embeddings, false, None)?;
        tx.execute(
            "INSERT INTO spreadsheets (document_id, sheet_count, total_rows, chunked_rows) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                doc_id,
                record.sheet_count,
                record.total_rows,
                record.chunked_rows
            ],
        )?;
        tx.execute(
            "UPDATE documents SET source_type = ?2, source_state = ?3, vault_path = ?4, \
                    content_hash = ?5, ext = ?6, byte_size = ?7, source_path = NULL, \
                    stored_summary = NULL, source_parent_folder_id = NULL, \
                    source_parent_folder_name = NULL, ingested_at = ?8, last_activity = ?8 \
             WHERE id = ?1",
            params![
                doc_id,
                SOURCE_TYPE_SPREADSHEET,
                SOURCE_STATE_OK,
                vault_name,
                content_hash,
                ext,
                byte_size,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = swap {
        // The DB swap rolled back with its transaction; remove the vault file written in step 5 so a
        // failed promote leaves no orphan.
        let _ = std::fs::remove_file(&vault_file);
        return Err(e);
    }

    // 7. Strip the promoted source from the encrypted manifest so it can't be resurrected as an
    //    index-only ghost. AFTER the commit — the DB mirror already excludes it, so even a racing sync's
    //    `write_synced` can only re-add it from the file, which this then removes.
    crate::index_only::forget_source(vault_root, manifest_cipher, &source_id)?;

    let conn = state.conn()?;
    load_document(&conn, doc_id)
}

/// Ingest (or re-ingest) a pinboard note as a REAL vault Markdown document. The note is already
/// Markdown, so it is written to the vault like any document — the FULL body in a `.md`/`.pmenc`
/// file, real chunk text, full-text FTS — not an index-only pointer that keeps only a 500-char
/// summary. This is what makes a note survive its own deletion: the body lives in the vault, and a
/// Rebuild reproduces it losslessly from disk.
///
/// Identity is the note's stable `source_id` (`note:<widget_id>`), folded into `content_hash`
/// ([`crate::index_only::pointer_content_hash`]) so two notes with identical text stay distinct and
/// an edit-then-revert is a clean no-op. Three cases, keyed off the existing row for this source id:
///   * **none** — a fresh ingest (project `Unsorted`, unreviewed), into the review queue;
///   * **an existing vault note** — rewrite the SAME vault file and re-embed in place, KEEPING its
///     project / tags / importance / reviewed (the "Re-ingest edits" affordance);
///   * **a legacy `index_only` note** (shipped in v2.89.0-alpha #214) — promote it in place: materialise
///     the vault `.md`, swap the summary chunk for the real leaves, clear the index-only-only columns,
///     keep the filing, and drop the manifest pointer so it can't be resurrected as a ghost.
///
/// Nothing reconciles a `note:` source, so the document is standalone; the vault filename is the
/// stable widget id, so an edit overwrites the same file instead of orphaning one per revision.
///
/// The existing document for a note's `source_id`, with the filing to carry forward on re-ingest.
struct ExistingNote {
    doc_id: i64,
    source_type: String,
    content_hash: String,
    /// Compared against the incoming title by the idempotency guard. `content_hash` covers the
    /// BODY only (`pointer_content_hash(source_id, body)`), so without this a rename was a Noop.
    title: String,
    project: String,
    tags: Vec<String>,
    importance: Option<String>,
    reviewed: bool,
    created_at: Option<String>,
    vault_path: String,
}

#[allow(clippy::too_many_arguments)]
pub fn ingest_note_document(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault: &Path,
    cipher: &MarkdownCipher,
    vault_root: &Path,
    manifest_cipher: &crate::index_only::ManifestCipher,
    widget_id: &str,
    title: &str,
    body: &str,
) -> Result<Document> {
    let source_id = format!("note:{widget_id}");
    let content_hash = crate::index_only::pointer_content_hash(&source_id, body);

    // The existing document for this note, if any — across source types (a fresh `vault` note or a
    // legacy `index_only` pointer from #214). Read the filing so an update/promote can preserve it.
    let existing: Option<ExistingNote> = {
        let conn = state.conn()?;
        match conn.query_row(
            "SELECT id, source_type, content_hash, title, project, tags, importance, reviewed, \
                    created_at, vault_path \
             FROM documents WHERE source_id = ?1",
            params![source_id],
            |r| {
                let tags_json: String = r.get(5)?;
                Ok(ExistingNote {
                    doc_id: r.get(0)?,
                    source_type: r.get(1)?,
                    content_hash: r.get(2)?,
                    title: r.get(3)?,
                    project: r.get(4)?,
                    tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                    importance: r.get(6)?,
                    reviewed: r.get::<_, i64>(7)? != 0,
                    created_at: r.get(8)?,
                    vault_path: r.get(9)?,
                })
            },
        ) {
            Ok(row) => Some(row),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        }
    };

    // Unchanged and already a full vault note → nothing to do (the idempotency guarantee).
    //
    // The TITLE is part of "unchanged": `content_hash` is `pointer_content_hash(source_id, body)`,
    // which never sees the title, so renaming a pinboard note (editable titles shipped in #349) and
    // re-ingesting an untouched body took this early return — leaving the old title in the DB, in
    // the vault file's front-matter, and everywhere the note is cited.
    //
    // A title change re-chunks and re-embeds by falling through, which is deliberate and not
    // wasteful: the title is the breadcrumb prepended to every leaf's `embed_content` (never its
    // display text), so it is genuinely part of what was embedded. A cheaper title-only UPDATE
    // would leave every chunk's vector keyed to the OLD title — the rename would show in the UI
    // while search kept answering to the name the user just changed away from.
    if let Some(e) = &existing {
        if e.source_type == SOURCE_TYPE_VAULT && e.content_hash == content_hash && e.title == title
        {
            let conn = state.conn()?;
            return load_document(&conn, e.doc_id);
        }
    }

    // Chunk + embed the full body off the lock, like a fresh ingest.
    let chunks = split_document(gateway, body, title, &content_hash)?;
    let texts = leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;

    let now = {
        let conn = state.conn()?;
        iso_now(&conn)?
    };

    // Keep an existing note's filing; a fresh note starts unsorted in the review queue.
    let (project, tags, importance, reviewed, created_at, old_vault_path) = match &existing {
        Some(e) => (
            e.project.clone(),
            e.tags.clone(),
            e.importance.clone(),
            e.reviewed,
            e.created_at.clone().unwrap_or_else(|| now.clone()),
            Some(e.vault_path.clone()),
        ),
        None => (
            "Unsorted".into(),
            Vec::new(),
            None,
            false,
            now.clone(),
            None,
        ),
    };
    // Editing a note re-writes its whole vault file, so its other project memberships have to be
    // carried across with the rest of the filing or the edit quietly unlinks it. A brand-new note
    // has none.
    let linked_projects = match &existing {
        Some(e) => {
            let conn = state.conn()?;
            crate::tags::linked_projects(&conn, e.doc_id, &project)?
        }
        None => Vec::new(),
    };

    // The note is named by its widget id, which arrives over IPC. Restrict it to a conservative id
    // charset so the derived name is always a single, ordinary filename inside the vault and can't
    // point anywhere else. Real ids (a UUID, or the `w-…` fallback) are unaffected.
    if widget_id.is_empty()
        || !widget_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::Other("invalid note id".into()));
    }
    // Name the file by the stable widget id so an edit overwrites the same file.
    let vault_name = cipher.on_disk_name(&format!("note-{widget_id}.md"));
    let vault_file = vault.join(&vault_name);
    // If we're overwriting an existing note's file in place, snapshot its bytes first so a failed DB
    // half can restore them — the vault file must never diverge from the (rolled-back) DB row it
    // mirrors, or a later Rebuild would bake the divergence in.
    let overwrote_existing_file = old_vault_path
        .as_deref()
        .is_some_and(|p| p == vault_name && !p.starts_with("idx://"));
    let prior_bytes = if overwrote_existing_file {
        std::fs::read(&vault_file).ok()
    } else {
        None
    };

    // Write the vault truth (full body) before touching the DB, carrying the note's `source_id` claim so
    // a Rebuild re-links it to the board.
    let front = Frontmatter {
        title,
        source_path: "",
        ext: Some("md"),
        content_hash: &content_hash,
        created_at: &created_at,
        ingested_at: &now,
        project: &project,
        linked_projects: &linked_projects,
        tags: &tags,
        importance: importance.as_deref(),
        last_activity: &now,
        reviewed,
        photo: None,
        spreadsheet: None,
        chat: None,
        source_id: Some(&source_id),
        external_ref: None,
    };
    cipher.write_to(&vault_file, &render_markdown(&front, body))?;

    let result = (|| -> Result<Document> {
        match &existing {
            None => {
                // Fresh: guard against a content_hash clash with an unrelated document, then index.
                {
                    let conn = state.conn()?;
                    let clashes: bool = conn
                        .query_row(
                            "SELECT 1 FROM documents WHERE content_hash = ?1",
                            params![content_hash],
                            |_| Ok(()),
                        )
                        .optional_exists()?;
                    if clashes {
                        return Err(Error::Other(
                            "A document with identical text is already ingested.".into(),
                        ));
                    }
                }
                let meta = DocMeta {
                    source_path: None,
                    vault_path: vault_name.clone(),
                    title: title.to_string(),
                    content_hash: content_hash.clone(),
                    ext: Some("md".into()),
                    byte_size: None,
                    created_at: Some(created_at.clone()),
                    last_activity: Some(now.clone()),
                    ingested_at: now.clone(),
                    project: project.clone(),
                    linked_projects: linked_projects.clone(),
                    tags: tags.clone(),
                    importance: importance.clone(),
                    reviewed,
                    source: SourceMeta {
                        source_id: Some(source_id.clone()),
                        ..SourceMeta::default()
                    },
                };
                index_document(state, &meta, &chunks, &embeddings, None, None)
            }
            Some(e) => {
                let doc_id = e.doc_id;
                // Update or promote in place (same id → keeps entity link + any citations): swap the
                // chunks for the real leaves and clear the index-only-only columns, KEEPING the filing.
                let mut conn = state.conn()?;
                let tx = conn.transaction()?;
                replace_chunks(&tx, doc_id, &chunks, &embeddings, false, None)?;
                tx.execute(
                    "UPDATE documents SET source_type = ?2, source_state = ?3, vault_path = ?4, \
                            content_hash = ?5, ext = ?6, title = ?7, byte_size = NULL, \
                            source_path = NULL, stored_summary = NULL, source_modified_at = NULL, \
                            source_content_hash = NULL, source_parent_folder_id = NULL, \
                            source_parent_folder_name = NULL, ingested_at = ?8, last_activity = ?8 \
                     WHERE id = ?1",
                    params![
                        doc_id,
                        SOURCE_TYPE_VAULT,
                        SOURCE_STATE_OK,
                        vault_name,
                        content_hash,
                        "md",
                        title,
                        now,
                    ],
                )?;
                tx.commit()?;
                load_document(&conn, doc_id)
            }
        }
    })();

    let document = match result {
        Ok(d) => d,
        Err(e) => {
            // The DB half rolled back (or never ran). Put the vault file back exactly as the DB still
            // sees it: restore the prior bytes when we overwrote an existing note, else remove the
            // freshly-written file. Keeps file and DB consistent — no divergence a Rebuild would bake in.
            match prior_bytes {
                Some(bytes) => {
                    // Atomic like every other vault write: a rollback that is itself interrupted
                    // would destroy the very file it exists to put back.
                    let _ = crate::vault::write_atomic(&vault_file, &bytes);
                }
                None => {
                    let _ = std::fs::remove_file(&vault_file);
                }
            }
            return Err(e);
        }
    };

    // A promoted legacy note must be stripped from the encrypted manifest so the DB-∪-file union can't
    // resurrect it as an index-only ghost. AFTER the commit (mirror already excludes it). Idempotent.
    if matches!(&existing, Some(e) if e.source_type == SOURCE_TYPE_INDEX_ONLY) {
        crate::index_only::forget_source(vault_root, manifest_cipher, &source_id)?;
    }
    // Remove a stale prior vault file if the on-disk name changed (a vault re-key) — never a synthetic
    // index-only path (`idx://…`), which isn't a file.
    if let Some(old) = old_vault_path {
        if old != vault_name && !old.starts_with("idx://") {
            let _ = std::fs::remove_file(vault.join(&old));
        }
    }

    Ok(document)
}

/// Where a saved photo original for `file_hash` goes, vault-relative — the string that lands on the
/// `photos` row and in the document's front-matter.
///
/// Pure, and the ONLY expression allowed to compute that name: [`ingest_photo`] now writes the row
/// and the front-matter before the copy exists (so a copy failure cannot lose an indexed document),
/// which means the prediction and the write have to agree byte for byte. They agree because the write
/// is built from this. And the stake is higher than a wrong pointer: the on-disk name is the AAD stem
/// (`MarkdownCipher::aad_stem`), so a name that drifted by one character yields a blob that cannot be
/// decrypted at all.
fn photo_copy_rel_path(cipher: &MarkdownCipher, file_hash: &str, ext: Option<&str>) -> String {
    format!(
        "{PHOTOS_SUBDIR}/{}",
        cipher.on_disk_name(&photo_copy_base_name(file_hash, ext))
    )
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
    std::fs::create_dir_all(vault.join(PHOTOS_SUBDIR))?;
    // Derived from [`photo_copy_rel_path`] rather than recomputed, so the name written and the name
    // stored can never be two expressions that drift apart.
    let rel = photo_copy_rel_path(cipher, file_hash, ext);
    cipher.write_bytes_to(&vault.join(&rel), bytes)?;
    Ok(rel)
}

/// The logical (pre-`.pmenc`) filename of a saved photo original. Content-addressed, so re-saving
/// the same image overwrites its own copy instead of accumulating duplicates — which is what makes
/// [`copy_original_to_vault`] idempotent and lets [`find_saved_photo_copy`] locate a copy from
/// front-matter alone.
fn photo_copy_base_name(file_hash: &str, ext: Option<&str>) -> String {
    format!("{file_hash}.{}", ext.unwrap_or("img"))
}

/// Locate an opt-in saved original for `file_hash` in `vault/photos/`, returning its vault-relative
/// path. Used to heal a photo whose front-matter lost track of its copy (see [`rebuild_one`]).
///
/// Tries both on-disk forms, because a photo keeps the name it was saved under: a vault whose
/// encryption policy later flipped has its originals re-encoded **in place** by
/// [`convert_photo_originals`], so the `.pmenc` suffix reflects the policy at save time, not now.
fn find_saved_photo_copy(vault: &Path, file_hash: &str, ext: Option<&str>) -> Option<String> {
    let base = photo_copy_base_name(file_hash, ext);
    let encrypted = format!("{base}{}", crate::vault::ENCRYPTED_SUFFIX);
    [base, encrypted].into_iter().find_map(|name| {
        // `is_file()` stays here on purpose, unlike every vault WALK: this is a best-effort heal
        // probe, `heal_photo_copy` treats `None` as "no heal" and never clears an existing pointer,
        // so a read failure lands on the pre-heal status quo. Hardening it turns a repair into a
        // hard failure.
        vault
            .join(PHOTOS_SUBDIR)
            .join(&name)
            .is_file()
            .then(|| format!("{PHOTOS_SUBDIR}/{name}"))
    })
}

/// Drop the derived index and rebuild it from the Markdown vault. Proves the
/// store is reconstructable from disk (spec §3 acceptance). Index-only items (no vault file) are
/// restored from the encrypted manifest, re-embedded from their offline summaries.
///
/// `extra_total` is folded into the progress `Counted` so the bar can span a SECOND phase the caller
/// runs afterwards — the async full-body re-index of index-only items (network I/O this blocking fn
/// can't do). Returns `(ingested, failed)` so the caller emits the terminal `Finished` once that phase
/// is done, rather than this fn ending the run prematurely.
/// The id of one rebuild pass — a uuid minted per run and stamped onto every document as that document
/// commits (`documents.rebuild_pass`, v35). It **is** the checkpoint #371 asks for: a resumed pass carries
/// the SAME id, so the documents its interrupted predecessor already finished are recognised and skipped;
/// a fresh Rebuild mints a NEW id, so nothing is skipped and "my index looks wrong, rebuild it" still
/// redoes everything.
pub(crate) fn new_pass_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// What a pass should do with one enumerated vault file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebuildPlan {
    /// This same pass already committed it, before whatever interruption stopped the run — skip it.
    /// The whole point of #371: the resumed run does only the work that was left.
    AlreadyDone,
    /// No pass, or an older one, owns this document: re-split, re-embed, write it in place.
    Rebuild,
}

/// The resume rule. Pure, so the one decision the incremental rebuild turns on is testable without a
/// store, a sidecar or an `AppState` — the shape [`crate::index_only::react`] uses for the same reason.
///
/// Deliberately NOT keyed on `content_hash`: a Rebuild's dominant trigger is a splitter/embedder change,
/// where every hash is identical and every chunk boundary must still move. Nor on a per-document copy of
/// the retrieval config: on a manual repair nothing has changed, so every document would be skipped and
/// the repair would do nothing. "Did THIS run already do it" is the only question resume needs answered.
pub(crate) fn plan_rebuild_one(stored_pass: Option<&str>, pass: &str) -> RebuildPlan {
    match stored_pass {
        Some(stored) if stored == pass => RebuildPlan::AlreadyDone,
        _ => RebuildPlan::Rebuild,
    }
}

/// What the final sweep should do with a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReapPlan {
    Keep,
    /// The vault file behind it is provably gone — delete the row.
    Delete,
}

/// What the sweep managed to learn about a document's vault file, taken FRESH at sweep time.
///
/// The three-way split is the whole safety of the sweep. `std::path::Path::exists()` collapses
/// "definitely not there" and "I couldn't tell" into one `false` — so a vault on a network share that
/// drops mid-pass, or a folder an antivirus scanner briefly locks, would report every file as absent and
/// the sweep would delete the entire library. A deletion decision may only ever be made on a PROVABLE
/// absence, which is what `try_exists` distinguishes and this enum preserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileState {
    Present,
    /// The filesystem positively reported no such file.
    Gone,
    /// We could not tell (permission denied, an unreachable root, an I/O error).
    Unknown,
}

impl FileState {
    fn of(path: &Path) -> Self {
        match path.try_exists() {
            Ok(true) => FileState::Present,
            Ok(false) => FileState::Gone,
            Err(_) => FileState::Unknown,
        }
    }
}

/// Stat one path for a walk that must not confuse "it isn't there" with "I couldn't look".
///
/// The same three-way split [`FileState`] makes for the sweep, but handing back the metadata a walk
/// needs to tell a file from a directory: `Ok(Some(meta))` = read it, `Ok(None)` = the filesystem
/// positively reported no such path (a PROVABLE absence), `Err` = we could not tell.
///
/// This exists because [`Path::is_file`] and [`Path::is_dir`] are `metadata(..).map(..).unwrap_or(false)`:
/// inside a walk they fold a permission denial, an I/O error, a network share that dropped mid-pass and
/// a genuinely absent file into one `false`, and the entry then simply vanishes from the enumeration
/// with no trace. Every downstream decision — reap, re-key, export, back up — is made on that
/// enumeration, so the distinction has to survive it. `NotFound` is the only error kind that is safe to
/// read as absence, which is exactly the rule [`FileState`] documents.
pub(crate) fn probe(path: &Path) -> std::io::Result<Option<std::fs::Metadata>> {
    // Deliberately `metadata`, not `symlink_metadata`: the walks index what a link POINTS AT, so a
    // dangling symlink resolves to `NotFound` — provably nothing to read, not a failure to look.
    classify_probe(std::fs::metadata(path))
}

/// The rule [`probe`] applies, as a free function so "which error kinds may be read as absence" is
/// testable without a filesystem that can be made to fail on demand. `NotFound` is the only one.
fn classify_probe<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// May the sweep delete anything at all? Only on a **provably complete** walk — every vault dir entry
/// enumerated. A partial walk can mean the vault root itself only half-read, where "we never saw it" is
/// not "the user deleted it"; sweeping on that picture could delete most of the library. Withholding it
/// costs nothing: the next complete pass reaps instead.
///
/// Kept as a `bool` although the walks now hand back a COUNT: every caller passes `unreadable == 0`,
/// and the question this answers is genuinely binary — one entry unread is as disqualifying as fifty.
///
/// Deliberately NOT gated on per-document failures. A document that failed this pass still has its vault
/// file sitting there, so [`plan_reap`] keeps it on presence alone — a failure endangers no reap
/// decision. Gating on failures *looks* safer and is worse: one permanently-broken file (an orphaned
/// chat `.md`, two vault files sharing a content hash) would withhold the sweep on EVERY future rebuild,
/// so a document the user deleted could never be reaped again.
pub(crate) fn may_reap(enumeration_complete: bool) -> bool {
    enumeration_complete
}

/// Is one document a leftover the sweep should delete? Keyed on the vault file alone, because the vault
/// IS the truth: no file, no document.
///
/// Deliberately independent of the pass stamp. A document another writer added after the walk enumerated
/// (a drag-drop ingest racing the pass) carries no stamp yet its file is right there — `Present` keeps
/// it, and PM always writes the vault file before the index row, so a half-finished insert can never
/// look `Gone`. Conversely a document this pass DID rebuild, whose file the user then deleted while the
/// app was closed between an interruption and its resume, is skipped by the resume and never re-read —
/// keying on the file still reaps it.
pub(crate) fn plan_reap(file: FileState) -> ReapPlan {
    match file {
        FileState::Gone => ReapPlan::Delete,
        FileState::Present | FileState::Unknown => ReapPlan::Keep,
    }
}

/// Which pass last rebuilt the document at `vault_path`, if PM holds one at all.
fn stored_pass(conn: &Connection, vault_path: &str) -> Result<Option<Option<String>>> {
    Ok(conn
        .query_row(
            "SELECT rebuild_pass FROM documents WHERE vault_path = ?1",
            params![vault_path],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?)
}

/// Re-index every Markdown file in the vault. Returns `(ingested, skipped, failed, unreadable)` —
/// the fourth is the vault walk's own count of entries it could not read ([`walk_vault_markdown`]),
/// which is what the caller puts on the terminal `Finished` so a partial rebuild does not report a
/// clean run. It is deliberately NOT folded into `failed`: `failed` is what withholds the retrieval
/// stamp, and an unreadable *entry* is not a document that failed to rebuild.
pub fn rebuild(
    app: &AppHandle,
    on_event: &ProgressSink,
    extra_total: usize,
    pass: &str,
    on_pass_start: &dyn Fn() -> Result<()>,
) -> Result<(usize, usize, usize, usize)> {
    let state = app.state::<AppState>();

    // Indexing is active use — hold the idle chat-indexer (card 7B) off so it doesn't contend with it.
    state.mark_user_activity();

    on_event.send(IngestEvent::Preparing {
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
        on_event.send(IngestEvent::Preparing {
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

    // The model is confirmed ready, so the mutating phase is about to begin: let the caller persist its
    // crash-resume marker FIRST, and propagate its error. If the marker can't be recorded, an
    // interruption would strand a half-rebuilt index with nothing to resume from — so refuse to start
    // work we could not recover, with the index still fully intact. (Deliberately after the warmup: a
    // model that can't load destroys nothing, so it must not leave a marker that makes every subsequent
    // launch retry a rebuild that fails identically.)
    on_pass_start()?;

    // TWO ARMS. A changed vector width means every stored vector is the wrong model AND the wrong shape,
    // so nothing is reusable — and `chunk_vec` must be EMPTY before it can be resized (`ensure_vec_dim`
    // refuses a populated table, by design). That arm keeps the historical wipe. Every other rebuild — a
    // splitter change, a manual repair, a resume — reuses nothing but destroys nothing either: rows are
    // upserted in place and only genuine leftovers are swept at the end.
    //
    // The wipe arm is still resumable: the resize lands before the loop, so a resumed run finds the width
    // already correct, takes the incremental arm, and skips whatever the interrupted run had committed.
    // `ensure_vec_dim` early-returns when the width already matches, so calling it on both arms is a
    // no-op on the incremental one.
    let wipe = {
        let conn = state.conn()?;
        crate::db::vec0_dim(&conn)? != embedder.dimension
    };
    {
        let conn = state.conn()?;
        if wipe {
            conn.execute_batch(
                "DELETE FROM chunks_fts; DELETE FROM chunk_vec; DELETE FROM chunks; DELETE FROM documents;",
            )?;
            // A wholesale delete orphans EVERY tag at once, by cascade, and the per-document prune in
            // `tags::set_document_projects` can never reach them: a tag whose documents are all gone is
            // in no surviving document's leaving-set. Re-filing heals the tags the vault still names —
            // it re-interns them — but a label no file mentions any more would linger forever, in the
            // pickers and in the cached filing prompt. Swept here, immediately after the wipe that
            // created them, rather than at the end of the run: a rebuild commits one transaction per
            // document, so a sweep deferred to the end is one a crash can skip entirely, and there is no
            // boot-time or periodic registry GC to catch what it missed.
            crate::tags::prune_orphan_project_tags(&conn)?;
            crate::tags::prune_orphan_group_tags(&conn)?;
        }
        crate::db::ensure_vec_dim(&conn, embedder.dimension)?;
    }
    let (vault, cipher) = state.markdown_io()?;
    // Collect the vault-markdown files up front so we know the total before the loop — the UI
    // shows a determinate bar from this count. Accept both plaintext (`.md`) and encrypted
    // (`.md.pmenc`) files; the cipher decides per file how to read them (read-by-magic). An
    // unreadable dir entry no longer just disappears: it makes the picture PARTIAL, which withholds the
    // final sweep (see [`may_reap`]) — "we never saw it" must never be mistaken for "the user deleted it".
    // The walk hands back a count so the same fact also reaches the user on `Finished`, instead of only
    // silently deferring a reap they were never told about.
    let (files, unreadable) = walk_vault_markdown(&vault)?;
    let complete = unreadable == 0;
    on_event.send(IngestEvent::Counted {
        total: files.len() + extra_total,
    });
    let (mut ingested, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    for file in files {
        let VaultFile { rel: name, path } = file;
        // `name` is the vault-ROOT-relative path (`chats/chat-….md` for a chat), which is exactly what
        // `documents.vault_path` stores — so the resume stamp below and the sweep further down agree
        // with the row they are keying on. A bare file name would silently rebuild every chat from
        // scratch on every resumed pass.
        // The resume check, taken BEFORE any read/split/embed — this is where an interrupted run's saved
        // work is actually banked, so it must gate the expensive part, not merely the write.
        let already = {
            let conn = state.conn()?;
            stored_pass(&conn, &name)?.is_some_and(|stored| {
                plan_rebuild_one(stored.as_deref(), pass) == RebuildPlan::AlreadyDone
            })
        };
        // `Started` first even when we're about to skip: the views render a file's terminal event by
        // amending the row `Started` opened (`replaceLastWorking`), so a bare `Skipped` would show up as
        // a nameless row. Every other ingest path announces then completes; this one must too.
        on_event.send(IngestEvent::Started {
            path: path.to_string_lossy().into(),
            name: name.clone(),
        });
        if already {
            skipped += 1;
            on_event.send(IngestEvent::Skipped {
                path: path.to_string_lossy().into(),
                reason: "already rebuilt by the run that was interrupted".into(),
            });
            continue;
        }
        // A chat session's `.md` must round-trip its chat IDENTITY (source_type/source_id, per-chunk
        // turn pointer + timestamp, and the session→document link), not re-index as a plain document —
        // otherwise citations lose their jump-to-turn and a later idle sweep births a duplicate. Route it
        // through the live chat engine, which reads the (un-wiped) messages table and rebuilds all of that.
        let outcome = if is_chat_vault_file(&cipher, &path) {
            rebuild_chat(&state, &cipher, &path, pass)
        } else {
            rebuild_one(&state, &gateway, &cipher, &path, &name, pass).map(Some)
        };
        match outcome {
            Ok(Some(document)) => {
                ingested += 1;
                on_event.send(IngestEvent::Done {
                    document,
                    warning: None,
                });
            }
            // A chat that only ever exchanged small talk indexes no substantive turns, so it (correctly)
            // births no document — count it done, but there is nothing to surface.
            Ok(None) => {
                ingested += 1;
            }
            Err(e) => {
                failed += 1;
                on_event.send(IngestEvent::Failed {
                    path: path.to_string_lossy().into(),
                    error: e.to_string(),
                });
            }
        }
    }

    // THE SWEEP — the "delete only the stragglers, at the end" half of #371, and the only destructive
    // step left on the incremental arm. It reaps the documents whose vault file the user deleted, which
    // is the one useful thing the old wipe did implicitly. Two independent gates, because this deletes
    // real user documents:
    //   1. [`may_reap`] — run at all only on a provably complete walk. A partial one withholds the sweep
    //      entirely and the next complete pass reaps instead.
    //   2. [`plan_reap`] — per candidate, a FRESH three-way check ([`FileState`]): delete only on a
    //      PROVABLE absence, never on "couldn't tell".
    // Index-only documents are excluded: their `vault_path` is the synthetic `idx://<source_id>` sentinel
    // that no file will ever back, and the manifest — not the vault walk — is their source of truth.
    if may_reap(complete) {
        let candidates: Vec<(i64, String)> = {
            let conn = state.conn()?;
            let mut stmt =
                conn.prepare("SELECT id, vault_path FROM documents WHERE source_type != ?1")?;
            let rows = stmt
                .query_map(params![SOURCE_TYPE_INDEX_ONLY], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        for (doc_id, doc_vault_path) in candidates {
            if plan_reap(FileState::of(&vault.join(&doc_vault_path))) == ReapPlan::Delete {
                let mut conn = state.conn()?;
                let tx = conn.transaction()?;
                delete_document(&tx, doc_id)?;
                tx.commit()?;
            }
        }
    }

    // The retrieval stamp is deliberately NOT written here. It may only be written once BOTH phases have
    // finished cleanly, and phase 2 (the index-only full-body re-fetch) runs after this returns — so the
    // caller owns it. See `commands::rebuild_passes`.

    // Rebuild re-resolved every document's entity from its frontmatter canonical; push any
    // resulting entity change out to the portable rules file (a no-op when nothing changed).
    state.sync_entity_rules();

    // Index-only documents have no Markdown file, so the walk above skipped them; restore them from
    // the encrypted manifest, re-embedded from their stored summaries (their bodies are remote). The
    // gateway is already warmed and `chunk_vec` already sized, so this reuses both. A WHOLESALE failure
    // here — the manifest can't be opened (a poisoned vault lock), or `rebuild_from_manifest` errors
    // before it can count per-item (an undecryptable/corrupt manifest) — would otherwise vanish EVERY
    // connector-indexed document from a Rebuild that still reports success, with only a stderr line
    // (B3-7). Surface it as one synthetic Failed item and count it, so the run reports non-clean and the
    // Documents view shows why. A vault with no manifest yet resolves to `Ok((0, 0))`, so this never
    // fires on the common connector-free rebuild; per-item failures are already folded into `failed`.
    let manifest_rebuild = state
        .manifest_io()
        .and_then(|(vault_root, manifest_cipher)| {
            crate::index_only::rebuild_from_manifest(
                &state,
                &gateway,
                &vault_root,
                &manifest_cipher,
                on_event,
            )
        });
    match manifest_rebuild {
        Ok((restored, idx_failed)) => {
            ingested += restored;
            failed += idx_failed;
        }
        Err(e) => {
            failed += 1;
            on_event.send(IngestEvent::Started {
                path: "connector-index".into(),
                name: "Connector index".into(),
            });
            on_event.send(IngestEvent::Failed {
                path: "connector-index".into(),
                error: format!("couldn't restore connector-indexed items: {e}"),
            });
        }
    }

    // The terminal `Finished` is sent by the caller (`commands::rebuild_index`) AFTER it has run the
    // async full-body re-index of index-only items; hand back the counts so it can fold them in.
    Ok((ingested, skipped, failed, unreadable))
}

fn rebuild_one(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    cipher: &MarkdownCipher,
    vault_file: &Path,
    // The file's path relative to the vault root, `/`-separated — what `documents.vault_path`
    // stores. Passed in rather than re-derived from `vault_file`, so a `.md` that is not a chat but
    // sits in a Markdown subfolder is filed under the path the sweep will look for it at.
    vault_rel: &str,
    pass: &str,
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
    let mut photo = photo_from_fields(&fields, &content_hash, body);
    let spreadsheet = spreadsheet_from_fields(&fields);

    if let (Some(p), Some(vault_dir)) = (photo.as_mut(), vault_file.parent()) {
        heal_photo_copy(
            p,
            vault_dir,
            cipher,
            &file_name(vault_file),
            fields.get("ext").map(String::as_str),
        )?;
    }
    let photo = photo;

    // Organisation metadata round-trips from the vault so a rebuild reproduces
    // the organised store (spec §3 acceptance). Missing fields fall back to the
    // fresh-ingest defaults, so pre-Step-4 vault files rebuild cleanly.
    let project = fields
        .get("project")
        .cloned()
        .unwrap_or_else(|| "Unsorted".into());
    let meta = DocMeta {
        source_path: fields.get("source_path").cloned(),
        vault_path: vault_rel.to_string(),
        title,
        content_hash,
        ext: fields.get("ext").cloned(),
        byte_size: None,
        created_at: fields.get("created_at").cloned(),
        linked_projects: linked_projects_from_fields(&fields, &project),
        project,
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
            // A promoted spreadsheet round-trips its connector claim: restore the `source_id` (so the
            // next sync recognises the still-present source as already-imported instead of re-indexing a
            // duplicate) + its `external_ref` (the Drive link). A locally-ingested spreadsheet has
            // neither front-matter line, so both stay `None` — indistinguishable from before.
            SourceMeta {
                source_id: fields.get("source_id").cloned(),
                external_ref: fields.get("external_ref").cloned(),
                ..SourceMeta::spreadsheet()
            }
        } else {
            // A plain vault document may still carry a source claim — a pinboard note keyed
            // `note:<widget_id>` (so the board re-links and a re-ingest updates in place instead of
            // duplicating), or any future locally-stored source. Round-trip it. A hand-imported local
            // file has no `source_id` line, so this stays a bare vault document exactly as before.
            SourceMeta {
                source_id: fields.get("source_id").cloned(),
                external_ref: fields.get("external_ref").cloned(),
                ..SourceMeta::default()
            }
        },
    };
    upsert_document(
        state,
        &meta,
        &chunks,
        &embeddings,
        photo.as_ref(),
        spreadsheet.as_ref(),
        pass,
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

/// Rebuild a chat session from its vault `.md`. `conversations` / `messages` / `chat_sessions` are never
/// touched by a rebuild — the authored turns are still the source of truth. For a chat that already has a
/// `documents` row we clear its chunk index IN PLACE ([`clear_document_chunks`]) but KEEP the row and its
/// id, reset only the index cursor to NULL, and re-run the SAME engine the live/idle indexer uses
/// ([`chat_index::index_session`], in `rebuilding` mode): it re-appends every completed turn-pair onto the
/// PRESERVED id (its `(Some(id), false)` branch), stamping per-chunk `chat_turn_id` + `chunk_at`. A chat
/// with no row yet is born fresh, exactly as the live indexer would. Returns the [`Document`], or `None`
/// for a chat with no substantive turns (small-talk-only ⇒ no document, by design).
///
/// Keeping the id is what dissolves the dangling-citation class for chats — the same fix #374 made for
/// vault documents. Because the `documents` row is never deleted, `corrections.document_id` is no longer
/// NULLed on every rebuild and jump-to-turn citations keep resolving. The append-only cursor still resets,
/// so every turn re-embeds from scratch; the birth path (`document_id` NULL) is untouched. A chat whose
/// preserved row re-indexes to nothing substantive (e.g. a splitter change now trims every turn) is reaped
/// so we never leave a 0-chunk ghost on the kept id.
fn rebuild_chat(
    state: &AppState,
    cipher: &MarkdownCipher,
    vault_file: &Path,
    pass: &str,
) -> Result<Option<Document>> {
    let raw = cipher.read(vault_file)?;
    let (fields, _body) = parse_frontmatter(&raw)
        .ok_or_else(|| Error::Other("chat vault file missing front-matter".into()))?;
    let conversation_id: i64 = fields
        .get("chat_conversation_id")
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| Error::Other("chat vault file missing chat_conversation_id".into()))?;

    {
        let mut conn = state.conn()?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT document_id FROM chat_sessions WHERE conversation_id = ?1",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let tx = conn.transaction()?;
        match existing {
            // Keep the row and its id: clear only its chunk index IN PLACE and reset the cursor, so
            // `index_session` re-appends onto the SAME id instead of re-birthing. That is what keeps chat
            // citations + `corrections.document_id` anchored across a Rebuild (#374 for chats). The row
            // still owns this chat's `vault_path` (NOT NULL UNIQUE), so nothing collides.
            Some(doc_id) => {
                clear_document_chunks(&tx, doc_id)?;
                tx.execute(
                    "UPDATE chat_sessions SET last_indexed_turn_id = NULL WHERE conversation_id = ?1",
                    params![conversation_id],
                )?;
            }
            // Never indexed (no substance yet): nothing to clear; reset the cursor so the engine re-reads
            // from the start and births the row if this pass finds substance.
            None => {
                tx.execute(
                    "UPDATE chat_sessions SET document_id = NULL, last_indexed_turn_id = NULL \
                     WHERE conversation_id = ?1",
                    params![conversation_id],
                )?;
            }
        }
        tx.commit()?;
    }
    // `rebuilding = true`: reuse the preserved id and skip the card-F append re-evaluation (the authored
    // classification is restored from the file just below).
    crate::chat_index::index_session(state, conversation_id, true)?;

    // The session row exists (its vault file implies an earlier `record_turn_pair` upsert). document_id is
    // NULL again only if index_session found nothing substantive to index (small-talk-only chat).
    let mut conn = state.conn()?;
    // `.optional()` because the session row may not exist at all: a chat deletion drops `conversations`
    // (cascading `chat_sessions`) and then removes the vault file as a separate, non-transactional step,
    // so a file left behind by a failed/locked delete is a reachable orphan. Without this the bare
    // `query_row` returns QueryReturnedNoRows, the `?` fails this file, and — since the sweep only runs on
    // a clean pass — that one orphan would withhold the straggler sweep on every rebuild, forever.
    // There is no document to rebuild, so report it as such and move on.
    let doc_id: Option<i64> = conn
        .query_row(
            "SELECT document_id FROM chat_sessions WHERE conversation_id = ?1",
            params![conversation_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(id) = doc_id else {
        return Ok(None);
    };
    // Reap a chat whose preserved row re-indexed to nothing substantive (e.g. a splitter change now trims
    // every turn as trivial): honour the small-talk-only contract (no document) rather than leave a
    // 0-chunk ghost on the kept id.
    let chunk_count: i64 = conn.query_row(
        "SELECT count(*) FROM chunks WHERE document_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    if chunk_count == 0 {
        let tx = conn.transaction()?;
        delete_document(&tx, id)?;
        tx.execute(
            "UPDATE chat_sessions SET document_id = NULL WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        tx.commit()?;
        return Ok(None);
    }

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
    // A chat is a document like any other, so its extra project memberships restore from its own
    // front-matter here rather than only for the `rebuild_one` path.
    crate::tags::set_document_projects(
        &conn,
        id,
        &project,
        &linked_projects_from_fields(&fields, &project),
    )?;
    crate::tags::set_document_group_tags(&conn, id, &tags)?;
    // Claim it for this pass, so a resume skips this chat instead of re-indexing every turn again — and so
    // the sweep reads it as rebuilt rather than as a leftover.
    stamp_rebuild_pass(&conn, id, pass)?;
    Ok(Some(load_document(&conn, id)?))
}

/// Insert a document and its chunks/vectors/FTS rows in one transaction. `embeddings` are the
/// leaf embeddings in leaf order (parents are structural-only and carry none); the loop pairs
/// them with leaves as it walks the chunk list. The splitter emits parents before their
/// children, so a single ordered pass resolves `parent_uid` → row id from a uid map.
/// Index a document whose vault file THIS ingest just wrote, removing that file if indexing fails.
///
/// The vault is the source of truth a Rebuild reads back, so a vault file with no DB row is not an
/// inert orphan — it is a document that RESURRECTS on the next Rebuild, carrying whatever made us
/// reject it, filed as Unsorted with no trace of the failure. The note-ingest and spreadsheet-promote
/// paths always rolled back; the three FRESH ingest paths (document, photo, spreadsheet) wrote the
/// file and then propagated the index error over it. Rolling back at each call site is what let them
/// drift apart, so the rollback lives with the write instead.
///
/// Only for a file this ingest CREATED: it deletes unconditionally on failure, with no prior bytes
/// to restore. Never call it for a re-ingest over an existing vault file.
fn index_fresh_document(
    state: &AppState,
    vault_file: &Path,
    meta: &DocMeta,
    chunks: &[splitter::Chunk],
    embeddings: &[Vec<f32>],
    photo: Option<&PhotoRecord>,
    spreadsheet: Option<&SpreadsheetRecord>,
) -> Result<Document> {
    index_document(state, meta, chunks, embeddings, photo, spreadsheet).inspect_err(|_| {
        // Best-effort: if the file is locked we surface the INDEX error, which is the one the user
        // can act on. A stranded file costs a spurious doc at the next Rebuild, not data.
        let _ = std::fs::remove_file(vault_file);
    })
}

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
    insert_satellites(&tx, doc_id, photo, spreadsheet)?;

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

/// Write a document's satellite rows — the photo's capture/OCR/copy truth, or a spreadsheet's sheet/row
/// counts + truncation record — in the SAME transaction as the document itself, so a row and its
/// satellite are never inconsistent. `photos.visual_description` and `spreadsheets.structured_data_summary`
/// are left to their NULL defaults (reserved for later enrichment; no writer this stage).
///
/// Extracted so the rebuild's in-place [`upsert_document`] writes them identically to a fresh
/// [`index_document`] — the satellite shape must not drift between the two paths.
fn insert_satellites(
    tx: &Connection,
    doc_id: i64,
    photo: Option<&PhotoRecord>,
    spreadsheet: Option<&SpreadsheetRecord>,
) -> Result<()> {
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
    if let Some(sp) = spreadsheet {
        tx.execute(
            "INSERT INTO spreadsheets (document_id, sheet_count, total_rows, chunked_rows) \
             VALUES (?1, ?2, ?3, ?4)",
            params![doc_id, sp.sheet_count, sp.total_rows, sp.chunked_rows],
        )?;
    }
    Ok(())
}

/// Write one document a REBUILD has just re-split: update the row the vault already owns for this
/// `vault_path`, insert one when it doesn't exist, and stamp it with this run's `pass` either way — all in
/// ONE transaction, so the checkpoint can never claim work that didn't commit.
///
/// The in-place half is what makes #371's resume possible, and it keeps `documents.id` STABLE across a
/// Rebuild, which the old drop-and-recreate never did. Everything keyed to that id now survives:
/// `corrections.document_id` (declared `ON DELETE SET NULL`, so every rebuild silently orphaned the whole
/// Learning-You correction corpus from its documents), the `messages.citations` blobs behind chat
/// citations, and the reader's saved document links.
fn upsert_document(
    state: &AppState,
    meta: &DocMeta,
    chunks: &[splitter::Chunk],
    embeddings: &[Vec<f32>],
    photo: Option<&PhotoRecord>,
    spreadsheet: Option<&SpreadsheetRecord>,
    pass: &str,
) -> Result<Document> {
    let mut conn = state.conn()?;
    let tx = conn.transaction()?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM documents WHERE vault_path = ?1",
            params![meta.vault_path],
            |r| r.get(0),
        )
        .optional()?;
    let doc_id = match existing {
        Some(doc_id) => {
            update_document_row(&tx, doc_id, meta)?;
            // The satellites are keyed by `document_id`, and `photos.file_hash` is UNIQUE — so a bare
            // re-insert would collide with the row this very document already owns. The wipe is what used
            // to make that impossible; clear them first, then re-write from the front-matter truth.
            tx.execute("DELETE FROM photos WHERE document_id = ?1", params![doc_id])?;
            tx.execute(
                "DELETE FROM spreadsheets WHERE document_id = ?1",
                params![doc_id],
            )?;
            insert_satellites(&tx, doc_id, photo, spreadsheet)?;
            replace_chunks(
                &tx,
                doc_id,
                chunks,
                embeddings,
                meta.source.is_index_only(),
                meta.source.stored_summary.as_deref(),
            )?;
            doc_id
        }
        None => {
            let doc_id = insert_document_row(&tx, meta)?;
            insert_satellites(&tx, doc_id, photo, spreadsheet)?;
            insert_chunks(
                &tx,
                doc_id,
                chunks,
                embeddings,
                meta.source.is_index_only(),
                meta.source.stored_summary.as_deref(),
            )?;
            doc_id
        }
    };
    stamp_rebuild_pass(&tx, doc_id, pass)?;
    tx.commit()?;
    load_document(&conn, doc_id)
}

/// Record that `pass` rebuilt this document (v35). Always called inside the document's own transaction,
/// so the stamp and the chunks it vouches for commit together — a checkpoint that could outlive a
/// rolled-back write would make resume skip work that never landed.
pub(crate) fn stamp_rebuild_pass(tx: &Connection, doc_id: i64, pass: &str) -> Result<()> {
    tx.execute(
        "UPDATE documents SET rebuild_pass = ?1 WHERE id = ?2",
        params![pass, doc_id],
    )?;
    Ok(())
}

/// The UPDATE half of [`insert_document_row`] — the same columns, resolved the same way, for a document
/// the vault already holds. Kept beside the INSERT so the two can't drift. `vault_path` is the key this
/// matched on, so it is never rewritten.
///
/// `byte_size` and the four source facts are deliberately COALESCEd rather than assigned. They are
/// learned from something the vault front-matter does not carry — `byte_size` is measured at ingest
/// from the ORIGINAL file, and the source facts are what the provider said about it — so
/// `rebuild_one` always passes `None` for them and a plain assignment nulls them on every pass. It
/// did exactly that to `byte_size` on every rebuild until this, and the #701 columns shipped with
/// the same bug because they did not inherit the fix (#708).
fn update_document_row(tx: &Connection, doc_id: i64, meta: &DocMeta) -> Result<()> {
    let tags_json =
        serde_json::to_string(&meta.tags).map_err(|e| Error::Other(format!("encode tags: {e}")))?;
    let entity_id = crate::entities::resolve_project(tx, &meta.project, true)?;
    let source_account = meta
        .source
        .source_id
        .as_deref()
        .and_then(crate::drive::account_of);
    tx.execute(
        "UPDATE documents SET \
         source_path = ?1, title = ?2, content_hash = ?3, ext = ?4, \
         byte_size = COALESCE(?5, byte_size), created_at = ?6, ingested_at = ?7, project = ?8, \
         tags = ?9, importance = ?10, reviewed = ?11, last_activity = ?12, entity_id = ?13, \
         source_type = ?14, source_state = ?15, source_id = ?16, external_ref = ?17, \
         source_modified_at = ?18, source_content_hash = ?19, stored_summary = ?20, \
         source_parent_folder_id = ?21, source_parent_folder_name = ?22, source_account = ?23, \
         source_author = COALESCE(?24, source_author), \
         source_last_modified_by = COALESCE(?25, source_last_modified_by), \
         source_created_at = COALESCE(?26, source_created_at), \
         source_size_bytes = COALESCE(?27, source_size_bytes) \
         WHERE id = ?28",
        params![
            meta.source_path,
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
            meta.source.source_author,
            meta.source.source_last_modified_by,
            meta.source.source_created_at,
            meta.source.source_size_bytes,
            doc_id,
        ],
    )?;
    // The row and the membership join move together: this is the rebuild-from-vault path, so the
    // file's `linked_projects:` line is the truth being restored, not an edit being applied.
    crate::tags::set_document_projects(tx, doc_id, &meta.project, &meta.linked_projects)?;
    crate::tags::set_document_group_tags(tx, doc_id, &meta.tags)?;
    Ok(())
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
          source_parent_folder_id, source_parent_folder_name, source_account, \
          source_author, source_last_modified_by, source_created_at, source_size_bytes) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                 ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
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
            meta.source.source_author,
            meta.source.source_last_modified_by,
            meta.source.source_created_at,
            meta.source.source_size_bytes,
        ],
    )?;
    let doc_id = tx.last_insert_rowid();
    crate::tags::set_document_projects(tx, doc_id, &meta.project, &meta.linked_projects)?;
    crate::tags::set_document_group_tags(tx, doc_id, &meta.tags)?;
    Ok(doc_id)
}

/// Insert one leaf's keyword-search row into `chunks_fts`. On a multilingual vault the text is run
/// through [`crate::fts_segment::fts_tokens`] and re-joined with spaces, so the default `unicode61`
/// tokenizer sees each CJK bigram (and each Latin word) as its own token — without which a
/// space-less CJK run collapses to one unmatchable token and keyword search silently dies (F-33).
/// The English/default path (`multilingual == false`) inserts the text verbatim, so those vaults'
/// FTS rows stay byte-for-byte identical. Must stay in lockstep with the query-side segmentation in
/// `retrieval::fts_query`: both go through the one shared `fts_segment` helper so a query bigram
/// always equals an indexed token.
fn insert_fts_row(tx: &Connection, rowid: i64, text: &str, multilingual: bool) -> Result<()> {
    let content: std::borrow::Cow<'_, str> = if multilingual {
        crate::fts_segment::fts_tokens(text).join(" ").into()
    } else {
        text.into()
    };
    // Cached: this runs once per leaf inside the ingest loops, so don't re-parse the SQL per row.
    tx.prepare_cached("INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)")?
        .execute(params![rowid, content.as_ref()])?;
    Ok(())
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
    // CJK-segment the FTS content only on multilingual vaults (F-33); resolved once per call — a
    // single sync settings read on this same connection, no lock held across an await.
    let multilingual = crate::db::selected_embedder(tx)?.multilingual;
    let mut uid_to_id: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut leaf_idx = 0usize;
    let mut ordinal = start_ordinal;
    // Cached statements: prepared once per connection, re-bound per chunk (not re-parsed per row).
    let mut insert_chunk = tx.prepare_cached(
        "INSERT INTO chunks \
         (document_id, ordinal, heading, content, char_count, uid, parent_id, kind, \
          start_offset, end_offset, chat_turn_id, chunk_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    let mut insert_vec =
        tx.prepare_cached("INSERT INTO chunk_vec (rowid, embedding) VALUES (?1, ?2)")?;
    for chunk in chunks {
        let parent_id = chunk
            .parent_uid
            .as_deref()
            .and_then(|uid| uid_to_id.get(uid).copied());
        insert_chunk.execute(params![
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
        ])?;
        let chunk_id = tx.last_insert_rowid();
        uid_to_id.insert(&chunk.uid, chunk_id);
        ordinal += 1;

        if chunk.kind == ChunkKind::Leaf {
            let vector = embedding_blob(&embeddings[leaf_idx]);
            insert_vec.execute(params![chunk_id, vector])?;
            insert_fts_row(tx, chunk_id, &chunk.embed_content, multilingual)?;
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
    // CJK-segment the FTS content only on multilingual vaults (F-33); resolved once per document.
    let multilingual = crate::db::selected_embedder(tx)?.multilingual;
    let mut uid_to_id: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut leaf_idx = 0usize;
    let mut first_leaf_id: Option<i64> = None;
    // Cached statements: prepared once per connection, re-bound per chunk (not re-parsed per row).
    let mut insert_chunk = tx.prepare_cached(
        "INSERT INTO chunks \
         (document_id, ordinal, heading, content, char_count, uid, parent_id, kind, start_offset, end_offset) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    let mut insert_vec =
        tx.prepare_cached("INSERT INTO chunk_vec (rowid, embedding) VALUES (?1, ?2)")?;
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
        insert_chunk.execute(params![
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
        ])?;
        let chunk_id = tx.last_insert_rowid();
        uid_to_id.insert(&chunk.uid, chunk_id);

        if chunk.kind == ChunkKind::Leaf {
            let vector = embedding_blob(&embeddings[leaf_idx]);
            insert_vec.execute(params![chunk_id, vector])?;
            // Vault docs index the heading-prepended text so keyword search benefits from the
            // breadcrumb, while `chunks.content` stays clean for display + citations. Index-only
            // docs index nothing here — keeping the body out of the index — and become
            // keyword-findable by their summary, attached to the first leaf rowid below.
            if !index_only {
                insert_fts_row(tx, chunk_id, &chunk.embed_content, multilingual)?;
            }
            first_leaf_id.get_or_insert(chunk_id);
            leaf_idx += 1;
        }
    }

    if index_only {
        if let (Some(rowid), Some(summary)) = (first_leaf_id, stored_summary) {
            if !summary.trim().is_empty() {
                insert_fts_row(tx, rowid, summary, multilingual)?;
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

/// Clear a document's chunk index — its `chunk_vec` + `chunks_fts` mirror rows and its `chunks` — but
/// WITHOUT deleting the `documents` row, so the row and its id survive (with its citations and its
/// `corrections.document_id` FK). Order matters: the rowid-keyed mirrors (`chunk_vec` vec0, `chunks_fts`
/// FTS5) are NOT FK targets, so they must be deleted while the `chunks` they key off still exist —
/// exactly as [`replace_chunks`] does. Caller owns the transaction. [`rebuild_chat`] uses this to
/// re-index a chat onto its stable id.
pub(crate) fn clear_document_chunks(tx: &Connection, doc_id: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM chunk_vec WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
        params![doc_id],
    )?;
    tx.execute(
        "DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
        params![doc_id],
    )?;
    tx.execute("DELETE FROM chunks WHERE document_id = ?1", params![doc_id])?;
    Ok(())
}

/// Purge one document entirely: [`clear_document_chunks`] plus the `documents` row itself. This is the
/// exact cascade a "delete document" uses (card 7G deletes a chat through it); it also matches the global
/// teardown order at the top of [`rebuild`]. Deleting `documents` is explicit: `chunks.document_id`
/// cascades in the *delete-documents* direction, but we delete bottom-up. Caller owns the transaction.
pub(crate) fn delete_document(tx: &Connection, doc_id: i64) -> Result<()> {
    clear_document_chunks(tx, doc_id)?;
    // Snapshot the registry rows this document holds BEFORE it goes: `document_tags` cascades off
    // `documents`, so afterwards nothing can say which tags just lost a member. Filing prunes the
    // tags each write orphans (`tags::set_document_projects`), and a DELETE is the other way a tag
    // loses its last document — the one the filing writer can never see. Left unpruned a label
    // lingers in every picker AND in the cached filing prompt, where the model reads it as
    // established vocabulary and re-mints it onto new documents. Inside the caller's transaction, so
    // the orphaning and the prune commit or roll back together.
    let held = crate::tags::document_tag_ids(tx, doc_id)?;
    tx.execute("DELETE FROM documents WHERE id = ?1", params![doc_id])?;
    crate::tags::prune_orphan_tags_by_id(tx, &held)?;
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
     d.source_type, d.source_state, d.external_ref, d.source_id, \
     d.source_parent_folder_id, d.source_parent_folder_name,      d.source_author, d.source_last_modified_by, d.source_created_at, d.source_size_bytes, \
     d.source_modified_at, COALESCE(d.pm_refreshed_at, d.ingested_at)";

/// Fill in each document's extra project memberships from the join, in ONE query for the whole
/// list. A lookup per document would be an N+1 across the entire library — which is exactly the
/// size this list grows to.
fn attach_memberships(conn: &Connection, docs: &mut [Document]) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let memberships = crate::tags::all_project_memberships(conn)?;
    for doc in docs.iter_mut() {
        let home = crate::tags::normalize(&doc.project);
        doc.linked_projects = memberships
            .get(&doc.id)
            .map(|names| {
                names
                    .iter()
                    .filter(|n| crate::tags::normalize(n) != home)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
    }
    Ok(())
}

/// All documents, most-recent first, with their chunk counts.
pub fn list_documents(conn: &Connection) -> Result<Vec<Document>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d ORDER BY d.ingested_at DESC, d.id DESC"
    ))?;
    let mut rows = stmt
        .query_map([], row_to_document)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    attach_memberships(conn, &mut rows)?;
    Ok(rows)
}

/// Documents still awaiting the sorting review (`reviewed = 0`), newest first.
pub fn review_queue(conn: &Connection) -> Result<Vec<Document>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.reviewed = 0 \
         ORDER BY d.ingested_at DESC, d.id DESC"
    ))?;
    let mut rows = stmt
        .query_map([], row_to_document)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    attach_memberships(conn, &mut rows)?;
    Ok(rows)
}

/// Just the COUNT of documents awaiting review — for the sidebar badge, so the whole queue isn't
/// materialised (rows + columns) only to read its length on every view change (F-47).
pub fn review_queue_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM documents WHERE reviewed = 0",
        [],
        |r| r.get(0),
    )?)
}

pub fn load_document(conn: &Connection, id: i64) -> Result<Document> {
    let mut doc = conn.query_row(
        &format!("SELECT {DOCUMENT_COLUMNS} FROM documents d WHERE d.id = ?1"),
        params![id],
        row_to_document,
    )?;
    doc.linked_projects = crate::tags::linked_projects(conn, doc.id, &doc.project)?;
    Ok(doc)
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
        // Filled by the caller (see `attach_memberships`). This reader is positional, and inserting
        // a column here would shift every index after it — silently, because `tags` is parsed with
        // `unwrap_or_default()` and would simply come back empty.
        linked_projects: Vec::new(),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        importance: row.get(10)?,
        reviewed: reviewed != 0,
        last_activity: row.get(12)?,
        source_type: row.get(13)?,
        source_state: row.get(14)?,
        external_ref: row.get(15)?,
        source_id: row.get(16)?,
        source_parent_folder_id: row.get(17)?,
        source_parent_folder_name: row.get(18)?,
        source_author: row.get(19)?,
        source_last_modified_by: row.get(20)?,
        source_created_at: row.get(21)?,
        source_size_bytes: row.get(22)?,
        source_modified_at: row.get(23)?,
        pm_refreshed_at: row.get(24)?,
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

/// Whether filing a document should append a Stage-3 project-activity observation. A genuine user
/// organize edit `Record`s per-project engagement; bulk identity maintenance (an entity
/// rename/merge rewriting every linked doc — [`crate::commands`]) `Suppress`es it, because a
/// rename is not per-document engagement and would otherwise read as a burst of it (B6-6).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilingActivity {
    Record,
    Suppress,
}

/// Persist a document's organisational truth (canonical project + tags/importance/reviewed/
/// last_activity) to wherever its source keeps it, returning the file snapshot for rollback. The
/// single indirection point every metadata write goes through — so hard-coding front-matter can't
/// creep back in: a vault document rewrites its Markdown front-matter, an index-only document
/// rewrites the encrypted manifest (see [`TruthSource`]). `vault`/`cipher` reach the Markdown layer;
/// `vault_root`/`manifest_cipher` reach the manifest — a caller passes both and the dispatch picks.
/// `activity` decides whether filing into a real project logs engagement ([`FilingActivity`]).
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
    linked_projects: &[String],
    tags: &[String],
    importance: Option<&str>,
    reviewed: bool,
    last_activity: &str,
    vault_root: &Path,
    manifest_cipher: &crate::index_only::ManifestCipher,
    activity: FilingActivity,
) -> Result<(std::path::PathBuf, Vec<u8>)> {
    // The membership join is a queryable index over what the truth file is about to say, so it is
    // written here — inside the caller's transaction — and nowhere else. This is what INVARIANTS
    // I-02 ("one writer owns a document's filing") buys: a new filing surface gets correct
    // memberships by construction rather than by remembering to.
    //
    // BEFORE the dispatch, not after: the index-only arm regenerates the encrypted manifest from
    // the DB mirror, and that mirror reads this join. Written afterwards, the manifest would carry
    // the PREVIOUS membership set and only catch up at the next unrelated edit.
    crate::tags::set_document_projects(tx, doc_id, project, linked_projects)?;
    // Group tags ride the same seam (#276). `documents.tags` stays their truth — it is what the
    // vault's `tags:` line round-trips and what a Rebuild restores from — and this keeps the
    // queryable index over it from drifting, so `@tag` can scope by a label without a full scan.
    crate::tags::set_document_group_tags(tx, doc_id, tags)?;

    let written = match truth_source(tx, doc_id)? {
        TruthSource::VaultFrontmatter => rewrite_vault_metadata(
            tx,
            vault,
            cipher,
            doc_id,
            project,
            linked_projects,
            tags,
            importance,
            reviewed,
            last_activity,
        )?,
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
        )?,
    };

    // Stage-3 activity log: filing a document INTO a real project is per-project engagement, and this
    // seam is the single choke-point every user-initiated organize edit passes through (Rebuild and
    // chat-document birth insert their rows directly, bypassing it). "Unsorted" is the pre-triage
    // bucket, not a project the user chose, so it doesn't count; nor does a bulk identity rewrite
    // (`FilingActivity::Suppress` — B6-6). Best-effort, inside the caller's tx.
    if activity == FilingActivity::Record && project != "Unsorted" {
        crate::project_activity::record(
            tx,
            project,
            crate::project_activity::Kind::Ingest,
            Some(doc_id),
        );
    }

    Ok(written)
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

/// Record an opt-in saved original in **both** places that describe one: the photo's vault Markdown
/// (the truth a Rebuild reconstructs the row FROM) and its `photos` row (what the reader queries).
///
/// The two must move together, and this function exists so they cannot drift apart at a call site.
/// Flipping only the row is a promise the next Rebuild breaks: it re-reads
/// `photo_saved_to_vault: false` from the untouched front-matter, resets the flag, and orphans the
/// copy — in an encrypted vault, unreachable by every surface PM has, and the feature's whole pitch
/// is that the user may have deleted the original by then. That was the bug until v3.19.2.
///
/// Order matters: vault truth first, row second. A failure part-way then leaves "no copy recorded",
/// which is self-consistent and costs nothing (the bytes are hash-named, so re-saving rewrites the
/// same file), rather than a row promising a file nothing can find.
///
/// `conn` is the caller's guard: `state.conn()` is NOT reentrant.
fn record_saved_photo_copy(
    conn: &Connection,
    vault: &Path,
    cipher: &MarkdownCipher,
    file_hash: &str,
    copy_rel: &str,
) -> Result<()> {
    if let Some(md_rel) = photo_doc_vault_path(conn, file_hash)? {
        rewrite_photo_vault_block(vault, cipher, &md_rel, copy_rel)?;
    }
    conn.execute(
        "UPDATE photos SET saved_to_vault = 1, vault_path = ?1 WHERE file_hash = ?2",
        params![copy_rel, file_hash],
    )?;
    Ok(())
}

/// Heal a photo block that lost track of its saved copy, in place. Returns whether it healed.
///
/// Pre-v3.19.2 builds flipped only the `photos` row on a dedupe-hit save (see
/// [`record_saved_photo_copy`]), so a real user's vault can hold a block saying "no copy" with the
/// copy sitting right there in `photos/`. A rebuild reconstructs the row from the block, which is
/// what turns that divergence into actual loss — so heal at exactly that moment: believe the disk,
/// and write the correction back so the heal is durable rather than re-derived on every rebuild.
///
/// Only ever adds a copy that is provably present, and never second-guesses a block that already
/// knows its own state — so it cannot invent a `vault_path` or overrule a deliberate `false`.
fn heal_photo_copy(
    photo: &mut PhotoRecord,
    vault: &Path,
    cipher: &MarkdownCipher,
    md_name: &str,
    ext: Option<&str>,
) -> Result<bool> {
    if photo.saved_to_vault {
        return Ok(false);
    }
    let Some(rel) = find_saved_photo_copy(vault, &photo.file_hash, ext) else {
        return Ok(false);
    };
    photo.saved_to_vault = true;
    photo.vault_path = Some(rel.clone());
    rewrite_photo_vault_block(vault, cipher, md_name, &rel)?;
    Ok(true)
}

/// The vault Markdown path of the photo document whose original hashes to `file_hash`, if there is
/// one. Takes the caller's connection: `state.conn()` is a non-reentrant mutex, so re-taking it
/// under a held guard self-deadlocks the whole app.
///
/// `None` covers the honest case where the hash matched a NON-photo document (a dedupe hit against
/// an ordinary file with identical bytes), which has no photo block to record anything in.
fn photo_doc_vault_path(conn: &Connection, file_hash: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT d.vault_path FROM documents d JOIN photos p ON p.document_id = d.id \
             WHERE p.file_hash = ?1",
            params![file_hash],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Record an opt-in saved original in the photo's own vault Markdown — the truth a Rebuild
/// reconstructs its `photos` row from. Preserves every other front-matter field and the body,
/// mirroring [`rewrite_vault_metadata`]'s field-preserving construction (this is the photo-block
/// analogue: that one rewrites organisation metadata and preserves the photo block; this one
/// rewrites the photo block and preserves the organisation metadata).
///
/// A file whose front-matter says it isn't a photo is left untouched rather than treated as an
/// error — see [`photo_doc_vault_path`] for how a non-photo can be reached at all.
fn rewrite_photo_vault_block(
    vault: &Path,
    cipher: &MarkdownCipher,
    md_rel: &str,
    copy_rel: &str,
) -> Result<()> {
    let file = vault.join(md_rel);
    let decoded = cipher.read(&file)?;
    let (fields, body) = parse_frontmatter(&decoded)
        .ok_or_else(|| Error::Other("vault file missing front-matter".into()))?;

    let content_hash = fields.get("content_hash").map(String::as_str).unwrap_or("");
    let Some(mut photo) = photo_from_fields(&fields, content_hash, body) else {
        return Ok(());
    };
    photo.saved_to_vault = true;
    photo.vault_path = Some(copy_rel.to_string());

    let tags = fields
        .get("tags")
        .map(|s| parse_yaml_list(s))
        .unwrap_or_default();
    let project = fields
        .get("project")
        .map(String::as_str)
        .unwrap_or("Unsorted");
    let linked = linked_projects_from_fields(&fields, project);
    let importance = nullable(fields.get("importance"));
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
        linked_projects: &linked,
        tags: &tags,
        importance: importance.as_deref(),
        last_activity: fields
            .get("last_activity")
            .map(String::as_str)
            .unwrap_or(""),
        reviewed: fields
            .get("reviewed")
            .map(|v| v.trim() == "true")
            .unwrap_or(false),
        photo: Some(&photo),
        spreadsheet: None, // mutually exclusive with `photo`, and this file is a photo
        chat: None,
        source_id: fields.get("source_id").map(String::as_str),
        external_ref: fields.get("external_ref").map(String::as_str),
    };
    cipher.write_to(&file, &render_markdown(&front, body))?;
    Ok(())
}

/// A chat vault file's identity lines, carried across an organisation edit.
///
/// These four fields ARE the chat: `is_chat_vault_file` routes a Rebuild on `source_type`, and
/// `rebuild_chat` reads `chat_conversation_id` to find the session. Losing them doesn't fail loudly —
/// the file stops matching the chat predicate, so the Rebuild quietly re-ingests a conversation as an
/// ordinary document, NULLing every chunk's turn pointer and indexing PM's own answers as if they
/// were source material.
pub(crate) struct ChatIdentity {
    pub conversation_id: String,
    pub scope: String,
    pub source_id: String,
}

/// Reconstruct a chat's identity block from parsed front-matter fields, or `None` if this isn't a
/// chat document — the same shape as [`photo_from_fields`] / [`spreadsheet_from_fields`], and for the
/// same reason: every writer that rebuilds a vault file from a `Frontmatter` must round-trip the
/// source-type block rather than dropping it.
pub(crate) fn chat_from_fields(
    fields: &std::collections::HashMap<String, String>,
) -> Option<ChatIdentity> {
    if fields.get("source_type").map(String::as_str) != Some(SOURCE_TYPE_CHAT) {
        return None;
    }
    // `chat_conversation_id` is the load-bearing one (rebuild_chat errors without it); scope and
    // source_id are recorded as-is. A file missing them is already damaged, so preserve what is there
    // rather than inventing values — the on-open heal (`chat::reconcile_vault_identity`) is what
    // repairs a file that has lost them, from `chat_sessions`.
    Some(ChatIdentity {
        conversation_id: fields.get("chat_conversation_id")?.trim().to_string(),
        scope: fields
            .get("chat_scope")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "general".into()),
        source_id: fields
            .get("chat_source_id")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
    })
}

/// Read a document's ADDITIONAL project memberships back out of parsed front-matter (#275).
///
/// The same shape as [`photo_from_fields`] / [`chat_from_fields`], and called from the same three
/// places for the same reason: every writer that rebuilds a vault file from a `Frontmatter` must
/// round-trip this or the next unrelated organisation write silently deletes it.
///
/// Tolerant by design, because a vault file is the user's to edit. A missing key (every file
/// written before this shipped) is "no extras". The `home` is filtered out case-insensitively, so
/// someone who hand-writes the home project into the list too gets what they meant rather than a
/// document linked to itself.
pub(crate) fn linked_projects_from_fields(
    fields: &std::collections::HashMap<String, String>,
    home: &str,
) -> Vec<String> {
    let home_norm = crate::tags::normalize(home);
    fields
        .get("linked_projects")
        .map(|s| parse_yaml_list(s))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| crate::tags::normalize(p) != home_norm)
        .collect()
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
    linked_projects: &[String],
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

    // Preserve a photo's, spreadsheet's or chat's block across an organisation edit, so a later
    // Rebuild still routes the file correctly and reconstructs its satellite row (a plain document has
    // none of the three → `None`, unchanged behaviour).
    //
    // The chat arm is not symmetry for its own sake. Every organisation write funnels through here —
    // approving a chat in Review, editing its project, or renaming/merging the project that owns it —
    // and dropping the block demoted the conversation to an ordinary document at the next Rebuild,
    // silently. Anything added to a source-type block from here on must be round-tripped here too.
    let content_hash = fields.get("content_hash").map(String::as_str).unwrap_or("");
    let photo = photo_from_fields(&fields, content_hash, body);
    let spreadsheet = spreadsheet_from_fields(&fields);
    let chat = chat_from_fields(&fields);
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
        linked_projects,
        tags,
        importance,
        last_activity,
        reviewed,
        photo: photo.as_ref(),
        spreadsheet: spreadsheet.as_ref(),
        chat: chat.as_ref(),
        // Preserve a promoted document's remote pointer across an organisation edit, so filing it into a
        // project doesn't strip the `source_id`/`external_ref` a later Rebuild needs (a plain vault
        // document has neither line → both `None`, unchanged behaviour).
        source_id: fields.get("source_id").map(String::as_str),
        external_ref: fields.get("external_ref").map(String::as_str),
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
    // No `linked_projects` argument, unlike the vault arm: this document has no front-matter to
    // write it into. Its portable truth is the manifest, which `write_synced` regenerates from the
    // membership join that `write_document_truth` has already updated before dispatching here.
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
            // Atomic, for the same reason the forward write is: an interrupted restore would leave
            // a truncated container, which on an encrypted vault reads as nothing at all.
            let _ = crate::vault::write_atomic(&file, &original);
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
    /// The document's **other** project memberships (#275) — everything except `project`, which
    /// stays the home. Emitted as `linked_projects:`, deliberately NOT `projects:`: a file showing
    /// `project: "Sales"` next to `projects: [...]` reads either as "home plus these" or as "no,
    /// THESE are its projects", and a vault file is a thing users open and hand-edit. The key uses
    /// the same word the UI does — a document is *primary* in one project and *linked* into the
    /// rest.
    ///
    /// Writers pass the extras only; readers drop the home if a hand-edited file repeats it, so a
    /// human writing the obvious thing can't create a double membership.
    linked_projects: &'a [String],
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
    /// Present only for a chat: appends `source_type: chat` + the `chat_*` identity lines so a Rebuild
    /// still routes the file through the chat engine. Mutually exclusive with `photo`/`spreadsheet`.
    /// Omitting it is not cosmetic — it silently demotes a conversation to an ordinary document on the
    /// next Rebuild (see [`ChatIdentity`]).
    chat: Option<&'a ChatIdentity>,
    /// The remote source id + link a **promoted** index-only document keeps, so the claim survives a
    /// Rebuild: without it the rebuilt row would drop the `source_id` and the next connector sync would
    /// re-index the (still-present) source as a duplicate index-only pointer. `None` for every ordinary
    /// vault ingest (a local file has no remote pointer). Only the spreadsheet arm reads them back today.
    source_id: Option<&'a str>,
    external_ref: Option<&'a str>,
}

/// Render YAML front-matter + body. `tags` is written flow-style on one line
/// (`["a", "b"]`) so the flat parser reads it back as a single field.
fn render_markdown(f: &Frontmatter, body: &str) -> String {
    let tags = render_yaml_list(f.tags);
    // Always emitted, even empty, matching `tags:`. A key that is present-or-absent has to be read
    // as two states ("no extras" vs "written by a build that didn't know about extras") at every
    // call site; a key that is always there has one.
    let linked = render_yaml_list(f.linked_projects);
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
         linked_projects: {}\n\
         tags: {}\n\
         importance: {}\n\
         last_activity: {}\n\
         reviewed: {}\n\
         {}{}{}{}---\n\n{}\n",
        yaml_quote(f.title),
        yaml_quote(f.source_path),
        f.ext.unwrap_or(""),
        f.content_hash,
        f.created_at,
        f.ingested_at,
        yaml_quote(f.project),
        linked,
        tags,
        importance,
        f.last_activity,
        f.reviewed,
        render_source_pointer(f.source_id, f.external_ref),
        f.photo.map(render_photo_block).unwrap_or_default(),
        f.spreadsheet
            .map(render_spreadsheet_block)
            .unwrap_or_default(),
        f.chat.map(render_chat_block).unwrap_or_default(),
        body,
    )
}

/// The chat-identity front-matter lines (only present for a chat). `source_type: chat` is the marker
/// `is_chat_vault_file` routes on; `chat_conversation_id` is what `rebuild_chat` needs to find the
/// session. Matches the field names `chat::render_chat_frontmatter` writes at creation, so a rewritten
/// file round-trips through the same parser.
fn render_chat_block(c: &ChatIdentity) -> String {
    let mut s = format!(
        "source_type: chat\n\
         chat_conversation_id: {}\n\
         chat_scope: {}\n",
        c.conversation_id, c.scope,
    );
    if !c.source_id.is_empty() {
        s.push_str(&format!("chat_source_id: {}\n", c.source_id));
    }
    s
}

/// The remote-pointer front-matter lines a promoted index-only document carries (`source_id` +
/// `external_ref`), emitted only when set. Kept out of the source-type blocks so it composes with any
/// of them; `rebuild_one` reads them back to restore the connector claim (see [`Frontmatter::source_id`]).
fn render_source_pointer(source_id: Option<&str>, external_ref: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(id) = source_id {
        s.push_str(&format!("source_id: {}\n", yaml_quote(id)));
    }
    if let Some(r) = external_ref {
        s.push_str(&format!("external_ref: {}\n", yaml_quote(r)));
    }
    s
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
/// non-list value (or `[]`) yields an empty vec.
///
/// The split is quote-aware. It used to be a plain `split(',')`, which was fine while the only
/// flow list was `tags` (the tag editor strips commas) but is wrong for `projects` — a project
/// name is a real name the user typed, and "Atlas, Inc." is exactly the case that motivated
/// allowing commas there. A naive split tore that back out of the vault as two projects called
/// `Atlas` and `Inc.`, silently, on the next rebuild.
pub(crate) fn parse_yaml_list(value: &str) -> Vec<String> {
    let v = value.trim();
    let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Vec::new();
    };
    split_flow_items(inner)
        .into_iter()
        .map(|s| yaml_unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a YAML flow-sequence body on the commas that SEPARATE items, leaving commas inside a
/// quoted scalar alone. `yaml_quote` always quotes, so every item we write is quoted and every
/// embedded comma is inside quotes.
///
/// A hand-edited file with an unbalanced quote yields one long item rather than an error: this
/// parser's whole contract is tolerance (a vault file is the user's, and they may edit it), and a
/// too-long project name is visible and fixable where a panic on open is neither.
fn split_flow_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                items.push(&inner[start..i]);
                start = i + 1; // ',' is one byte, so this stays on a char boundary
            }
            _ => {}
        }
    }
    items.push(&inner[start..]);
    items
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

/// Does PM hold the file behind this document — so that deleting it must unlink something on disk?
///
/// `index_only` is the ONLY pointer kind. Its body lives at the source (Drive, OneDrive, a watched
/// local folder), and its `vault_path` is the synthetic `idx://<source_id>` sentinel no file will
/// ever back. Every other kind — `vault`, `photo`, `spreadsheet`, `chat` — keeps its body in a
/// Markdown file PM wrote, which is precisely why a Rebuild can reconstruct it from the vault walk.
///
/// Deliberately phrased as "is this the pointer kind?" rather than "is this a plain vault
/// document?". The delete paths used to ask the latter (`source_type != "vault"`), which silently
/// reclassified `photo` and `spreadsheet` as pointers the day those kinds were added: the file
/// survived the delete, and the next Rebuild — whose walk treats the vault file as the truth —
/// re-ingested the document the user had deleted. A kind added to the enum later is a document PM
/// owns a file for until someone says otherwise, and this defaults that way. `rebuild`'s own reap
/// sweep already asks the question in this direction.
pub(crate) fn owns_a_vault_file(source_type: Option<&str>) -> bool {
    source_type != Some(SOURCE_TYPE_INDEX_ONLY)
}

/// The vault-relative path of the original image PM saved for `doc_id`, when the user ticked "keep a
/// copy" — `None` for every other document, and for a photo whose original was never saved.
///
/// Call it BEFORE deleting the document. `photos.document_id` is `ON DELETE CASCADE`, so once the
/// row is gone nothing on the machine records where that image is, and an encrypted picture the
/// user believes they deleted sits in the vault indefinitely.
pub(crate) fn saved_photo_original(conn: &Connection, doc_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT vault_path FROM photos WHERE document_id = ?1 AND saved_to_vault = 1",
            params![doc_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|p| !p.trim().is_empty()))
}

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
    /// What the SOURCE says about the document, as opposed to what PM measured at ingest (#701).
    /// `None` everywhere means the provider did not say — rendered as "Unknown", never blank and
    /// never attributed to the user. Only the two cloud connectors and the local folder can fill any
    /// of these; a vault document, chat, photo or spreadsheet has no provider to ask.
    pub source_author: Option<String>,
    pub source_last_modified_by: Option<String>,
    /// The source's own creation time (ISO-8601), distinct from `created_at`, which is PM's.
    pub source_created_at: Option<String>,
    /// The source file's size in bytes, distinct from `byte_size`, which measures the file PM
    /// ingested — an index-only pointer has no such file. `None` for a Google-native Doc/Sheet/Slide,
    /// which has no byte size at all.
    pub source_size_bytes: Option<i64>,
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
            source_author: None,
            source_last_modified_by: None,
            source_created_at: None,
            source_size_bytes: None,
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
    /// The document's OTHER project memberships (#275) — never including `project`, which is the
    /// home. Empty for every fresh ingest: a document arrives in one place and is linked elsewhere
    /// later, by hand.
    pub linked_projects: Vec<String>,
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

/// Whether `path` is a symlink whose target resolves OUTSIDE `root` (L-2). A symlinked file is
/// indexed like any other, but if it points out of the tracked/dropped tree it would pull unrelated
/// content (e.g. `~/.ssh/id_rsa`) into the index — so reject it. A non-symlink file reached by
/// descending from `root` is inherently inside it; only symlinks can escape. If containment can't be
/// verified (a canonicalization failure), err on the side of rejecting.
pub(crate) fn symlink_escapes_root(path: &Path, root: &Path) -> bool {
    let is_symlink = std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_symlink {
        return false;
    }
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        // Both canonical → the one shared containment primitive decides "is P inside R" (L-5).
        (Ok(real), Ok(real_root)) => !crate::pathguard::within_root(&real, &real_root),
        _ => true,
    }
}

/// Recursively collect files from the given paths (folders are walked), plus a count of the entries
/// the walk could not read.
///
/// The count is not bookkeeping: `run` sends `Counted { total: files.len() }` and finishes with counters
/// tallied only inside the loop over that list, so anything this walk drops is missing from the total
/// AND from the summary. Without it, a drop of 400 files where one locked subfolder held 388 of them
/// reports "Done — 12 ingested, 0 skipped, 0 failed" and reads as a complete import.
fn collect_files(inputs: &[String]) -> (Vec<PathBuf>, usize) {
    let mut files = Vec::new();
    let mut unreadable = 0usize;
    for input in inputs {
        let root = Path::new(input);
        collect_into(root, root, &mut files, &mut unreadable, 0);
    }
    (files, unreadable)
}

fn collect_into(
    root: &Path,
    path: &Path,
    out: &mut Vec<PathBuf>,
    unreadable: &mut usize,
    depth: usize,
) {
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
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    // An entry the directory listing itself could not produce. `.flatten()` used to
                    // drop it, so a single bad inode removed a file from the import with no trace.
                    let Ok(entry) = entry else {
                        *unreadable += 1;
                        continue;
                    };
                    collect_into(root, &entry.path(), out, unreadable, depth + 1);
                    if out.len() >= MAX_COLLECTED_FILES {
                        break;
                    }
                }
            }
            // The folder is there and will not open (permissions, an ACL, a share that dropped): its
            // whole subtree is unknown. It counts as ONE entry we could not read, never as the N files
            // that may sit beneath it — that number is unknowable by definition, which is why the user
            // is told "items", not "files".
            Err(_) => *unreadable += 1,
        }
    } else if path.is_file() {
        // L-2: a symlinked file that resolves outside the dropped tree would pull unrelated content
        // into the index — skip it. (A file the user drops *directly* is its own root, so it stays.)
        // Deliberately NOT counted: a refused symlink is a security decision working as designed, not
        // a read failure, and reporting it would alarm the user about a correctly-handled link.
        if !symlink_escapes_root(path, root) {
            out.push(path.to_path_buf());
        }
    } else {
        // Neither a directory nor a file. `is_dir()`/`is_file()` are `metadata(..).unwrap_or(false)`,
        // so this arm is both "the path is genuinely gone" (deleted or renamed between the drop and
        // the import) and "its stat was refused", indistinguishable from here — and both mean the user
        // asked for something they will not get. Count it rather than let it vanish from a total the
        // summary is computed from.
        //
        // This is also where the `symlink_metadata` probe above lands when IT fails: that `if let Ok`
        // falls through into this very chain, so the error is already counted here. Do not add a
        // second branch up there.
        *unreadable += 1;
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

/// Whether a local file's extension is one the ingest pipeline can turn into indexed text — the union
/// of the document (`SUPPORTED`), photo (`PHOTO_EXTS`), and spreadsheet (`SPREADSHEET_EXTS`) routes.
/// Shared with the local-folder connector so a watched-folder walk and `ingest_one` agree on what is
/// worth pointing at; anything else (binaries, archives, temp files) is skipped at the walk.
pub(crate) fn is_supported_source(path: &Path) -> bool {
    match extension(path) {
        Some(e) => {
            let e = e.as_str();
            SUPPORTED.contains(&e) || PHOTO_EXTS.contains(&e) || SPREADSHEET_EXTS.contains(&e)
        }
        None => false,
    }
}

/// One Markdown file in the vault: the `/`-separated path **relative to the vault root** — the exact
/// value `documents.vault_path` stores — plus the absolute path to read it at.
pub(crate) struct VaultFile {
    pub rel: String,
    pub path: PathBuf,
}

/// Every Markdown file in the vault, and whether the walk **provably saw all of them**.
///
/// The one enumeration of the vault, shared by the three walks that must never disagree about what
/// the vault contains: the rebuild sweep, the key migration ([`convert_markdown`]) and the plaintext
/// export ([`export_plaintext`]). They used to each `read_dir` the root separately and
/// non-recursively, which was correct only while the vault was flat — the moment chats moved into
/// [`CHATS_SUBDIR`] the same three would have failed in three different ways: the sweep would see
/// every chat as deleted and **reap the lot**, the migration would strand them under the old key,
/// and the export would quietly omit them. One walk makes that class unrepresentable.
///
/// Recursion is an explicit allow-list ([`MARKDOWN_SUBDIRS`]), never "descend everywhere" — see
/// [`PHOTOS_SUBDIR`].
///
/// The count is the sweep's data-loss guard ([`may_reap`], via `unreadable == 0`) and is
/// **conservative in one direction only**: any dir entry that fails to read raises it, because "we
/// never saw it" must never be read as "the user deleted it". A missing subfolder is not
/// incompleteness — a vault with no chats yet simply has no `chats/`.
///
/// Returns a COUNT rather than the `bool` it carried until Batch K, so the same number can be shown
/// to the user ("N items could not be read") instead of only silently withholding a reap. The two
/// are exactly equivalent — every site that cleared the old flag increments this — so `complete` is
/// `unreadable == 0` at each caller. One unopenable directory counts as **one** entry, never as the
/// N files that might sit under it; that number is unknowable by definition.
pub(crate) fn walk_vault_markdown(vault: &Path) -> Result<(Vec<VaultFile>, usize)> {
    let mut files = Vec::new();
    let mut unreadable = 0usize;
    // The root's own failure propagates: an unreadable vault root is not a partial picture, it is a
    // broken vault, and every caller wants to hear about that rather than act on nothing.
    collect_dir(vault, None, &mut files, &mut unreadable, is_vault_markdown)?;
    for sub in MARKDOWN_SUBDIRS {
        let dir = vault.join(sub);
        match probe(&dir) {
            Ok(Some(meta)) if meta.is_dir() => {}
            // Provably no such subfolder (or something that isn't a directory sitting under the
            // name): absence, not incompleteness. `is_dir()` gave the same answer here for BOTH
            // readings, so a `chats/` we merely couldn't stat used to be skipped as "not there".
            Ok(_) => continue,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        }
        // A subfolder that exists but won't open is exactly the "couldn't tell" case: raise the count
        // rather than propagate, so a locked folder costs one deferred reap, never a deletion.
        if collect_dir(
            &dir,
            Some(sub),
            &mut files,
            &mut unreadable,
            is_vault_markdown,
        )
        .is_err()
        {
            unreadable += 1;
        }
    }
    Ok((files, unreadable))
}

/// Every "keep a copy" original under [`PHOTOS_SUBDIR`], plus how many entries the walk could not
/// read — same counter, same meaning and same conservatism as [`walk_vault_markdown`]'s.
///
/// Deliberately NOT folded into [`walk_vault_markdown`]: [`is_vault_markdown`] accepts any `.pmenc`,
/// so a photos-inclusive document walk would hand every saved photo to the rebuild sweep as a
/// document. The two walks answer different questions over the same vault and must stay separate —
/// which is why this one sits here, next to its sibling, rather than in the module that needs it.
///
/// The `keep` predicate is an allow-list, not a catch-all: only the encrypted originals and the
/// plaintext photo extensions. Anything else under `photos/` is left alone rather than swept.
pub(crate) fn walk_vault_photos(vault: &Path) -> Result<(Vec<VaultFile>, usize)> {
    let mut files = Vec::new();
    let mut unreadable = 0usize;
    let dir = vault.join(PHOTOS_SUBDIR);
    match probe(&dir) {
        Ok(Some(meta)) if meta.is_dir() => {}
        // No photos folder is not incompleteness — a vault where nobody kept a copy simply has none.
        Ok(_) => return Ok((files, unreadable)),
        // But a `photos/` we could not stat is NOT "nobody kept a copy": the sweep would then see
        // every saved original as an orphan-free vault and the re-key would leave them all behind.
        Err(_) => return Ok((files, 1)),
    }
    if collect_dir(
        &dir,
        Some(PHOTOS_SUBDIR),
        &mut files,
        &mut unreadable,
        |p| {
            matches!(extension(p).as_deref(), Some("pmenc"))
                || extension(p)
                    .as_deref()
                    .is_some_and(|e| PHOTO_EXTS.contains(&e))
        },
    )
    .is_err()
    {
        unreadable += 1;
    }
    Ok((files, unreadable))
}

/// One directory's files matching `keep`, appended to `out` with `prefix` (if any) joined by `/`. An
/// entry the walk cannot read increments `unreadable` instead of failing the walk.
fn collect_dir(
    dir: &Path,
    prefix: Option<&str>,
    out: &mut Vec<VaultFile>,
    unreadable: &mut usize,
    keep: impl Fn(&Path) -> bool,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            *unreadable += 1;
            continue;
        };
        let path = entry.path();
        // `keep` FIRST, and it must stay first. It is a pure test of the name, so a path this walk
        // was never going to take (a `.tmp`, a photo original under the document walk) is dropped
        // without a stat — and, more to the point, a stat failure on a file we would have ignored
        // anyway must not withhold the sweep from every file we did read.
        if !keep(&path) {
            continue;
        }
        match probe(&path) {
            Ok(Some(meta)) if meta.is_file() => {}
            // Either not a file (a directory whose name happens to pass `keep`), or provably absent —
            // `read_dir` listed it and it is already gone, which is an ordinary race with a writer
            // removing a temp file, or a dangling symlink. Neither is "I couldn't tell", so neither
            // makes the enumeration partial.
            Ok(_) => continue,
            // Permission denied, an I/O error, a share that dropped mid-walk. We did not learn whether
            // this file exists, so the walk is no longer provably complete: `Path::is_file()` used to
            // answer `false` here and the entry vanished, which is how a locked folder could read as
            // "the user deleted all of these".
            Err(_) => {
                *unreadable += 1;
                continue;
            }
        }
        let name = file_name(&path);
        let rel = match prefix {
            Some(p) => format!("{p}/{name}"),
            None => name,
        };
        out.push(VaultFile { rel, path });
    }
    Ok(())
}

/// Swap the file name in a `/`-separated relative vault path, keeping its folder. `chats/a.md` +
/// `a.md.pmenc` -> `chats/a.md.pmenc`; a root file just becomes the new name.
fn rel_with_name(rel: &str, name: &str) -> String {
    match rel.rfind('/') {
        Some(i) => format!("{}/{name}", &rel[..i]),
        None => name.to_string(),
    }
}

/// The refusal both halves of a key migration return when they cannot see the whole vault.
///
/// `detail` names what could not be read; everything after it is the same for every caller, and has
/// to be, because the state the user is left in is the same. [`crate::vault::migrate::recover`] runs
/// at LAUNCH only, so an `Err` out of [`convert_vault_files`] rolls nothing back in-process: the DB
/// has already been re-keyed (`migrate.rs`, `db::rekey`) and the vault has not, and the journal's
/// backup is only restored on the next start. Without the restart sentence the user is sitting on a
/// database under the new key and a vault under the old one, with nothing on screen explaining why
/// their notes stopped opening.
///
/// Refusing is the trade this batch takes deliberately: a refusal is retryable, an orphaned file
/// under a discarded key is not.
fn rekey_refusal(detail: String) -> Error {
    Error::Other(format!(
        "The key change was stopped because {detail}. Re-encoding the rest would leave those files \
         encrypted under the old key for good, so PM stopped instead. Restart PM before using the \
         vault — the change is only rolled back on the next launch — then try again once the vault \
         folder is readable."
    ))
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
///
/// Walks the whole allow-listed tree ([`walk_vault_markdown`]), so chats under [`CHATS_SUBDIR`]
/// are re-keyed with everything else rather than stranded under the previous key.
///
/// A renamed file re-points **both** tables that name it. `chat_sessions.vault_path` is not
/// bookkeeping: `chat::record_turn_pair` appends the next turn to whatever path that column holds,
/// so leaving it on the pre-rename name made the next message create a SECOND file under the old
/// name and split the transcript in two — both stamped with the same `chat_conversation_id`, which
/// a later Rebuild then fights over on `documents.vault_path`'s UNIQUE. Found auditing #281; the
/// same statement pair fixes it.
///
/// **Fails closed on an incomplete walk** — see [`rekey_refusal`].
pub(crate) fn convert_markdown(
    conn: &Connection,
    dir: &Path,
    read_with: &MarkdownCipher,
    write_with: &MarkdownCipher,
) -> Result<usize> {
    let mut changed = 0usize;
    let (files, unreadable) = walk_vault_markdown(dir)?;
    // Refuse BEFORE the loop rewrites anything. Re-keying whatever the walk happened to see leaves
    // every entry it could not read encrypted under the OLD subkey — with no row, no error and no
    // second chance once that key is discarded. This is the I-15 row the flag exists for, and until
    // Batch K the count was thrown away here (`_complete`), so the migration reported success.
    if unreadable > 0 {
        return Err(rekey_refusal(format!(
            "{unreadable} item(s) in the vault folder at {} could not be read",
            dir.display()
        )));
    }
    for file in files {
        let old_name = file_name(&file.path);
        let new_name = write_with.on_disk_name(&MarkdownCipher::logical_name(&old_name));
        let raw = std::fs::read(&file.path)?;
        // Already in the exact target form? Nothing to do (idempotent). "Exact" means the
        // same name, the same encryption state, AND the same key — a passphrase change
        // keeps the name but moves the subkey, so those files must still be re-encoded.
        if new_name == old_name
            && crate::vault::crypto::is_encrypted(&raw) == write_with.encryption_on()
            && read_with.same_key_as(write_with)
        {
            continue;
        }
        let content = read_with.decode(&raw, &file.path)?;
        // Write beside the original, never back at the vault root — a chat must stay in `chats/`.
        // The AAD binds the file NAME only (`MarkdownCipher::aad_stem`), so which folder a file
        // sits in never enters the ciphertext.
        let parent = file.path.parent().unwrap_or(dir);
        write_with.write_to(&parent.join(&new_name), &content)?;
        if new_name != old_name {
            std::fs::remove_file(&file.path)?;
            let new_rel = rel_with_name(&file.rel, &new_name);
            conn.execute(
                "UPDATE documents SET vault_path = ?1 WHERE vault_path = ?2",
                params![new_rel, file.rel],
            )?;
            conn.execute(
                "UPDATE chat_sessions SET vault_path = ?1 WHERE vault_path = ?2",
                params![new_rel, file.rel],
            )?;
        }
        changed += 1;
    }
    Ok(changed)
}

/// Re-encode a vault's files from `read_with` to `write_with` — **all** of them: the Markdown
/// documents *and* the opt-in saved photo originals under `photos/`. Returns the total changed.
///
/// This is the single entry point for the file half of a key or policy migration, and it exists to
/// make one specific bug unrepresentable. The vault is a flat folder of Markdown **plus** the one
/// [`PHOTOS_SUBDIR`] subfolder, and every walk in this module is deliberately non-recursive — so
/// "convert the vault" implemented as [`convert_markdown`] alone type-checks, passes its own tests,
/// and silently strands every saved original under the previous key. It did exactly that until
/// v3.19.2. Call this; don't hand-roll the pair at the call site.
pub(crate) fn convert_vault_files(
    conn: &Connection,
    dir: &Path,
    read_with: &MarkdownCipher,
    write_with: &MarkdownCipher,
) -> Result<usize> {
    let documents = convert_markdown(conn, dir, read_with, write_with)?;
    let originals = convert_photo_originals(dir, read_with, write_with)?;
    Ok(documents + originals)
}

/// Re-encode every opt-in saved photo original under `vault/photos/` from `read_with` to
/// `write_with` — the byte analogue of [`convert_markdown`], and the other half of a key migration.
/// Prefer [`convert_vault_files`], which pairs the two halves so neither can be forgotten.
/// Returns how many files changed.
///
/// Photo originals are written with the **same Markdown subkey** as the documents around them
/// ([`copy_original_to_vault`]), so a passphrase change moves their key too. Without this they would
/// stay encrypted under the *old* subkey forever — unreadable by the very app that saved them, and
/// gone for good once the user deletes the original they were told they no longer needed.
///
/// Idempotent on the same terms as [`convert_markdown`] (already in the target encryption state
/// *and* under the target key ⇒ skip), so an interrupted migration is safe to re-run.
///
/// Unlike Markdown, the file is **not renamed** when the encryption policy flips: a photo's on-disk
/// name is recorded in `photos.vault_path` *and* in its document's front-matter, so renaming would
/// mean rewriting both from inside the migration's riskiest phase — to buy nothing functional, since
/// [`MarkdownCipher::read_bytes`] dispatches on the magic bytes and `aad_stem` already ignores the
/// suffix (the AAD is identical either way). The cost is cosmetic and worth naming: after a vault is
/// made private, its saved originals keep a `.pmenc` suffix that no longer describes them. Callers
/// that locate a copy by name must therefore try both forms — see [`find_saved_photo_copy`].
///
/// Takes no `Connection`: it writes no rows, so it stays outside the migration's transaction.
///
/// **Fails closed on anything it cannot read** — see [`rekey_refusal`]. This half had three separate
/// holes, all the same one: a `photos/` that would not stat read as "nobody kept a copy", an entry
/// that would not read was filtered away, and a file that would not stat was skipped as "not a file".
pub(crate) fn convert_photo_originals(
    dir: &Path,
    read_with: &MarkdownCipher,
    write_with: &MarkdownCipher,
) -> Result<usize> {
    let photos = dir.join(PHOTOS_SUBDIR);
    let unlistable = |e: &dyn std::fmt::Display| {
        rekey_refusal(format!(
            "the saved photo originals at {} could not be listed: {e}",
            photos.display()
        ))
    };
    match probe(&photos) {
        Ok(Some(meta)) if meta.is_dir() => {}
        // Provably no `photos/` (or something that is not a directory under the name): nobody kept a
        // copy, so there is nothing here to re-encode. Absence, not incompleteness.
        Ok(_) => return Ok(0),
        // A `photos/` we could not stat is NOT "nobody kept a copy" — `is_dir()` answered `false` to
        // both readings, and the migration then committed the new key with every saved original left
        // under the old one. The copy the user kept precisely so they could delete the original.
        Err(e) => return Err(unlistable(&e)),
    }
    let mut changed = 0usize;
    // Collect BEFORE writing. The write is now a temp file plus a rename (`vault::write_atomic`), so
    // each iteration adds and removes a directory entry — and enumerating a directory while it is
    // being mutated is explicitly unspecified on Windows, which can hand the loop its own staging
    // file or skip a photo entirely. `walk_vault_markdown` already materialises for the same reason.
    //
    // An entry that fails to read is propagated, not filtered away: `filter_map(|e| e.ok())` stranded
    // one photo at a time, which is the whole-folder orphan above in miniature.
    let entries: Vec<PathBuf> = std::fs::read_dir(&photos)
        .map_err(|e| unlistable(&e))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<PathBuf>>>()
        .map_err(|e| unlistable(&e))?;
    for path in entries {
        match probe(&path) {
            Ok(Some(meta)) if meta.is_file() => {}
            // Not a file, or gone between the listing and now (a provable absence — there is nothing
            // left to strand under the old key).
            Ok(_) => continue,
            Err(e) => {
                return Err(rekey_refusal(format!(
                    "{} could not be read: {e}",
                    path.display()
                )))
            }
        }
        let raw = std::fs::read(&path)?;
        // "Already in the exact target form" minus the name (see above): the same encryption state
        // AND the same key — a passphrase change keeps both the name and the state but moves the
        // subkey, which is precisely the case this function exists for.
        if crate::vault::crypto::is_encrypted(&raw) == write_with.encryption_on()
            && read_with.same_key_as(write_with)
        {
            continue;
        }
        let plain = read_with.decode_bytes(raw, &path)?;
        write_with.write_bytes_to(&path, &plain)?;
        changed += 1;
    }
    Ok(changed)
}

/// The refusal the plaintext export returns rather than hand over a folder that looks complete.
///
/// [`export_plaintext`]'s own doc records the rule: a hatch that silently drops files is worse than
/// no hatch, because the user checks the folder, sees documents in it, and stops looking. An
/// unreadable entry is exactly that case — the export cannot list what it left out, so it must not
/// produce the folder at all. `detail` names what could not be read.
///
/// Unlike [`rekey_refusal`] this says nothing about restarting: no key moved and no state needs
/// restoring, so the only useful instruction is to retry once the vault folder is readable.
fn export_refusal(detail: String) -> Error {
    Error::Other(format!(
        "The plaintext export was stopped because {detail}. A folder quietly missing files it does \
         not mention still looks like a complete copy of your library, so PM stopped rather than \
         finish one. Delete anything it wrote and try again once the vault folder is readable."
    ))
}

/// Export every Markdown file in `vault` to `dest` as plaintext `.md`, decrypting
/// encrypted files with `cipher` and dropping the `.pmenc` suffix. Returns the count
/// written. The core of the "never locked in" escape hatch — kept here (next to the
/// rebuild walk it mirrors) so it is unit-testable without a running app.
///
/// The vault's folder structure is reproduced, so chats come out under `chats/` rather than
/// avalanching into one directory beside the documents. An escape hatch that silently dropped
/// every chat — which is what a root-only walk would now do — would be worse than no hatch:
/// it looks complete.
///
/// For that same reason it **fails closed** on a walk that could not read part of the vault — see
/// [`export_refusal`] — rather than hand back a folder short of files it does not mention.
pub(crate) fn export_plaintext(
    vault: &Path,
    cipher: &MarkdownCipher,
    dest: &Path,
) -> Result<usize> {
    // Walk BEFORE creating `dest`, so a refusal leaves not even an empty folder behind to be mistaken
    // for an export that produced nothing.
    let (files, unreadable) = walk_vault_markdown(vault)?;
    if unreadable > 0 {
        return Err(export_refusal(format!(
            "{unreadable} item(s) in the vault folder at {} could not be read",
            vault.display()
        )));
    }
    std::fs::create_dir_all(dest)?;
    let mut written = 0usize;
    for file in files {
        // Decrypt-if-needed, then write under the logical `.md` name (no `.pmenc`), in the same
        // relative folder it came from.
        let content = cipher.read(&file.path)?;
        let out = dest.join(rel_with_name(
            &file.rel,
            &MarkdownCipher::logical_name(&file_name(&file.path)),
        ));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out, content)?;
        written += 1;
    }
    // The saved photo originals live one level down and are encrypted with the same subkey, so the
    // escape hatch only tells the truth if they come out too — under their real `.png`/`.jpg` name,
    // which is what makes the exported folder openable by anything other than PM.
    let photos = vault.join(PHOTOS_SUBDIR);
    let has_photos = match probe(&photos) {
        Ok(Some(meta)) => meta.is_dir(),
        // No `photos/` at all — nobody kept a copy, so there is nothing to free.
        Ok(None) => false,
        // But a `photos/` we could not stat would have read as "no photos" and produced an export
        // silently missing every saved original — the files the user was told they could delete the
        // source of. `entry?` below already propagates for the same reason.
        Err(e) => {
            return Err(export_refusal(format!(
                "the saved photo originals at {} could not be read: {e}",
                photos.display()
            )))
        }
    };
    if has_photos {
        let out_dir = dest.join(PHOTOS_SUBDIR);
        std::fs::create_dir_all(&out_dir)?;
        for entry in std::fs::read_dir(&photos)? {
            let path = entry?.path();
            match probe(&path) {
                Ok(Some(meta)) if meta.is_file() => {}
                // Not a file, or gone since the listing — nothing to export either way.
                Ok(_) => continue,
                Err(e) => {
                    return Err(export_refusal(format!(
                        "{} could not be read: {e}",
                        path.display()
                    )))
                }
            }
            let bytes = cipher.read_bytes(&path)?;
            let out_name = MarkdownCipher::logical_name(&file_name(&path));
            std::fs::write(out_dir.join(out_name), bytes)?;
            written += 1;
        }
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

/// Encode a vector as the raw little-endian `f32` blob sqlite-vec accepts natively (its
/// `fvec_from_value` memcpys any BLOB whose byte length is a multiple of 4 — here exactly 4×dim).
/// vec0 stores parsed float32 regardless of the input encoding, so blob-bound rows and the older
/// JSON-text-bound rows are byte-identical at rest and interchangeable at query time; binding the
/// blob just skips a JSON round-trip per vector. The one encoding for every `chunk_vec` write and
/// KNN `MATCH` bind (see `retrieval::vector_search`).
pub(crate) fn embedding_blob(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for f in vector {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
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

pub(crate) fn iso_from_mtime(conn: &Connection, path: &Path) -> Result<String> {
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
    fn an_imported_file_is_its_own_source() {
        // A drag-and-drop was the one path where every source fact read "Unknown" about a file PM
        // had in its hands — the document IS the source here, so what it states is what the source
        // says (#709). The discriminator comes from `base` and is never overwritten by the facts.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.docx");
        std::fs::write(&path, b"x").unwrap();
        let stated = crate::sidecar::FileProperties {
            author: Some("Jane Okafor".into()),
            last_modified_by: Some("Sam Reyes".into()),
            created_at: Some("2023-04-11T09:12:00Z".into()),
        };
        let meta = imported_file_source(
            SourceMeta::spreadsheet(),
            stated,
            &path,
            "2026-08-02T10:00:00Z",
            Some(4096),
        );
        assert_eq!(meta.source_type, SOURCE_TYPE_SPREADSHEET);
        assert_eq!(meta.source_author.as_deref(), Some("Jane Okafor"));
        assert_eq!(meta.source_last_modified_by.as_deref(), Some("Sam Reyes"));
        assert_eq!(
            meta.source_created_at.as_deref(),
            Some("2023-04-11T09:12:00Z")
        );
        assert_eq!(
            meta.source_modified_at.as_deref(),
            Some("2026-08-02T10:00:00Z")
        );
        assert_eq!(meta.source_size_bytes, Some(4096));
    }

    #[test]
    fn a_file_that_states_nothing_still_answers_created_from_the_disk() {
        // A dropped .txt has no property block to read, but PM is holding the file — reporting
        // "Unknown" for a date the filesystem will state outright is a miss, not honesty.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"x").unwrap();
        // Only where the platform has a birth time to give; `None` there is the ordinary case on a
        // Linux filesystem without statx support, not a failure.
        if file_birth_time(&path).is_some() {
            let meta = imported_file_source(
                SourceMeta::default(),
                crate::sidecar::FileProperties::default(),
                &path,
                "2026-08-02T10:00:00Z",
                Some(1),
            );
            assert!(meta.source_created_at.is_some());
            assert!(
                meta.source_author.is_none(),
                "no author is still no author — never the OS account"
            );
        }
        assert!(file_birth_time(Path::new("/pm/test/gone.txt")).is_none());
    }

    fn doc_event(path: &str) -> IngestEvent {
        IngestEvent::Failed {
            path: path.into(),
            error: "x".into(),
        }
    }

    #[test]
    fn a_landed_file_that_lost_something_says_so_on_its_row() {
        // A photo whose OCR did not run indexes perfectly normally — same title, same chunks, an
        // empty body where the receipt's text should be. `ocr_ran` is the only thing that knows,
        // and until this it had no reader anywhere in Rust or TS. The detail line is the reader.
        assert_eq!(done_detail(3, None), "3 chunks");
        assert_eq!(done_detail(1, None), "1 chunk");
        assert_eq!(
            done_detail(2, Some("text recognition did not run")),
            "2 chunks — text recognition did not run"
        );
    }

    #[test]
    fn snapshot_counts_completed_files_not_started_ones() {
        // This is what a tab returning mid-rebuild renders, so the count has to match the views':
        // `Started` announces a file, the terminal event completes it. Counting both would double.
        let mut snap = crate::IngestJobState::default();
        apply_event(&mut snap, &IngestEvent::Counted { total: 3 });
        assert_eq!((snap.processed, snap.total), (0, Some(3)));

        apply_event(
            &mut snap,
            &IngestEvent::Started {
                path: "a".into(),
                name: "a".into(),
            },
        );
        assert_eq!(
            snap.processed, 0,
            "Started announces work, it doesn't finish it"
        );

        apply_event(&mut snap, &doc_event("a"));
        apply_event(
            &mut snap,
            &IngestEvent::Skipped {
                path: "b".into(),
                reason: "dupe".into(),
            },
        );
        assert_eq!(snap.processed, 2, "failed and skipped both complete a file");
    }

    #[test]
    fn snapshot_drops_the_setup_label_once_counting_starts() {
        // Preparing (engine install / model download) has no total, so the bar sweeps; once the
        // count lands the bar goes determinate and the label must not linger beside it.
        let mut snap = crate::IngestJobState::default();
        apply_event(
            &mut snap,
            &IngestEvent::Preparing {
                message: "Preparing the document engine…".into(),
            },
        );
        assert!(snap.prep.is_some() && snap.total.is_none());

        apply_event(&mut snap, &IngestEvent::Counted { total: 7 });
        assert!(snap.prep.is_none());
        assert_eq!(snap.total, Some(7));
    }

    #[test]
    fn snapshot_keeps_the_final_report_for_a_tab_that_returns_after_it_finished() {
        // The live event only reaches a mounted listener; someone who came back afterwards still
        // needs to see how it went, so the counts live in the snapshot too. `unreadable` included:
        // "the run was partial" is the one number a returning user must not be shown as a zero.
        let mut snap = crate::IngestJobState::default();
        apply_event(
            &mut snap,
            &IngestEvent::Finished {
                ingested: 5,
                skipped: 1,
                failed: 2,
                unreadable: 3,
            },
        );
        let report = snap.last_report.expect("a finished run reports its counts");
        assert_eq!(
            (
                report.ingested,
                report.skipped,
                report.failed,
                report.unreadable
            ),
            (5, 1, 2, 3)
        );
    }

    #[test]
    fn snapshot_keeps_one_activity_row_per_file_and_amends_it_in_place() {
        // The rows a returning tab renders. `Started` opens a row; the terminal event amends that
        // same row rather than appending, so one file is one line — and because the backend always
        // has the preceding `Started` in hand, a skip/failure can never produce a nameless row (the
        // frontend, mounting mid-file, had no name to amend and pushed "failed — …" with none).
        let mut snap = crate::IngestJobState::default();
        apply_event(
            &mut snap,
            &IngestEvent::Started {
                path: "a".into(),
                name: "notes.md".into(),
            },
        );
        assert_eq!(snap.recent.len(), 1);
        assert_eq!(snap.recent[0].status, "working");

        apply_event(
            &mut snap,
            &IngestEvent::Skipped {
                path: "a".into(),
                reason: "already indexed".into(),
            },
        );
        assert_eq!(snap.recent.len(), 1, "the row is amended, not appended");
        assert_eq!(snap.recent[0].name, "notes.md");
        assert_eq!(snap.recent[0].status, "skipped");
        assert_eq!(snap.recent[0].detail.as_deref(), Some("already indexed"));
        assert!(!snap.recent_truncated);
    }

    #[test]
    fn activity_rows_are_capped_keeping_the_tail() {
        // A 10k-file rebuild must not grow the snapshot without bound. The TAIL is what someone
        // returning is looking for, so the oldest rows go first and the truncation is flagged.
        let mut snap = crate::IngestJobState::default();
        for i in 0..(crate::RECENT_ITEMS_CAP + 5) {
            apply_event(
                &mut snap,
                &IngestEvent::Started {
                    path: format!("f{i}"),
                    name: format!("f{i}"),
                },
            );
        }
        assert_eq!(snap.recent.len(), crate::RECENT_ITEMS_CAP);
        assert!(snap.recent_truncated);
        assert_eq!(snap.recent[0].name, "f5", "the oldest rows are dropped");
        assert_eq!(
            snap.recent[crate::RECENT_ITEMS_CAP - 1].name,
            format!("f{}", crate::RECENT_ITEMS_CAP + 4)
        );
    }

    #[test]
    fn a_regular_file_is_never_a_symlink_escape() {
        // The common case: a real file reached by walking the tree is inherently inside root (L-2).
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("note.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(!symlink_escapes_root(&f, dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_root_escapes_but_one_inside_does_not() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        let inside = root.join("real.txt");
        std::fs::write(&inside, b"ok").unwrap();

        let escape = root.join("escape");
        symlink(&outside, &escape).unwrap();
        let local = root.join("local");
        symlink(&inside, &local).unwrap();

        assert!(
            symlink_escapes_root(&escape, &root),
            "a symlink pointing outside root is an escape (L-2)"
        );
        assert!(
            !symlink_escapes_root(&local, &root),
            "a symlink pointing inside root is allowed"
        );
    }

    #[test]
    fn probe_tells_a_provable_absence_from_a_failure_to_look() {
        // The primitive every walk in this file now stands on. `Path::is_file()` answers `false` to
        // both questions at once; this must not.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("note.txt");
        std::fs::write(&f, b"x").unwrap();

        assert!(
            probe(&f).unwrap().is_some_and(|m| m.is_file()),
            "a real file comes back with its metadata"
        );
        assert!(probe(dir.path()).unwrap().is_some_and(|m| m.is_dir()));
        assert!(
            probe(&dir.path().join("nope.txt")).unwrap().is_none(),
            "NotFound is a PROVABLE absence — the one error kind a walk may act on"
        );

        // The arm that matters most and that no portable filesystem can be made to produce on demand:
        // anything other than NotFound stays an `Err`, so a refused stat can never be read as "gone".
        use std::io::ErrorKind;
        assert!(classify_probe::<()>(Err(ErrorKind::PermissionDenied.into())).is_err());
        assert!(classify_probe::<()>(Err(ErrorKind::Other.into())).is_err());
        assert!(classify_probe::<()>(Err(ErrorKind::TimedOut.into())).is_err());
        assert!(classify_probe::<()>(Err(ErrorKind::NotFound.into()))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_dropped_path_that_vanished_is_counted_unreadable() {
        // A path the user dropped that is no longer there by the time the walk reaches it — moved or
        // deleted between the drop and the import. Before Batch K it fell off the `is_dir`/`is_file`
        // chain with no `else`, so it was missing from `Counted { total }` AND from every counter in
        // the summary: the import reported a clean run over a file it never opened.
        let dir = tempfile::tempdir().unwrap();
        let ghost = dir.path().join("moved-away.md");
        let (files, unreadable) = collect_files(&[ghost.to_string_lossy().into_owned()]);
        assert!(files.is_empty());
        assert_eq!(
            unreadable, 1,
            "the drop must be reported, not silently lost"
        );
    }

    #[test]
    fn a_clean_drop_reports_nothing_unreadable() {
        // The regression fence on the new `else` arm: an ordinary nested folder must count ZERO. A
        // counter that inflates on the happy path would put "could not be read" on every import and
        // the user would learn to ignore the one that matters.
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("sub").join("deeper");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("a.md"), "a").unwrap();
        std::fs::write(dir.path().join("sub").join("b.md"), "b").unwrap();
        std::fs::write(nested.join("c.md"), "c").unwrap();

        let (files, unreadable) = collect_files(&[dir.path().to_string_lossy().into_owned()]);
        assert_eq!(files.len(), 3);
        assert_eq!(unreadable, 0, "a walk that read everything reports nothing");
    }

    #[cfg(unix)]
    #[test]
    fn a_drop_folder_that_will_not_open_is_one_unreadable_entry_and_its_siblings_still_import() {
        // The locked-folder case itself: drop 400 files, one subfolder refuses to open, and the run
        // used to report "Done — 12 ingested, 0 skipped, 0 failed". Unix-gated because Windows ACL
        // manipulation in a unit test is unreliable; the portable fence is the vanished-path test.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("drop");
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(root.join("visible.md"), "ok").unwrap();
        std::fs::write(locked.join("hidden-1.md"), "x").unwrap();
        std::fs::write(locked.join("hidden-2.md"), "y").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let (files, unreadable) = collect_files(&[root.to_string_lossy().into_owned()]);
        // Restore before asserting, so a failure still leaves a removable tempdir behind.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(files.len(), 1, "the readable sibling still imports");
        assert_eq!(
            unreadable, 1,
            "one unopenable folder is ONE entry, never the two files inside it — that number is \
             unknowable by definition, which is why the user is told 'items'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_vault_subfolder_that_will_not_open_withholds_the_sweep() {
        // I-15's whole point. A `chats/` that exists but refuses to open must not read as "the user
        // deleted every chat": the walk reports it, `may_reap` withholds the reap, and the next
        // complete pass does the deleting.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        let chats = vault.join(CHATS_SUBDIR);
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(vault.join("report-01-07-2026-ff00.md"), "doc").unwrap();
        std::fs::write(chats.join("chat-28-06-2026-abc123def456.md"), "chat").unwrap();
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o000)).unwrap();

        let walked = walk_vault_markdown(&vault);
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o700)).unwrap();

        let (files, unreadable) = walked.unwrap();
        assert_eq!(files.len(), 1, "the root document is still enumerated");
        assert_eq!(unreadable, 1);
        assert!(
            !may_reap(unreadable == 0),
            "a partial picture must never sweep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_vault_file_whose_stat_is_refused_withholds_the_sweep() {
        // `collect_dir` itself, one level below the test above. A directory with read but no execute
        // permission LISTS its entries and refuses to stat any of them — which is exactly the shape
        // `path.is_file()` collapsed into "not a file", making each entry vanish from the walk. The
        // whole vault would then look deleted to the sweep.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        let chats = vault.join(CHATS_SUBDIR);
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(chats.join("chat-28-06-2026-abc123def456.md"), "chat").unwrap();
        // r-- : `read_dir` succeeds, `metadata` on each entry does not.
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o444)).unwrap();

        let walked = walk_vault_markdown(&vault);
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o700)).unwrap();

        let (files, unreadable) = walked.unwrap();
        assert!(files.is_empty());
        assert_eq!(
            unreadable, 1,
            "an entry we could not stat is not an absent one"
        );
        assert!(!may_reap(unreadable == 0));
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_keeps_the_walk_complete() {
        // THE regression pin. `NotFound` is a provable absence, so a broken link is nothing to hold
        // back for — and it must not be, because a walk that counted it would withhold EVERY future
        // sweep for as long as the link sat there. One broken link, no reap, ever.
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join("report-01-07-2026-ff00.md"), "doc").unwrap();
        symlink(vault.join("no-such-target.md"), vault.join("dangling.md")).unwrap();

        let (files, unreadable) = walk_vault_markdown(&vault).unwrap();
        assert_eq!(files.len(), 1, "the broken link is not a document to index");
        assert_eq!(unreadable, 0, "provably nothing there ⇒ nothing was missed");
        assert!(may_reap(unreadable == 0));

        // Same rule on the drop walk: a broken link inside a dropped folder is not an import failure.
        let (dropped, drop_unreadable) = collect_files(&[vault.to_string_lossy().into_owned()]);
        assert_eq!(dropped.len(), 1);
        assert_eq!(drop_unreadable, 0);
    }

    /// A vault holding one readable root document and a `chats/` that exists but will not open — the
    /// shape both fail-closed tests below need. Returns the vault root and the readable document's
    /// path; the caller must restore the permissions before asserting (a panic would otherwise leave
    /// an undeletable temp dir behind).
    #[cfg(unix)]
    fn vault_with_an_unopenable_chats_folder(root: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let vault = root.join("vault");
        let chats = vault.join(CHATS_SUBDIR);
        std::fs::create_dir_all(&chats).unwrap();
        let readable = vault.join("report-01-07-2026-ff00.md");
        std::fs::write(&readable, "the sibling that must survive").unwrap();
        std::fs::write(chats.join("chat-28-06-2026-abc123def456.md"), "chat").unwrap();
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o000)).unwrap();
        (vault, readable)
    }

    #[cfg(unix)]
    #[test]
    fn a_rekey_over_a_vault_it_cannot_fully_read_refuses_and_rewrites_nothing() {
        // The orphan pin, and the reason this fails CLOSED. Re-keying what the walk happened to see
        // leaves the rest under the old subkey forever — unreadable by the app that wrote it, with no
        // row and no error. Before Batch K the completeness flag was discarded here (`_complete`) and
        // the migration reported success.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (vault, readable) = vault_with_an_unopenable_chats_folder(dir.path());
        let chats = vault.join(CHATS_SUBDIR);
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        let old = MarkdownCipher::plaintext("vault-1");
        let new = MarkdownCipher::for_test_encrypted("vault-1");

        let result = convert_markdown(&conn, &vault, &old, &new);
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o700)).unwrap();

        let err = result
            .expect_err("an incompletely enumerated vault must not be re-keyed")
            .to_string();
        assert!(
            err.contains("could not be read"),
            "names what stopped it: {err}"
        );
        assert!(
            err.contains("Restart PM"),
            "`recover` is launch-only, so without this the user sits on a re-keyed DB and an \
             old-key vault with no visible reason: {err}"
        );
        // And it refused BEFORE touching a file: the readable sibling is untouched, still plaintext,
        // still under the OLD cipher — not half a vault re-encoded around the hole.
        assert_eq!(
            old.read(&readable).unwrap(),
            "the sibling that must survive"
        );
        assert!(
            !vault.join("report-01-07-2026-ff00.md.pmenc").exists(),
            "nothing may be rewritten before the refusal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_export_over_a_vault_it_cannot_fully_read_refuses_and_writes_nothing() {
        // The escape hatch's own doc: a folder that looks complete is worse than no folder. It cannot
        // list what it left out, so it must not produce the folder at all.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (vault, _) = vault_with_an_unopenable_chats_folder(dir.path());
        let chats = vault.join(CHATS_SUBDIR);
        let dest = dir.path().join("export");

        let result = export_plaintext(&vault, &MarkdownCipher::plaintext("vault-1"), &dest);
        std::fs::set_permissions(&chats, std::fs::Permissions::from_mode(0o700)).unwrap();

        let err = result
            .expect_err("a partial vault must not export")
            .to_string();
        assert!(err.contains("could not be read"), "got: {err}");
        assert!(
            !dest.exists(),
            "the walk runs first, so a refusal leaves not even an empty folder to mistake for an \
             export that found nothing"
        );
    }

    #[test]
    fn warmup_ok_requires_the_exact_expected_width() {
        // The rebuild warmup only proceeds to wipe the index when the model emits the exact width
        // we expect — this guards against an offline/absent model (None) and a wrong-dimension
        // export (e.g. a 384-d export sneaking into the 1024-d slot).
        assert!(warmup_ok(Some(1024), 1024));
        assert!(!warmup_ok(Some(384), 1024));
        assert!(!warmup_ok(None, 1024));
    }

    #[test]
    fn guard_dimension_refuses_a_width_the_live_table_cannot_hold() {
        // The embed-WRITE seam (`AppState::gateway_for_write`, F-46) leans on this guard to turn a
        // raw vec0 width mismatch — which a search-language switch before re-index creates on the
        // *unattended* connector-sync write paths — into the same "re-index" guidance `ingest::run`
        // already gives, instead of a cryptic sqlite-vec error every sync cycle.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        let embedder = crate::db::selected_embedder(&conn).unwrap();
        // Fresh vault: chunk_vec matches the selected embedder's width -> the guard passes.
        guard_dimension(&conn, &embedder).expect("a matching width must pass");
        // Rebuild chunk_vec at a width the embedder can't fill (derived from the embedder so the
        // test pins no model id and holds whatever the vault default is): the guard must refuse.
        let other = embedder.dimension + 128;
        conn.execute_batch(&format!(
            "DROP TABLE chunk_vec; CREATE VIRTUAL TABLE chunk_vec USING vec0(embedding float[{other}]);"
        ))
        .unwrap();
        let err = guard_dimension(&conn, &embedder).unwrap_err();
        assert!(err.to_string().contains("re-index"), "got: {err}");
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
    fn plan_rebuild_one_skips_only_this_very_pass() {
        // The resume rule (#371). A pass id is minted per RUN, so "this run already did it" is the only
        // thing that skips work.
        assert_eq!(
            plan_rebuild_one(Some("pass-a"), "pass-a"),
            RebuildPlan::AlreadyDone
        );
        // Never rebuilt, or rebuilt long ago under a v34 store (NULL) → do the work.
        assert_eq!(plan_rebuild_one(None, "pass-a"), RebuildPlan::Rebuild);
        // THE case that keeps "Rebuild" meaning rebuild: a fresh run mints a new id, so a document a
        // PREVIOUS rebuild finished is redone. If this ever returned AlreadyDone, every rebuild after the
        // first would silently do nothing and the repair button would be a lie.
        assert_eq!(
            plan_rebuild_one(Some("pass-a"), "pass-b"),
            RebuildPlan::Rebuild
        );
    }

    #[test]
    fn may_reap_only_on_a_provably_complete_walk() {
        assert!(may_reap(true), "whole walk enumerated → safe to sweep");
        // A dir entry we couldn't read: a file may exist that we never enumerated, and in the worst case
        // the vault root itself is half-readable — sweeping that picture could delete the library.
        assert!(!may_reap(false), "a partial walk must never sweep");
    }

    #[test]
    fn plan_reap_deletes_only_on_a_provable_absence() {
        // The sweep's whole purpose: the user deleted the vault file.
        assert_eq!(plan_reap(FileState::Gone), ReapPlan::Delete);
        // Still there → keep, whether we rebuilt it this pass or another writer added it underneath us.
        assert_eq!(plan_reap(FileState::Present), ReapPlan::Keep);
        // THE data-loss guard. "I couldn't tell" is not "it's gone": a vault on a network share that
        // drops, or a folder an antivirus scanner locks, must never be read as the user deleting their
        // library. `Path::exists()` would collapse this into `false` — hence `FileState`.
        assert_eq!(plan_reap(FileState::Unknown), ReapPlan::Keep);
    }

    #[test]
    fn file_state_reports_a_real_absence_as_gone_not_unknown() {
        // Pins `FileState::of`'s mapping against the file system itself, so the sweep's one destructive
        // input can't silently invert.
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("here.md");
        std::fs::write(&present, "x").unwrap();
        assert_eq!(FileState::of(&present), FileState::Present);
        assert_eq!(FileState::of(&dir.path().join("nope.md")), FileState::Gone);
    }

    #[test]
    fn upsert_keeps_the_document_id_so_corrections_survive_a_rebuild() {
        // `corrections.document_id` is declared ON DELETE SET NULL (migrations.rs), so the old
        // drop-and-recreate rebuild silently unlinked the ENTIRE Learning-You corpus from its documents on
        // every pass — unreconstructably. Rebuilding in place is what fixes that, and this is the pin.
        let (_d, conn, vault_id, _idx) = store_with_one_of_each();
        conn.execute(
            "INSERT INTO corrections(document_id, field, before_val, after_val, title) \
             VALUES (?1, 'project', '\"Unsorted\"', '\"Atlas\"', 'V')",
            params![vault_id],
        )
        .unwrap();

        let meta = DocMeta {
            source_path: None,
            vault_path: "v.md".into(),
            title: "V, re-titled".into(),
            content_hash: "hv".into(),
            ext: None,
            byte_size: None,
            created_at: None,
            ingested_at: "2026-07-16T00:00:00Z".into(),
            project: "Unsorted".into(),
            linked_projects: Vec::new(),
            tags: vec![],
            importance: None,
            reviewed: false,
            last_activity: None,
            source: SourceMeta::default(),
        };
        update_document_row(&conn, vault_id, &meta).unwrap();

        let still_linked: i64 = conn
            .query_row(
                "SELECT count(*) FROM corrections WHERE document_id = ?1",
                params![vault_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            still_linked, 1,
            "the correction must still point at its document"
        );
        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = ?1",
                params![vault_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "V, re-titled", "the row is updated, not merely kept");
    }

    #[test]
    fn upsert_preserves_a_byte_size_the_vault_cannot_restore() {
        // `byte_size` is measured at ingest from the ORIGINAL file and never written to the vault
        // front-matter, so every rebuild passes None for it. A plain assignment nulled it on every pass;
        // the COALESCE in `update_document_row` is what keeps it.
        let (_d, conn, vault_id, _idx) = store_with_one_of_each();
        conn.execute(
            "UPDATE documents SET byte_size = 4096 WHERE id = ?1",
            params![vault_id],
        )
        .unwrap();

        let meta = DocMeta {
            source_path: None,
            vault_path: "v.md".into(),
            title: "V".into(),
            content_hash: "hv".into(),
            ext: None,
            byte_size: None, // exactly what `rebuild_one` supplies
            created_at: None,
            ingested_at: "2026-07-16T00:00:00Z".into(),
            project: "Unsorted".into(),
            linked_projects: Vec::new(),
            tags: vec![],
            importance: None,
            reviewed: false,
            last_activity: None,
            source: SourceMeta::default(),
        };
        update_document_row(&conn, vault_id, &meta).unwrap();

        let size: Option<i64> = conn
            .query_row(
                "SELECT byte_size FROM documents WHERE id = ?1",
                params![vault_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            size,
            Some(4096),
            "a rebuild must not forget the file's size"
        );
    }

    #[test]
    fn upsert_preserves_the_source_facts_the_vault_cannot_restore() {
        // Exactly the `byte_size` story, for the four columns #701 added: the author, last editor,
        // created date and size come from the PROVIDER, the vault front-matter carries none of them,
        // and `rebuild_one` therefore supplies None for all four. They shipped without the COALESCE
        // that had already been added next door, so a Rebuild wiped them off every promoted row.
        let (_d, conn, vault_id, _idx) = store_with_one_of_each();
        conn.execute(
            "UPDATE documents SET source_author = 'Ada Lovelace', \
                 source_last_modified_by = 'Grace Hopper', \
                 source_created_at = '2026-01-01T00:00:00Z', source_size_bytes = 4096 \
             WHERE id = ?1",
            params![vault_id],
        )
        .unwrap();

        let meta = DocMeta {
            source_path: None,
            vault_path: "v.md".into(),
            title: "V".into(),
            content_hash: "hv".into(),
            ext: None,
            byte_size: None,
            created_at: None,
            ingested_at: "2026-07-16T00:00:00Z".into(),
            project: "Unsorted".into(),
            linked_projects: Vec::new(),
            tags: vec![],
            importance: None,
            reviewed: false,
            last_activity: None,
            source: SourceMeta::default(), // exactly what `rebuild_one` supplies
        };
        update_document_row(&conn, vault_id, &meta).unwrap();

        let (author, editor, created, size): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT source_author, source_last_modified_by, source_created_at, \
                        source_size_bytes FROM documents WHERE id = ?1",
                params![vault_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(editor.as_deref(), Some("Grace Hopper"));
        assert_eq!(created.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(
            size,
            Some(4096),
            "a rebuild must not forget what the source said"
        );
    }

    #[test]
    fn stamping_a_pass_is_what_a_resume_reads_back() {
        // The checkpoint round-trip: what `stamp_rebuild_pass` writes is what the loop's skip check reads.
        let (_d, conn, vault_id, _idx) = store_with_one_of_each();
        assert_eq!(
            stored_pass(&conn, "v.md").unwrap(),
            Some(None),
            "a v34 row exists but carries no pass"
        );
        stamp_rebuild_pass(&conn, vault_id, "pass-a").unwrap();
        assert_eq!(
            stored_pass(&conn, "v.md").unwrap(),
            Some(Some("pass-a".into()))
        );
        // A file with no document at all reads as absent — distinct from "present but unstamped".
        assert_eq!(stored_pass(&conn, "gone.md").unwrap(), None);
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
    fn only_an_index_only_document_is_a_pointer() {
        // The classification behind every delete path. It used to be asked the other way round --
        // "is it a plain vault document?" -- which silently reclassified `photo` and `spreadsheet`
        // as pointers the day those kinds were added: the delete skipped the Markdown, and the
        // next Rebuild (whose walk treats the vault file as the truth) re-ingested the document the
        // user had deleted. Enumerated deliberately, so a new source type has to come here and
        // state which side it is on.
        assert!(owns_a_vault_file(Some(SOURCE_TYPE_VAULT)));
        assert!(owns_a_vault_file(Some(SOURCE_TYPE_PHOTO)));
        assert!(owns_a_vault_file(Some(SOURCE_TYPE_SPREADSHEET)));
        assert!(owns_a_vault_file(Some(SOURCE_TYPE_CHAT)));
        assert!(owns_a_vault_file(None), "an unset kind is not a pointer");
        assert!(!owns_a_vault_file(Some(SOURCE_TYPE_INDEX_ONLY)));
        // The one that must never flip: PM does not delete from Drive, OneDrive or a watched
        // folder, and an index-only `vault_path` is the `idx://` sentinel no file ever backs.
        assert!(!owns_a_vault_file(Some("index_only")));
    }

    #[test]
    fn the_saved_photo_original_is_found_before_the_row_cascades_away() {
        // A photo ingested with "keep a copy" leaves an encrypted image in `vault/photos/`. Nothing
        // has ever deleted it -- and `photos.document_id` cascades, so the moment the document goes
        // the only record of where that picture lives goes with it.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        let seed = |vault_path: &str, hash: &str, saved: i64, original: Option<&str>| -> i64 {
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash, source_type) \
                 VALUES (?1,'T',?2,'photo')",
                params![vault_path, hash],
            )
            .unwrap();
            let doc = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO photos(document_id, file_hash, saved_to_vault, vault_path) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![doc, hash, saved, original],
            )
            .unwrap();
            doc
        };
        let kept = seed("a.md", "ha", 1, Some("photos/ha.png.pmenc"));
        let not_kept = seed("b.md", "hb", 0, None);
        // A saved_to_vault row whose path never landed: the copy does not exist, so there is
        // nothing to unlink and an empty string must not become `vault/`.
        let blank = seed("c.md", "hc", 1, Some("  "));

        assert_eq!(
            saved_photo_original(&conn, kept).unwrap().as_deref(),
            Some("photos/ha.png.pmenc")
        );
        assert_eq!(saved_photo_original(&conn, not_kept).unwrap(), None);
        assert_eq!(saved_photo_original(&conn, blank).unwrap(), None);

        // A document with no photos row at all -- every non-photo document.
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash) VALUES ('d.md','T','hd')",
            [],
        )
        .unwrap();
        let plain = conn.last_insert_rowid();
        assert_eq!(saved_photo_original(&conn, plain).unwrap(), None);

        // And it really does vanish with the document, which is why the caller reads it first.
        let tx = conn.unchecked_transaction().unwrap();
        delete_document(&tx, kept).unwrap();
        tx.commit().unwrap();
        assert_eq!(saved_photo_original(&conn, kept).unwrap(), None);
    }

    /// v50 exists to take `WHERE reviewed = 0` off a full table scan — a read the sidebar badge
    /// fires on every view change, over rows each carrying a ~500-char `stored_summary`, every page
    /// of it decrypted. Pinned as a PLAN, not a duration: a wall-clock assertion would be flaky on a
    /// low-RAM box, and the plan is the durable claim.
    #[test]
    fn the_review_badge_reads_a_covering_index_and_the_library_listing_still_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        let plan = |sql: &str| -> String {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            rows.join(" | ")
        };

        // Must stay byte-identical to `review_queue_count`'s statement.
        let badge = plan("SELECT count(*) FROM documents WHERE reviewed = 0");
        assert!(
            badge.contains("COVERING INDEX idx_documents_reviewed"),
            "the badge must not touch the table at all; plan was: {badge}"
        );

        // The guard that matters. `list_documents` returns the WHOLE table, so a sequential scan plus
        // a temp b-tree is already its optimal plan. Widening v50 to carry the listing's sort keys
        // looks strictly better and measured 2x slower on a fresh import — see the v50 comment. If
        // this assertion starts failing, the index grew columns it must not have.
        let listing = plan(&format!(
            "SELECT {DOCUMENT_COLUMNS} FROM documents d ORDER BY d.ingested_at DESC, d.id DESC"
        ));
        assert!(
            !listing.contains("idx_documents_reviewed"),
            "the whole-library listing must still scan; plan was: {listing}"
        );
    }

    /// v50 is behaviour-neutral, checked rather than asserted: an index changes no result, and
    /// `ORDER BY d.ingested_at DESC, d.id DESC` is a total order, so no tie can drift either.
    #[test]
    fn the_review_queue_reads_the_same_rows_with_and_without_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, reviewed, ingested_at) VALUES \
                 (1,'a.md','A','ha',0,'2026-01-03T00:00:00Z'), \
                 (2,'b.md','B','hb',1,'2026-01-02T00:00:00Z'), \
                 (3,'c.md','C','hc',0,'2026-01-02T00:00:00Z'), \
                 (4,'d.md','D','hd',0,'2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let queued = |c: &Connection| -> Vec<i64> {
            review_queue(c).unwrap().into_iter().map(|d| d.id).collect()
        };

        assert_eq!(queued(&conn), vec![1, 3, 4]);
        assert_eq!(review_queue_count(&conn).unwrap(), 3);

        conn.execute("DROP INDEX idx_documents_reviewed", [])
            .unwrap();
        assert_eq!(queued(&conn), vec![1, 3, 4], "same ids, same order");
        assert_eq!(review_queue_count(&conn).unwrap(), 3);
    }

    /// A DELETE is the other way a tag loses its last document, and it is the one the filing writer
    /// can never see: `document_tags` cascades off `documents`, so afterwards nothing is left to say
    /// which tags just went empty. It used to be healed by accident — every filing anywhere in the
    /// store ran a whole-registry sweep — which meant the label stayed in every picker AND in the
    /// CACHED filing prompt, where the model reads it as established vocabulary and re-mints it onto
    /// new documents, until something unrelated was next filed. It must be gone at the deletion.
    #[test]
    fn deleting_the_last_document_carrying_a_label_retires_it_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        conn.execute("INSERT INTO projects(name) VALUES ('Kept Triage')", [])
            .unwrap();
        let seed = |vp: &str, hash: &str| -> i64 {
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash, project) \
                 VALUES (?1,'T',?2,'Alpha')",
                params![vp, hash],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let a = seed("a.md", "ha");
        let b = seed("b.md", "hb");
        crate::tags::set_document_projects(&conn, a, "Alpha", &["Kept Triage".into()]).unwrap();
        crate::tags::set_document_projects(&conn, b, "Alpha", &[]).unwrap();
        crate::tags::set_document_group_tags(&conn, a, &["tax".into(), "draft".into()]).unwrap();
        crate::tags::set_document_group_tags(&conn, b, &["draft".into()]).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        delete_document(&tx, a).unwrap();
        tx.commit().unwrap();

        let names: Vec<String> = crate::tags::list_all(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(
            !names.contains(&"tax".to_string()),
            "the label only the deleted document carried is retired with it"
        );
        assert!(
            names.contains(&"draft".to_string()),
            "a label another document still carries survives"
        );
        assert!(
            names.contains(&"Alpha".to_string()),
            "and so does the project the survivor is filed in"
        );
        assert!(
            names.contains(&"Kept Triage".to_string()),
            "an empty-but-real project outlives its last document — it has a triage row"
        );
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
        // THE #281 data-loss guard. `rebuild`'s walk feeds two things: the list it re-embeds, and — via
        // `plan_reap` — the set of documents it considers still to exist. A walk that missed `chats/`
        // would not merely skip those files: it would see every chat as deleted and reap the lot on the
        // next complete pass. So assert the chats are FOUND, plaintext and encrypted alike, and that
        // each comes back under the `chats/…` relative path `documents.vault_path` stores — a bare name
        // here would resume-skip and reap by the wrong key.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join(CHATS_SUBDIR)).unwrap();
        std::fs::write(vault.join("report-01-07-2026-ff00.md"), "doc").unwrap();
        std::fs::write(
            vault
                .join(CHATS_SUBDIR)
                .join("chat-28-06-2026-abc123def456.md"),
            "chat",
        )
        .unwrap();
        std::fs::write(
            vault
                .join(CHATS_SUBDIR)
                .join("chat-28-06-2026-def456abc123.md.pmenc"),
            b"enc",
        )
        .unwrap();
        std::fs::write(vault.join("scratch.tmp"), "ignore").unwrap();

        let (files, unreadable) = walk_vault_markdown(&vault).unwrap();
        let collected: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();

        assert_eq!(unreadable, 0, "nothing unreadable ⇒ the sweep may run");
        assert!(collected.contains(&"report-01-07-2026-ff00.md".to_string()));
        assert!(collected.contains(&"chats/chat-28-06-2026-abc123def456.md".to_string()));
        assert!(collected.contains(&"chats/chat-28-06-2026-def456abc123.md.pmenc".to_string()));
        assert!(!collected.iter().any(|n| n.ends_with(".tmp")));
        assert_eq!(collected.len(), 3, "two chats + one document, not the .tmp");
    }

    #[test]
    fn vault_walk_never_descends_into_the_photo_originals() {
        // The trap that makes `MARKDOWN_SUBDIRS` an allow-list rather than a blind recursion: encryption
        // suffixes a saved photo original `h.png.pmenc`, and `is_vault_markdown` says yes to ANY `.pmenc`.
        // A walk that descended everywhere would hand a JPEG to the document pipeline on every Rebuild,
        // re-encode it as Markdown on every key change, and emit it as a `.png` full of ciphertext from
        // the plaintext export.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join(PHOTOS_SUBDIR)).unwrap();
        std::fs::create_dir_all(vault.join(CHATS_SUBDIR)).unwrap();
        std::fs::write(vault.join(PHOTOS_SUBDIR).join("abc123.png.pmenc"), b"jpeg").unwrap();
        std::fs::write(
            vault.join(CHATS_SUBDIR).join("chat-01-01-2026-a.md"),
            "chat",
        )
        .unwrap();

        let (files, _) = walk_vault_markdown(&vault).unwrap();
        let collected: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
        assert_eq!(collected, vec!["chats/chat-01-01-2026-a.md".to_string()]);
        // Guard the predicate itself, so the next person sees WHY the allow-list is load-bearing rather
        // than assuming the walk is safe because photos "obviously" aren't Markdown.
        assert!(
            is_vault_markdown(Path::new("abc123.png.pmenc")),
            "the predicate really does accept a photo original — the allow-list is the only defence"
        );
    }

    #[test]
    fn vault_walk_is_complete_when_the_chats_folder_does_not_exist_yet() {
        // A vault with no chats yet has no `chats/`. That is ABSENCE, not incompleteness — reading it as
        // a partial walk would withhold the straggler sweep on every rebuild of every new store, so a
        // document the user deleted could never be reaped.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(vault.join("report-01-07-2026-ff00.md"), "doc").unwrap();

        let (files, unreadable) = walk_vault_markdown(&vault).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            unreadable, 0,
            "a missing subfolder is not an unreadable one"
        );
        assert!(may_reap(unreadable == 0));
    }

    #[test]
    fn rel_with_name_keeps_the_folder() {
        // The one string operation behind both the key migration's rename and the plaintext export's
        // output path. Getting it wrong sends a re-keyed chat back to the vault root — where the next
        // open's relocation pass would find a file already at the destination and refuse to touch either.
        assert_eq!(
            rel_with_name("chats/a.md", "a.md.pmenc"),
            "chats/a.md.pmenc"
        );
        assert_eq!(rel_with_name("a.md", "a.md.pmenc"), "a.md.pmenc");
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
            &[],
            &["urgent".to_string()],
            Some("high"),
            true,
            "2026-06-26T00:00:00Z",
            dir.path(),
            &cipher,
            FilingActivity::Record,
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

    /// Filing a document INTO a real project appends one `kind='ingest'` activity observation keyed
    /// to that project (source_ref = the document id); filing into the pre-triage `Unsorted` bucket
    /// appends nothing. `write_document_truth` is the single organize choke-point, so this locks the
    /// per-project engagement signal (Stage-3 activity log).
    #[test]
    fn write_document_truth_logs_activity_only_for_a_real_project() {
        use crate::index_only::ManifestCipher;
        let (dir, mut conn, _vault_id, index_id) = store_with_one_of_each();
        let cipher = ManifestCipher::from_master("vault-test", &[9u8; 32]);
        let vault_dir = dir.path().join("vault");
        let markdown = crate::vault::MarkdownCipher::plaintext("vault-test");

        let file = |conn: &mut Connection, project: &str| {
            let tx = conn.transaction().unwrap();
            write_document_truth(
                &tx,
                &vault_dir,
                &markdown,
                index_id,
                project,
                &[],
                &[],
                None,
                true,
                "2026-06-26T00:00:00Z",
                dir.path(),
                &cipher,
                FilingActivity::Record,
            )
            .unwrap();
            tx.commit().unwrap();
        };

        // File into a real project → exactly one 'ingest' observation, keyed by name, ref = doc id.
        file(&mut conn, "Project X");
        let rows: Vec<(String, String, Option<i64>)> = {
            let mut s = conn
                .prepare("SELECT project, kind, source_ref FROM project_activity")
                .unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![(
                "Project X".to_string(),
                "ingest".to_string(),
                Some(index_id)
            )]
        );

        // Re-file into the pre-triage bucket → no new observation (Unsorted isn't a chosen project).
        file(&mut conn, "Unsorted");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_activity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "filing into Unsorted logs nothing");
    }

    /// B6-6: a bulk identity rewrite (entity rename/merge) files each linked doc into the new
    /// canonical name via `FilingActivity::Suppress`, so NO per-doc `ingest` observation is logged —
    /// renaming a 200-doc project must not read as 200 filings in one instant on the Stage-4 heat map.
    #[test]
    fn write_document_truth_suppresses_activity_when_asked() {
        use crate::index_only::ManifestCipher;
        let (dir, mut conn, _vault_id, index_id) = store_with_one_of_each();
        let cipher = ManifestCipher::from_master("vault-test", &[9u8; 32]);
        let vault_dir = dir.path().join("vault");
        let markdown = crate::vault::MarkdownCipher::plaintext("vault-test");

        let tx = conn.transaction().unwrap();
        write_document_truth(
            &tx,
            &vault_dir,
            &markdown,
            index_id,
            "Project X", // a real project — would log under Record, but we Suppress
            &[],
            &[],
            None,
            true,
            "2026-06-26T00:00:00Z",
            dir.path(),
            &cipher,
            FilingActivity::Suppress,
        )
        .unwrap();
        tx.commit().unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_activity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a suppressed filing logs no engagement");
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
            linked_projects: &[],
            tags: &tags,
            importance: Some("high"),
            last_activity: "2026-06-17T00:00:00.000Z",
            reviewed: true,
            photo: None,
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
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
        //
        // The `vault_path` is the PREDICTED one an encrypted vault stores (`ingest_photo` writes the
        // block before the copy exists), so this also pins that the prediction survives the
        // render → parse → row round trip a Rebuild performs, `.pmenc` suffix and all.
        let cipher = MarkdownCipher::for_test_encrypted("vault-1");
        let rec = PhotoRecord {
            source_path: Some("/imgs/Screenshot 2026-03-12.png".into()),
            source_type: PhotoSourceType::Screenshot,
            capture_date: "2026-03-12".into(),
            file_hash: "deadbeefcafe".into(),
            ocr_text: Some("Total due £42.00".into()),
            saved_to_vault: true,
            vault_path: Some(photo_copy_rel_path(&cipher, "deadbeefcafe", Some("png"))),
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
            linked_projects: &[],
            tags: &[],
            importance: None,
            last_activity: "2026-06-28T00:00:00.000Z",
            reviewed: false,
            photo: Some(&rec),
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
        };
        let rendered = render_markdown(&front, &body);
        let (fields, parsed_body) = parse_frontmatter(&rendered).unwrap();

        // The marker drives rebuild's source_type='photo' branch.
        assert_eq!(fields.get("source_type").map(String::as_str), Some("photo"));
        let recovered = photo_from_fields(&fields, &rec.file_hash, parsed_body)
            .expect("a photo block reconstructs a record");
        assert_eq!(recovered, rec, "the photos record round-trips exactly");

        // …and the name that came back out is the one a copy on disk actually answers to. If the
        // prediction ever drifted from what `copy_original_to_vault` writes, the front-matter would
        // point at a file no heal probe could find — and, since the on-disk name is the AAD stem, at
        // a blob that could not be decrypted even if it were found.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join(PHOTOS_SUBDIR)).unwrap();
        std::fs::write(vault.join(recovered.vault_path.as_deref().unwrap()), b"png").unwrap();
        assert_eq!(
            find_saved_photo_copy(&vault, &rec.file_hash, Some("png")),
            recovered.vault_path,
            "the round-tripped path is exactly the name the heal probe looks for"
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
                linked_projects: &[],
                tags: &[],
                importance: None,
                last_activity: "",
                reviewed: false,
                photo: None,
                spreadsheet: None,
                chat: None,
                source_id: None,
                external_ref: None,
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
            linked_projects: &[],
            tags: &[],
            importance: None,
            last_activity: "2026-07-01T00:00:00.000Z",
            reviewed: false,
            photo: None,
            spreadsheet: Some(&rec),
            chat: None,
            source_id: None,
            external_ref: None,
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
                linked_projects: &[],
                tags: &[],
                importance: None,
                last_activity: "",
                reviewed: false,
                photo: None,
                spreadsheet: None,
                chat: None,
                source_id: None,
                external_ref: None,
            },
            "just text",
        );
        let (pf, _) = parse_frontmatter(&plain).unwrap();
        assert!(spreadsheet_from_fields(&pf).is_none());
    }

    #[test]
    fn promoted_spreadsheet_frontmatter_round_trips_the_connector_claim() {
        // A promoted Sheet must keep its `source_id` (+ Drive link) through the vault front-matter, so a
        // Rebuild reconstructs the claim and the next sync recognises the still-present source instead of
        // re-indexing it as an index-only duplicate.
        let rec = SpreadsheetRecord {
            sheet_count: 2,
            total_rows: 30,
            chunked_rows: 30,
        };
        let front = Frontmatter {
            title: "Budget",
            source_path: "",
            ext: Some("xlsx"),
            content_hash: "abc",
            created_at: "2026-07-01",
            ingested_at: "2026-07-03T00:00:00.000Z",
            project: "Finances",
            linked_projects: &[],
            tags: &[],
            importance: None,
            last_activity: "2026-07-03T00:00:00.000Z",
            reviewed: true,
            photo: None,
            spreadsheet: Some(&rec),
            chat: None,
            source_id: Some("gdrive:a@b.com:F1"),
            external_ref: Some("https://docs.google.com/spreadsheets/d/F1/edit"),
        };
        let rendered = render_markdown(&front, "## Sheet: S\n\n### Overview\n\ntext\n");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();

        // The spreadsheet marker still round-trips, and the connector claim survives for the rebuild arm.
        assert_eq!(
            fields.get("source_type").map(String::as_str),
            Some("spreadsheet")
        );
        assert_eq!(
            fields.get("source_id").map(String::as_str),
            Some("gdrive:a@b.com:F1")
        );
        assert_eq!(
            fields.get("external_ref").map(String::as_str),
            Some("https://docs.google.com/spreadsheets/d/F1/edit")
        );

        // A locally-ingested spreadsheet writes NEITHER claim line (so it stays a pure local document).
        // Done before the `rec`-consuming assert below, since this still borrows `front` (hence `rec`).
        let local = render_markdown(
            &Frontmatter {
                source_id: None,
                external_ref: None,
                ..front
            },
            "## Sheet: S\n",
        );
        assert!(!local.contains("source_id:"));
        assert!(!local.contains("external_ref:"));

        // The counts round-trip too (the claim lines don't disturb the satellite block). Last, because it
        // moves `rec`.
        assert_eq!(spreadsheet_from_fields(&fields), Some(rec));
    }

    #[test]
    fn photo_dedupe_save_flips_saved_to_vault_by_file_hash() {
        // The dedupe-hit path of `ingest_photo`: when the user re-drops an already-ingested image
        // with "save a copy" newly checked, we skip re-indexing but still record the copy, keyed by
        // file_hash. Drives the REAL seam (`record_saved_photo_copy`) rather than a hand-copied
        // UPDATE, so it guards the statement against the migrated schema AND cannot pass while the
        // production path does something else.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TEST_KEY).unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cipher = MarkdownCipher::for_test_encrypted("vault-1");
        write_photo_vault_file(&vault, &cipher, "photo.md.pmenc", "imghash", None);

        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type) \
             VALUES ('photo.md.pmenc','Screenshot','imghash','photo')",
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

        record_saved_photo_copy(&conn, &vault, &cipher, "imghash", "photos/imghash.png").unwrap();

        let (saved, path): (i64, Option<String>) = conn
            .query_row(
                "SELECT saved_to_vault, vault_path FROM photos WHERE file_hash = 'imghash'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(saved, 1, "the opt-in flag is now set");
        assert_eq!(path.as_deref(), Some("photos/imghash.png"));

        // The half that was missing, and the half that matters: the vault file — the truth a
        // Rebuild rebuilds the row from — learned about the copy too. Without this assert the row
        // above is a promise the next Rebuild silently breaks.
        let raw = cipher.read(&vault.join("photo.md.pmenc")).unwrap();
        let (fields, body) = parse_frontmatter(&raw).unwrap();
        let rebuilt = photo_from_fields(&fields, "imghash", body).expect("still a photo document");
        assert!(
            rebuilt.saved_to_vault,
            "a Rebuild must reconstruct the row KNOWING the copy exists"
        );
        assert_eq!(rebuilt.vault_path.as_deref(), Some("photos/imghash.png"));
    }

    #[test]
    fn recording_a_saved_copy_leaves_the_rest_of_the_document_alone() {
        // `rewrite_photo_vault_block` rebuilds the whole front-matter to change one field, so the
        // risk it carries is silently dropping the fields it doesn't own — organisation metadata a
        // Rebuild would then lose. Mirror of `rewrite_vault_metadata`'s photo-block preservation.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cipher = MarkdownCipher::for_test_encrypted("vault-1");
        write_photo_vault_file(&vault, &cipher, "photo.md.pmenc", "imghash", None);

        rewrite_photo_vault_block(&vault, &cipher, "photo.md.pmenc", "photos/imghash.png").unwrap();

        let raw = cipher.read(&vault.join("photo.md.pmenc")).unwrap();
        let (fields, body) = parse_frontmatter(&raw).unwrap();
        assert_eq!(fields.get("project").map(String::as_str), Some("Receipts"));
        assert_eq!(parse_yaml_list(fields.get("tags").unwrap()), ["scan"]);
        assert_eq!(nullable(fields.get("importance")).as_deref(), Some("high"));
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("true"));
        let rebuilt = photo_from_fields(&fields, "imghash", body).unwrap();
        assert_eq!(rebuilt.ocr_text.as_deref(), Some("Total due"));
        assert_eq!(rebuilt.width, Some(100), "the rest of the photo block too");
        assert_eq!(rebuilt.source_type, PhotoSourceType::Screenshot);
    }

    /// A photo's vault file as `ingest_photo` writes it, with `saved_to_vault` under the caller's
    /// control — the starting state for the dedupe-hit and heal tests below.
    fn write_photo_vault_file(
        vault: &Path,
        cipher: &MarkdownCipher,
        name: &str,
        hash: &str,
        saved: Option<&str>,
    ) {
        let rec = PhotoRecord {
            source_path: Some("/imgs/Screenshot.png".into()),
            source_type: PhotoSourceType::Screenshot,
            capture_date: "2026-03-12".into(),
            file_hash: hash.into(),
            ocr_text: Some("Total due".into()),
            saved_to_vault: saved.is_some(),
            vault_path: saved.map(String::from),
            width: Some(100),
            height: Some(200),
            lat: None,
            lon: None,
        };
        let body =
            photos::photo_markdown(rec.source_type, &rec.capture_date, None, None, "Total due");
        let title = photos::photo_title(rec.source_type, &rec.capture_date);
        let front = Frontmatter {
            title: &title,
            source_path: rec.source_path.as_deref().unwrap(),
            ext: Some("png"),
            content_hash: hash,
            created_at: &rec.capture_date,
            ingested_at: "2026-06-28T00:00:00.000Z",
            project: "Receipts",
            linked_projects: &["Tax 2026".to_string()],
            tags: &["scan".to_string()],
            importance: Some("high"),
            last_activity: "2026-06-28T00:00:00.000Z",
            reviewed: true,
            photo: Some(&rec),
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
        };
        cipher
            .write_to(&vault.join(name), &render_markdown(&front, &body))
            .unwrap();
    }

    #[test]
    fn a_rebuild_heals_a_photo_whose_front_matter_lost_its_copy() {
        // Already-divergent vaults: a pre-v3.19.2 dedupe-hit save flipped only the row, so the copy
        // exists on disk while the block denies it. Fixing the write path doesn't help those users —
        // their NEXT rebuild would still reset the flag and orphan the file. Heal at rebuild time.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join(PHOTOS_SUBDIR)).unwrap();
        let cipher = MarkdownCipher::for_test_encrypted("vault-1");
        write_photo_vault_file(&vault, &cipher, "photo.md.pmenc", "imghash", None);
        cipher
            .write_bytes_to(
                &vault.join(PHOTOS_SUBDIR).join("imghash.png.pmenc"),
                b"the copy the row knew about and the file did not",
            )
            .unwrap();

        let mut photo = PhotoRecord {
            source_path: None,
            source_type: PhotoSourceType::Screenshot,
            capture_date: "2026-03-12".into(),
            file_hash: "imghash".into(),
            ocr_text: None,
            saved_to_vault: false, // what the stale block says
            vault_path: None,
            width: None,
            height: None,
            lat: None,
            lon: None,
        };
        assert!(
            heal_photo_copy(&mut photo, &vault, &cipher, "photo.md.pmenc", Some("png")).unwrap(),
            "a copy on disk that the block denies must be healed"
        );
        assert!(photo.saved_to_vault);
        assert_eq!(
            photo.vault_path.as_deref(),
            Some("photos/imghash.png.pmenc")
        );

        // Durable: the correction is written back, so the next rebuild reads it as truth instead of
        // re-deriving the heal (and so a later `find` failure can't silently undo it).
        let raw = cipher.read(&vault.join("photo.md.pmenc")).unwrap();
        let (fields, body) = parse_frontmatter(&raw).unwrap();
        let rebuilt = photo_from_fields(&fields, "imghash", body).unwrap();
        assert!(
            rebuilt.saved_to_vault,
            "the heal is written back to the vault"
        );
        assert_eq!(
            rebuilt.vault_path.as_deref(),
            Some("photos/imghash.png.pmenc")
        );
    }

    #[test]
    fn healing_never_invents_a_copy_or_overrules_a_block_that_knows() {
        // The two ways a heal could do harm: fabricating a vault_path for a copy that isn't there,
        // and second-guessing a block that already states its own truth.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join(PHOTOS_SUBDIR)).unwrap();
        let cipher = MarkdownCipher::for_test_encrypted("vault-1");
        write_photo_vault_file(&vault, &cipher, "photo.md.pmenc", "imghash", None);

        let base = PhotoRecord {
            source_path: None,
            source_type: PhotoSourceType::Screenshot,
            capture_date: "2026-03-12".into(),
            file_hash: "imghash".into(),
            ocr_text: None,
            saved_to_vault: false,
            vault_path: None,
            width: None,
            height: None,
            lat: None,
            lon: None,
        };

        // No copy on disk: stays exactly as the block said.
        let mut no_copy = base.clone();
        assert!(
            !heal_photo_copy(&mut no_copy, &vault, &cipher, "photo.md.pmenc", Some("png")).unwrap()
        );
        assert!(!no_copy.saved_to_vault && no_copy.vault_path.is_none());

        // A block that already knows is never touched — even though a copy IS present.
        cipher
            .write_bytes_to(&vault.join(PHOTOS_SUBDIR).join("imghash.png.pmenc"), b"x")
            .unwrap();
        let mut knows = PhotoRecord {
            saved_to_vault: true,
            vault_path: Some("photos/somewhere-else.png".into()),
            ..base
        };
        assert!(
            !heal_photo_copy(&mut knows, &vault, &cipher, "photo.md.pmenc", Some("png")).unwrap()
        );
        assert_eq!(
            knows.vault_path.as_deref(),
            Some("photos/somewhere-else.png"),
            "the block's own path is authoritative and must not be rewritten"
        );
    }

    #[test]
    fn find_saved_photo_copy_finds_a_copy_saved_under_either_policy() {
        // A photo keeps the name it was saved under: `convert_photo_originals` re-encodes in place
        // without renaming, so a vault whose encryption later flipped holds a `.pmenc` name that no
        // longer describes the bytes. The heal probe must not care.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join(PHOTOS_SUBDIR)).unwrap();
        assert_eq!(find_saved_photo_copy(&vault, "h", Some("png")), None);

        std::fs::write(vault.join(PHOTOS_SUBDIR).join("h.png"), b"x").unwrap();
        assert_eq!(
            find_saved_photo_copy(&vault, "h", Some("png")).as_deref(),
            Some("photos/h.png")
        );

        let dir2 = tempfile::tempdir().unwrap();
        let vault2 = dir2.path().join("vault");
        std::fs::create_dir_all(vault2.join(PHOTOS_SUBDIR)).unwrap();
        std::fs::write(vault2.join(PHOTOS_SUBDIR).join("h.png.pmenc"), b"x").unwrap();
        assert_eq!(
            find_saved_photo_copy(&vault2, "h", Some("png")).as_deref(),
            Some("photos/h.png.pmenc"),
            "an original saved while the vault was encrypted is still found after make-private"
        );
        // A hash with no copy on disk stays unhealed rather than inventing a path.
        assert_eq!(find_saved_photo_copy(&vault2, "other", Some("png")), None);
    }

    #[test]
    fn photo_copy_rel_path_is_byte_exactly_what_the_copy_writes() {
        // THE load-bearing invariant of doing the copy after the index commits: the `photos` row and
        // the front-matter are written from the PREDICTION, so a prediction that differs from the
        // written name by one character does not merely dangle — the name is the AAD stem
        // (`MarkdownCipher::aad_stem`), so the blob would not decrypt even once found. Both policies,
        // because the `.pmenc` suffix is exactly where the two expressions could drift.
        for cipher in [
            MarkdownCipher::plaintext("vault-1"),
            MarkdownCipher::for_test_encrypted("vault-1"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let vault = dir.path().join("vault");
            std::fs::create_dir_all(&vault).unwrap();
            let bytes = b"\x89PNG\r\n\x1a\n not really a png".to_vec();

            let predicted = photo_copy_rel_path(&cipher, "abc123", Some("png"));
            let written = copy_original_to_vault(&vault, &cipher, &bytes, "abc123", Some("png"))
                .expect("the copy is written");

            assert_eq!(
                predicted, written,
                "prediction and write are one expression"
            );
            assert!(
                vault.join(&predicted).is_file(),
                "the predicted path names a real file: {predicted}"
            );
            // The whole point: readable back. Under encryption this only holds if the AAD stem the
            // writer used is the stem the predicted name yields.
            assert_eq!(
                cipher.read_bytes(&vault.join(&predicted)).unwrap(),
                bytes,
                "a drifted name yields an undecryptable blob, not just a wrong pointer"
            );
            // And the heal probe finds it under the same name, whichever policy wrote it.
            assert_eq!(
                find_saved_photo_copy(&vault, "abc123", Some("png")).as_deref(),
                Some(predicted.as_str())
            );
        }
    }

    #[test]
    fn a_photo_that_lost_both_its_ocr_and_its_copy_reports_both() {
        // `Outcome::Indexed.warning` is single-valued and now has two producers, so a plain assignment
        // would silently drop the OCR note when the vault copy fails on the same photo.
        assert_eq!(join_warnings([None, None]), None);
        assert_eq!(
            join_warnings([Some("no OCR".into()), None]).as_deref(),
            Some("no OCR")
        );
        assert_eq!(
            join_warnings([None, Some("no copy".into())]).as_deref(),
            Some("no copy")
        );
        assert_eq!(
            join_warnings([Some("no OCR".into()), Some("no copy".into())]).as_deref(),
            Some("no OCR; no copy"),
            "both survive, in the order they happened"
        );
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
            linked_projects: &[],
            tags: &[],
            importance: None,
            last_activity: "",
            reviewed: false,
            photo: None,
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
        };
        let rendered = render_markdown(&front, "x");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();
        assert!(parse_yaml_list(fields.get("tags").unwrap()).is_empty());
        assert_eq!(nullable(fields.get("importance")), None);
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("false"));
    }

    #[test]
    fn a_project_name_with_a_comma_survives_the_membership_list() {
        // The flow list is comma-separated, and `parse_yaml_list` used to split on every comma —
        // fine for tags (the editor strips them) but wrong for project names, which are names the
        // user typed. "Atlas, Inc." came back as `Atlas` and `Inc."`: corrupted, not lost, so a
        // happy-path test with simple names would never have shown it.
        let linked = vec!["Atlas, Inc.".to_string(), "R&D".to_string()];
        let front = Frontmatter {
            title: "Contract",
            source_path: "",
            ext: Some("pdf"),
            content_hash: "h",
            created_at: "2026-06-01",
            ingested_at: "2026-06-01",
            project: "Legal, EU",
            linked_projects: &linked,
            tags: &[],
            importance: None,
            last_activity: "2026-06-01",
            reviewed: true,
            photo: None,
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
        };
        let rendered = render_markdown(&front, "body");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();
        assert_eq!(fields.get("project").map(String::as_str), Some("Legal, EU"));
        assert_eq!(
            parse_yaml_list(fields.get("linked_projects").unwrap()),
            linked
        );
    }

    #[test]
    fn a_hand_edited_file_repeating_the_home_yields_no_self_link() {
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "linked_projects".to_string(),
            r#"["sales", "Ops"]"#.to_string(),
        );
        assert_eq!(linked_projects_from_fields(&fields, "Sales"), ["Ops"]);
        // And a file written before #275 has no such key at all.
        assert!(linked_projects_from_fields(&std::collections::HashMap::new(), "Sales").is_empty());
    }

    #[test]
    fn filing_an_unreviewed_document_does_not_link_it_to_the_inbox() {
        // The co-signer for the `linked_projects` read-order fix, driven through the real seam in
        // the exact order `commit_review` drives it: read the memberships, THEN write the truth
        // (which is what moves `documents.project`). Asserting on the vault file rather than on the
        // join is the point — the file is what a Rebuild believes, so a regression here is
        // permanent rather than a cache that heals.
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let mut conn = crate::db::open(&dir.path().join("t.sqlite"), key).unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cipher = MarkdownCipher::plaintext("test-vault");
        let manifest = crate::index_only::ManifestCipher::from_master("v", &[7u8; 32]);

        let born = "---\ntitle: Receipt\nsource_path: \next: pdf\ncontent_hash: h\n\
                    created_at: 2026-01-01\ningested_at: 2026-01-01\nproject: \"Unsorted\"\n\
                    tags: []\nimportance: null\nlast_activity: 2026-01-01\nreviewed: false\n\
                    ---\n\nBody.\n";
        cipher.write_to(&vault.join("r.md"), born).unwrap();
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, tags, reviewed) \
             VALUES (1, 'r.md', 'Receipt', 'h', 'Unsorted', '[]', 0)",
            [],
        )
        .unwrap();
        // Ingest interns the inbox as a real project tag, which is what made the old home visible
        // to the membership union in the first place.
        {
            let tx = conn.transaction().unwrap();
            crate::tags::set_document_projects(&tx, 1, "Unsorted", &[]).unwrap();
            tx.commit().unwrap();
        }

        // Approve it into "Atlas" — the two lines `commit_review` runs, in its order.
        {
            let tx = conn.transaction().unwrap();
            let linked = crate::tags::linked_projects(&tx, 1, "Atlas").unwrap();
            write_document_truth(
                &tx,
                &vault,
                &cipher,
                1,
                "Atlas",
                &linked,
                &[],
                None,
                true,
                "2026-06-20",
                dir.path(),
                &manifest,
                FilingActivity::Record,
            )
            .unwrap();
            tx.commit().unwrap();
        }

        let raw = cipher.read(&vault.join("r.md")).unwrap();
        let (fields, _) = parse_frontmatter(&raw).unwrap();
        assert_eq!(nullable(fields.get("project")).as_deref(), Some("Atlas"));
        assert!(
            linked_projects_from_fields(&fields, "Atlas").is_empty(),
            "a document approved out of the inbox must not stay a member of it"
        );
        drop(fields);

        // And the join agrees — the inbox tag is gone, not merely absent from the file.
        let still: Vec<String> = {
            let mut s = conn
                .prepare(
                    "SELECT t.name FROM document_tags dt JOIN tags t ON t.id = dt.tag_id \
                     WHERE dt.document_id = 1 AND t.kind = 'project' ORDER BY t.name",
                )
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(still, ["Atlas"]);
    }

    #[test]
    fn rewrite_vault_metadata_round_trips_project_membership() {
        // The INVARIANTS I-03 co-signer for the new key. `rewrite_vault_metadata` REBUILDS the file
        // from a fresh `Frontmatter`, so a key it does not re-emit is silently deleted by the next
        // unrelated organisation write — how the chat identity block was lost until 3.81.2 and the
        // photo copy flag until 3.19.2. This proves the write lands AND that a later edit which
        // changes something else entirely leaves it alone.
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let mut conn = crate::db::open(&dir.path().join("t.sqlite"), key).unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cipher = MarkdownCipher::plaintext("test-vault");

        // A file written by a build that predates #275: no `linked_projects:` line at all.
        let born = "---\ntitle: Q3\nsource_path: \next: md\ncontent_hash: h\n\
                    created_at: 2026-01-01\ningested_at: 2026-01-01\nproject: \"Sales\"\n\
                    tags: []\nimportance: null\nlast_activity: 2026-01-01\nreviewed: true\n\
                    ---\n\nBody.\n";
        cipher.write_to(&vault.join("q3.md"), born).unwrap();
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, tags, reviewed) \
             VALUES (1, 'q3.md', 'Q3', 'h', 'Sales', '[]', 1)",
            [],
        )
        .unwrap();

        // Link it into two more projects, through the real filing seam so the membership join is
        // written alongside the file exactly as it is in production.
        let manifest = crate::index_only::ManifestCipher::from_master("v", &[7u8; 32]);
        {
            let tx = conn.transaction().unwrap();
            write_document_truth(
                &tx,
                &vault,
                &cipher,
                1,
                "Sales",
                &["Marketing".into(), "Atlas, Inc.".into()],
                &[],
                None,
                true,
                "2026-06-20",
                dir.path(),
                &manifest,
                FilingActivity::Suppress,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let raw = cipher.read(&vault.join("q3.md")).unwrap();
        let (fields, _) = parse_frontmatter(&raw).unwrap();
        assert_eq!(
            linked_projects_from_fields(&fields, "Sales"),
            ["Marketing", "Atlas, Inc."]
        );
        drop(fields);

        // Now an edit that changes only the importance. The membership must survive it, which is
        // the whole point: it is passed as an argument, so a caller that forgot would blank it.
        {
            let tx = conn.transaction().unwrap();
            // What every rewrite path does: re-derive the membership from the join rather than
            // being told it. A caller that passed `&[]` here would silently unlink the document.
            let linked = crate::tags::linked_projects(&tx, 1, "Sales").unwrap();
            write_document_truth(
                &tx,
                &vault,
                &cipher,
                1,
                "Sales",
                &linked,
                &[],
                Some("high"),
                true,
                "2026-06-21",
                dir.path(),
                &manifest,
                FilingActivity::Suppress,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let raw = cipher.read(&vault.join("q3.md")).unwrap();
        let (fields, body) = parse_frontmatter(&raw).unwrap();
        assert_eq!(nullable(fields.get("importance")).as_deref(), Some("high"));
        let mut survived = linked_projects_from_fields(&fields, "Sales");
        survived.sort();
        assert_eq!(
            survived,
            ["Atlas, Inc.", "Marketing"],
            "an unrelated organisation edit must not unlink the document"
        );
        assert_eq!(body.trim_end(), "Body.");
    }

    #[test]
    fn rewriting_a_photo_block_leaves_project_membership_alone() {
        // The second rebuild-the-whole-file writer, which fires on a dedupe-hit photo save and
        // during every Rebuild via `heal_photo_copy`. It reads the organisation fields back OUT of
        // the file to preserve them, so the new key needs the same treatment or photo documents
        // alone would lose their memberships — during the operation users trust to be lossless.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cipher = MarkdownCipher::for_test_encrypted("vault-1");
        write_photo_vault_file(&vault, &cipher, "photo.md.pmenc", "imghash", None);

        rewrite_photo_vault_block(&vault, &cipher, "photo.md.pmenc", "photos/imghash.png").unwrap();

        let raw = cipher.read(&vault.join("photo.md.pmenc")).unwrap();
        let (fields, _) = parse_frontmatter(&raw).unwrap();
        assert_eq!(
            linked_projects_from_fields(&fields, "Receipts"),
            ["Tax 2026"]
        );
    }

    #[test]
    fn rewrite_vault_metadata_preserves_a_chat_s_identity() {
        // THE regression pin for the 3.81.2 fix. Every organisation write — approving a chat in
        // Review, editing its project, renaming/merging the project that owns it — funnels through
        // here. Before the fix this rebuilt the file from a `Frontmatter` with no chat arm, silently
        // deleting `source_type: chat` + the `chat_*` lines; the next Rebuild then stopped matching
        // `is_chat_vault_file` and re-ingested the conversation as an ordinary document.
        let dir = tempfile::tempdir().unwrap();
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let mut conn = crate::db::open(&dir.path().join("t.sqlite"), key).unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cipher = MarkdownCipher::plaintext("test-vault");

        // A chat file exactly as `chat::render_chat_frontmatter` writes it at birth.
        let born = "---\ntitle: A chat\ncontent_hash: h\nsource_type: chat\n\
                    chat_conversation_id: 42\nchat_scope: project\nchat_source_id: chat:42\n\
                    project: Atlas\ntags: []\nimportance: high\nreviewed: true\n\
                    created_at: 2026-01-01\ningested_at: 2026-01-01\nlast_activity: 2026-01-01\n\
                    ---\n\n**You:** hi\n\n**PM:** hello\n";
        cipher.write_to(&vault.join("chat.md"), born).unwrap();
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, tags, reviewed, source_type) \
             VALUES (1, 'chat.md', 'A chat', 'h', 'Atlas', '[]', 1, 'chat')",
            [],
        )
        .unwrap();

        {
            let tx = conn.transaction().unwrap();
            rewrite_vault_metadata(
                &tx,
                &vault,
                &cipher,
                1,
                "Renamed Project",
                &[],
                &["notes".into()],
                Some("high"),
                true,
                "2026-06-20",
            )
            .unwrap();
            tx.commit().unwrap();
        }

        let after = cipher.read(&vault.join("chat.md")).unwrap();
        let (fields, body) = parse_frontmatter(&after).unwrap();
        // The organisation edit landed...
        assert_eq!(
            fields.get("project").map(String::as_str),
            Some("Renamed Project")
        );
        // ...and the identity survived it.
        assert_eq!(
            fields.get("source_type").map(String::as_str),
            Some(SOURCE_TYPE_CHAT),
            "the rewrite must not demote a chat to an ordinary document"
        );
        assert_eq!(
            fields.get("chat_conversation_id").map(String::as_str),
            Some("42"),
            "rebuild_chat cannot find the session without this"
        );
        assert_eq!(
            fields.get("chat_scope").map(String::as_str),
            Some("project")
        );
        assert_eq!(
            fields.get("chat_source_id").map(String::as_str),
            Some("chat:42")
        );
        // The transcript itself is untouched.
        assert!(body.contains("**You:** hi"));
        assert!(body.contains("**PM:** hello"));
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
            linked_projects: &[],
            tags: &[],
            importance: None,
            last_activity: "",
            reviewed: false,
            photo: None,
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
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
                    &[],
                    &["tax".into()],
                    Some("high"),
                    true,
                    "2026-06-20",
                )
                .unwrap(),
            );
            assert!(
                rewrite_vault_metadata(
                    &tx,
                    &vault,
                    &cipher,
                    2,
                    "X",
                    &[],
                    &[],
                    None,
                    true,
                    "2026-06-20"
                )
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
            linked_projects: &[],
            tags: &[],
            importance: None,
            last_activity: "",
            reviewed: false,
            photo: None,
            spreadsheet: None,
            chat: None,
            source_id: None,
            external_ref: None,
        };
        let rendered = render_markdown(&front, "body");
        let (fields, _) = parse_frontmatter(&rendered).unwrap();
        // The forged lines stayed inside the title value; they didn't take effect.
        assert_eq!(fields.get("project").map(String::as_str), Some("Unsorted"));
        assert_eq!(fields.get("reviewed").map(|s| s.trim()), Some("false"));
    }
}
