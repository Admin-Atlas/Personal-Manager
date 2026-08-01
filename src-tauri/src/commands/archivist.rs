// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The index lifecycle end to end: the sidecar, ingest, rebuild/resume, the document
//! readouts, and the retrieval explain/diagnose + relevance-feedback instrumentation.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::blocking::spawn_blocking_result;
use crate::error::{Error, Result};
use crate::ingest::{self, Document, IngestEvent};
use crate::llm_gateway::{self, Role};
use crate::retrieval_config::RetrievalConfig;
use crate::retrieval_diag;
use crate::retrieval_feedback;
use crate::sidecar::SidecarStatus;
use crate::{chat, db, index_only, pathguard, paths, AppState, BusyGuard};

use super::connectors::reindex_index_only_core;

/// Marker for a rebuild started but not cleanly finished (crash-resume) — the ingest sibling of
/// `DRIVE_SYNC_PENDING_KEY`. Written before the rebuild's first destructive statement and cleared
/// only on success, so a value surviving a restart means the app closed mid-rebuild and the index
/// is partial. `resume_rebuild` picks it up on launch.
///
/// Unlike a connector resume, this one restarts from zero rather than continuing: rebuild drops the
/// index and re-ingests with no per-document checkpoint. That is still strictly better than leaving
/// a half-built index (it is already dropped; it MUST be rebuilt) — but it is a weaker guarantee
/// than the connectors', whose resume only does the work that was left.
const REBUILD_PENDING_KEY: &str = "rebuild_pending";

/// Delete ONE document (#575): its index rows, and the file behind it where PM owns one.
///
/// The three source kinds are genuinely different deletions, which is why this dispatches rather
/// than doing one thing:
///
/// * **A chat** is routed to the conversation delete instead. `chat_sessions.document_id` is
///   `ON DELETE SET NULL`, so purging the document alone would leave a live conversation whose
///   transcript index had silently vanished, plus an orphaned vault file. A saved chat and its
///   document are one object to the user, so deleting either deletes both.
/// * **An index-only document is a POINTER** at a file in Drive/OneDrive/a watched local folder. PM
///   drops its own row and its `.pmindex` manifest entry; the file at the source is never touched.
/// * **Everything else is a document PM holds the file for** — a plain vault document, a photo, a
///   spreadsheet — and loses its `documents`/`chunks` rows AND the Markdown behind it. For a photo
///   saved with "keep a copy", the original in `vault/photos/` goes with it: it exists only because
///   this document does, and the `photos` row that records where it is cascades away with the
///   delete, so nothing would ever find it again.
///
/// Side effects land only AFTER the commit — the same rule `MutationFiles` encodes for project
/// deletion: a file or manifest entry that outlives its row is harmless and self-healing, whereas
/// removing either before a failed commit strands the database pointing at truth that is gone.
#[tauri::command]
pub fn delete_document(state: State<'_, AppState>, document_id: i64) -> Result<()> {
    let (vault_dir, _cipher) = state.markdown_io()?;
    let (vault_root, _rules_cipher) = state.rules_io()?;
    let (_, manifest_cipher) = state.manifest_io()?;
    let conn = state.conn()?;

    // A chat document belongs to a conversation — delete that instead (see above).
    let conversation_id: Option<i64> = conn
        .query_row(
            "SELECT conversation_id FROM chat_sessions WHERE document_id = ?1",
            params![document_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(cid) = conversation_id {
        return chat::delete_conversation_inner(&conn, &vault_dir, cid);
    }

    let (vault_path, source_type, source_id): (Option<String>, Option<String>, Option<String>) =
        conn.query_row(
            "SELECT vault_path, source_type, source_id FROM documents WHERE id = ?1",
            params![document_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| Error::Other("that document no longer exists".into()))?;

    // Read BEFORE the delete — the `photos` row cascades away with the document.
    let photo_original = ingest::saved_photo_original(&conn, document_id)?;
    let owns_file = ingest::owns_a_vault_file(source_type.as_deref());

    let tx = conn.unchecked_transaction()?;
    ingest::delete_document(&tx, document_id)?;
    tx.commit()?;

    if owns_file {
        for rel in [vault_path, photo_original].into_iter().flatten() {
            if !rel.trim().is_empty() {
                let _ = std::fs::remove_file(vault_dir.join(rel));
            }
        }
    } else if let Some(sid) = source_id.as_deref().filter(|s| !s.trim().is_empty()) {
        let _ = index_only::forget_source(&vault_root, &manifest_cipher, sid);
    }
    Ok(())
}

/// In-chat "Retrieval explain" (card 7H): the same instrumented read the Developer-mode panel runs,
/// surfaced to graduated users so they can see which chunks a query retrieves and how they scored.
/// `k` defaults to the user's saved retrieval depth — so the panel opens showing what a real chat
/// turn would retrieve — while the live slider passes an explicit override to preview a different
/// candidate pool without committing it. Strictly read-only; delegates to the shared helper.
#[tauri::command]
pub async fn retrieval_explain(
    app: AppHandle,
    query: String,
    project: Option<String>,
    k: Option<usize>,
    // The chat the panel sits under, so the explained pool carries the same in-window chat dedup a
    // real turn here would apply.
    conversation_id: Option<i64>,
) -> Result<crate::commands_dev::DevRetrievalExplain> {
    spawn_blocking_result("retrieval explain", move || {
        let state = app.state::<AppState>();
        let k = match k {
            Some(k) => k,
            None => {
                let conn = state.conn()?;
                crate::db::retrieval_k(&conn)
            }
        };
        crate::commands_dev::run_retrieval_explain(
            &state,
            &query,
            project.as_deref(),
            k,
            conversation_id,
        )
    })
    .await
}

/// Natural-language retrieval diagnostic (card 7H): the user describes a symptom, and the background
/// model — reading their own current explain state — explains what it usually means and what to
/// change and why. RECOMMEND-only: it writes nothing; the user commits any change themselves via the
/// depth slider. Runs on the background key; resolves models under a short lock, then drops it before
/// the network call (rule #4).
#[tauri::command]
pub async fn retrieval_diagnose(
    app: AppHandle,
    symptom: String,
    query: String,
    explain: crate::commands_dev::DevRetrievalExplain,
) -> Result<String> {
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };
    retrieval_diag::diagnose(&app, &plan, &symptom, &query, &explain).await
}

// --- archivist: documents ---

/// Where the document engine (Python sidecar) is in its lifecycle, so the UI can
/// show first-run setup.
#[tauri::command]
pub fn sidecar_status(state: State<'_, AppState>) -> SidecarStatus {
    state.sidecar.status()
}

/// Progress for an optional-component download, broadcast on `<component>://install` — i.e.
/// `python://install` (the macOS interpreter fetch), `tsne://install`, and `ocr://install`. None of
/// these downloads has a file count, so `fraction` (0.0..=1.0, monotonic) renders as a percentage bar.
/// One shape + one emit helper for all three (X-D6); the per-component structs it replaced were
/// byte-identical. The python leg only ever fires on macOS when no system Python was found.
#[derive(Clone, Serialize)]
pub struct InstallProgressEvent {
    fraction: f32,
}

/// Emit optional-component install progress on the `<component>://install` channel. Fire-and-forget
/// (a dropped event costs a progress tick, never the install). Shared by `ensure_sidecar` (python),
/// `install_optional_tsne`, and `install_optional_ocr` so the channel name is built exactly one way.
pub fn emit_install_progress(app: &AppHandle, component: &str, fraction: f32) {
    let _ = app.emit(
        &format!("{component}://install"),
        InstallProgressEvent { fraction },
    );
}

/// Provision the managed venv if needed (slow on first run). Run off the async
/// runtime so the UI stays responsive. On macOS, if no interpreter is found and PM
/// downloads one, its byte progress streams over `python://install`.
#[tauri::command]
pub async fn ensure_sidecar(app: AppHandle) -> Result<()> {
    let progress_app = app.clone();
    spawn_blocking_result("setup", move || {
        app.state::<AppState>()
            .sidecar
            .ensure_installed_with_progress(move |fraction| {
                emit_install_progress(&progress_app, "python", fraction);
            })
    })
    .await
}

/// Refuse a user-started indexing operation that would race a running rebuild (#371).
///
/// A rebuild re-reads the whole vault, upserts each document, then sweeps away the ones it never saw; on
/// the vector-width arm it clears the store outright first. Either way, work started underneath it is the
/// thing at risk — so the automatic writers (the folder watcher, the idle chat-indexer) quietly defer,
/// while these user-pressed paths say so out loud. Nothing was going to happen either way; the difference
/// is whether the user finds out. `what` completes "…rebuilding the search index right now, so {what}".
pub(super) fn refuse_if_rebuilding(app: &AppHandle, what: &str) -> Result<()> {
    if app.state::<AppState>().rebuild_running() {
        return Err(Error::Other(format!(
            "PM is rebuilding the search index right now, so {what}. Open the Documents tab to watch it, \
             then try again once it's finished."
        )));
    }
    Ok(())
}

/// Ingest files/folders: convert → chunk → embed → index. Progress streams over
/// `on_event`. The whole pipeline is blocking, so it runs on a blocking thread.
///
/// `paths` are raw filesystem paths, so this is effectively an arbitrary-file-read
/// primitive — deliberately trusted: the only caller is PM's own webview, and the
/// paths come from the user's drag-drop / file-dialog (the same reach the dialog
/// already grants). It is not exposed to any external/untrusted caller.
#[tauri::command]
pub async fn ingest_paths(
    app: AppHandle,
    paths: Vec<String>,
    copy_photos_to_vault: Option<bool>,
    on_event: Channel<IngestEvent>,
) -> Result<()> {
    refuse_if_rebuilding(&app, "it can't take new documents")?;
    // L-5: `paths` arrives straight from the webview — the file picker AND the OS drag-drop both
    // funnel here — so validate every entry server-side before we read a byte. A path that is
    // relative, malformed, or doesn't exist is rejected fail-closed (a compromised webview can't
    // point ingest at a fabricated location). The originals are then walked unchanged so stored
    // source paths keep their on-disk form.
    for p in &paths {
        pathguard::sanitize_source(p)?;
    }
    let opts = ingest::IngestOpts {
        copy_photos_to_vault: copy_photos_to_vault.unwrap_or(false),
    };
    spawn_blocking_result("ingest", move || ingest::run(&app, paths, opts, on_event)).await
}

/// Drop the index and rebuild it from the Markdown vault (spec §3 acceptance), then upgrade every
/// reachable index-only item (Drive / OneDrive / local folder) from the ~500-char summary the rebuild
/// restored to a FULL-body index — so connected files end up chunked from their whole contents, not a
/// preview. The upgrade is best-effort and one item at a time: an unreachable source is left on its
/// summary and healed by the next connector Sync (its `summary_indexed` flag forces a re-embed).
///
/// Progress is broadcast on the global `ingest://progress` event rather than a per-call `Channel`,
/// so it reaches whatever view is mounted — including one that mounts long after the rebuild began.
/// Read `rebuild_status` on mount for what was missed.
#[tauri::command]
pub async fn rebuild_index(app: AppHandle) -> Result<()> {
    let sink = ingest::ProgressSink::new(app.clone());
    // A user-started Rebuild always mints a FRESH pass id, so nothing is skipped: "my index looks wrong,
    // rebuild it" must redo every document, not notice they all carry a stamp and do nothing. Only a
    // RESUME reuses a stored id (see `resume_rebuild`) — that is the whole distinction.
    rebuild_core(app, sink, ingest::new_pass_id()).await
}

/// What `REBUILD_PENDING_KEY` holds while a rebuild is in flight: the run's pass id, plus the retrieval
/// config that run is building under (#371).
///
/// Both halves are needed to decide whether a stored pass may be RESUMED. The pass id says which run's
/// stamps to trust; the config says whether this build would still produce the same chunks as that run
/// did. A marker whose config no longer matches must not be resumed — its committed documents carry
/// chunks today's build would not produce, and skipping them would silently bank them forever.
#[derive(Serialize, Deserialize)]
struct RebuildMarker {
    pass: String,
    config: RetrievalConfig,
}

impl RebuildMarker {
    fn encode(pass: &str, config: &RetrievalConfig) -> Result<String> {
        serde_json::to_string(&RebuildMarker {
            pass: pass.to_string(),
            config: config.clone(),
        })
        .map_err(|e| Error::Other(format!("encode rebuild marker: {e}")))
    }

    /// The pass id this marker's run may be resumed under, given what THIS build would produce — or
    /// `None` when the interrupted pass can't be continued and the caller must mint a fresh one.
    ///
    /// `None` covers both the pre-v3.19 marker (a bare `"1"`, which parses as neither a pass nor a
    /// config) and a marker written by a build whose retrieval config differs from this one. Either way
    /// the honest answer is the same: don't trust those stamps, rebuild everything.
    fn resumable_pass(marker: &str, current: &RetrievalConfig) -> Option<String> {
        let parsed: RebuildMarker = serde_json::from_str(marker).ok()?;
        (&parsed.config == current).then_some(parsed.pass)
    }
}

/// The rebuild itself, over whatever progress sink the caller supplies — a user-started rebuild
/// (channel + global) or one resumed on launch (global only). Owns the single-flight guard, the
/// shared snapshot's lifecycle, and the crash-resume marker, so every entry point gets them.
async fn rebuild_core(app: AppHandle, sink: ingest::ProgressSink, pass: String) -> Result<()> {
    // Single-flight. Two rebuilds at once would fight over the same rows and, on the width-change arm,
    // one's `DELETE FROM documents` would still eat the other's in-progress work — reachable before this
    // guard by switching tabs (which resets the UI's own component-local guard) and clicking Rebuild
    // again. It is also the flag every other indexing writer now defers to (see `rebuild_running`).
    // Refuse loudly rather than silently no-op: the user pressed a button and deserves an answer.
    // `state` is bound first so it outlives the guard borrowed out of it (locals drop in reverse).
    let state = app.state::<AppState>();
    let Some(_busy) = BusyGuard::acquire(&state.ingest_busy) else {
        return Err(Error::Other(
            "A rebuild is already running. It keeps going in the background — open the Documents \
             tab to watch it."
                .into(),
        ));
    };

    // PRECONDITION, not housekeeping: repair any chat vault file the pre-3.81.2 organisation-write
    // bug stripped of its identity, BEFORE this pass reads a single file.
    //
    // A stripped chat is recoverable right up until a Rebuild, and destroyed by one: with
    // `source_type: chat` gone the walk stops matching `is_chat_vault_file`, re-ingests the
    // conversation as an ordinary document, NULLs every turn pointer and indexes PM's own answers as
    // source material. Healing on vault open alone would leave a real window — update, click Rebuild,
    // lose the chats — so the dangerous path heals first rather than racing the open-time pass. It is
    // idempotent and writes nothing on a healthy store, so this costs one front-matter read per chat.
    //
    // Inside the single-flight guard and before the resume marker, so it cannot interleave with
    // another rebuild or be skipped by a resumed one.
    state.reconcile_chat_identity();

    // Count reachable index-only items up front so the progress bar's total spans BOTH phases (the
    // vault rebuild AND the full-body re-index). The count is stable because a local rebuild never
    // changes a source's reachability.
    let extra_total = {
        let conn = state.conn()?;
        conn.query_row(
            "SELECT count(*) FROM documents WHERE source_type = 'index_only' AND source_state = 'ok'",
            [],
            |r| r.get::<_, i64>(0),
        )? as usize
    };

    if let Ok(mut snap) = state.ingest_job.lock() {
        *snap = crate::IngestJobState {
            running: true,
            started_at_ms: Some(crate::epoch_ms()),
            ..Default::default()
        };
    }

    // The resume marker carries this run's PASS ID **and the retrieval config it is building under**, so
    // a relaunch doesn't merely know "a rebuild was unfinished" — it knows WHICH one, and whether this
    // build would still produce the same chunks (#371).
    //
    // The config half is load-bearing, not bookkeeping. The marker is durable, so a rebuild interrupted
    // at 50% can be resumed by a DIFFERENT BUILD — close PM mid-rebuild, the updater installs a version
    // with a new `SPLITTER_VERSION`, and the resume fires on next launch. Skipping on pass id alone would
    // then bank the half of the vault the old build chunked, finish the rest with the new splitter, and
    // stamp the vault as fully current — a permanently mixed-config index with the "Rebuild recommended"
    // prompt cleared, so nothing would ever tell the user. See `resume_rebuild` for the other half.
    //
    // `ingest::rebuild` writes it, not this function: only it knows when the mutating phase actually
    // begins, and it must land after the model warmup proves the embedder works. A warmup failure
    // destroys nothing, so it must not leave a marker behind that makes every future launch retry a
    // rebuild that fails identically — which is what writing it here unconditionally did.
    let marker_app = app.clone();
    let marker_pass = pass.clone();
    let on_pass_start = move || -> Result<()> {
        let state = marker_app.state::<AppState>();
        let conn = state.conn()?;
        let config = RetrievalConfig::current_for(&db::selected_embedder(&conn)?);
        db::set_setting(
            &conn,
            REBUILD_PENDING_KEY,
            &RebuildMarker::encode(&marker_pass, &config)?,
        )
    };

    let result = rebuild_passes(&app, &sink, extra_total, &pass, on_pass_start).await;

    // Clear `running` on every path, success or failure, so a failed rebuild can't wedge the UI
    // showing a phantom in-flight job for the rest of the session. The marker only clears on
    // success: a failure leaves the pass unfinished, which is exactly what resume is for.
    {
        if let Ok(mut snap) = state.ingest_job.lock() {
            snap.running = false;
            snap.started_at_ms = None;
        }
        if result.is_ok() {
            if let Ok(conn) = state.conn() {
                let _ = db::set_setting(&conn, REBUILD_PENDING_KEY, "");
            }
        }
    }

    let (ingested, skipped, failed, unreadable) = result?;
    sink.send(IngestEvent::Finished {
        ingested,
        skipped,
        failed,
        // A real number from the vault walk, not a placeholder. The walk's partial-picture signal
        // already withholds the straggler sweep (`may_reap`), but that is invisible: without this the
        // rebuild reported a clean run over a vault it had only half enumerated.
        unreadable,
    });
    Ok(())
}

/// Both rebuild phases: rebuild from the vault, then upgrade index-only items to a full body. Split out
/// so `rebuild_core` can bracket it with the guard/snapshot/marker teardown on every exit path, including
/// the error ones.
async fn rebuild_passes<F>(
    app: &AppHandle,
    sink: &ingest::ProgressSink,
    extra_total: usize,
    pass: &str,
    on_pass_start: F,
) -> Result<(usize, usize, usize, usize)>
where
    F: Fn() -> Result<()> + Send + 'static,
{
    // `spawn_blocking` needs 'static, so the blocking phase gets its own clone of the sink — as the
    // pre-sink code did with the bare Channel. Both clones address the same snapshot and emit the
    // same global event, so progress is continuous across the phase boundary.
    let app2 = app.clone();
    let sink2 = sink.clone();
    let pass2 = pass.to_string();
    // `unreadable` comes only from phase 1: phase 2 works from the encrypted manifest, not a walk of
    // the filesystem, so it has no entries it could fail to enumerate.
    let (ingested, skipped, failed, unreadable) = spawn_blocking_result("rebuild", move || {
        ingest::rebuild(&app2, &sink2, extra_total, &pass2, &on_pass_start)
    })
    .await?;
    let (upgraded, up_skipped, up_failed) =
        upgrade_index_only_to_full_body(app, sink, pass).await?;
    let failed_total = failed + up_failed;

    // Stamp the vault ONLY once BOTH phases have finished with nothing failed — that, and only that, means
    // the stored index really does reflect the current retrieval config end to end. The stamp clears the
    // "Rebuild recommended" prompt, so it is the user's ONLY signal that a rebuild is owed: writing it
    // after a pass that left documents on their old chunks (a vault file that wouldn't read, a connector
    // item phase 2 couldn't re-fetch) would retire that signal while the reason for it still stands, and
    // nothing would ever raise it again. Withholding it keeps the prompt up, and the next Rebuild heals
    // them. It lives here, not in `ingest::rebuild`, because only this layer has seen both phases.
    //
    // Skips don't block it: a skipped document was built by this same pass under this same config, which
    // `resume_rebuild` verifies against the marker before it agrees to reuse a pass id at all.
    if failed_total == 0 {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let config = RetrievalConfig::current_for(&db::selected_embedder(&conn)?);
        db::set_retrieval_stamp(&conn, &config)?;
    }
    Ok((
        ingested + upgraded,
        skipped + up_skipped,
        failed_total,
        unreadable,
    ))
}

/// Upgrade every reachable index-only item to a full-body index: re-fetch its live body and re-embed (via
/// [`reindex_index_only_core`], which preserves the item's classification), one at a time with per-item
/// progress. Their bodies are remote and never held locally, so this network pass is the ONLY thing that
/// can re-chunk them under a changed splitter/embedder — which is why it runs on every rebuild, not just
/// the ones that restored a summary. Best-effort: a per-item failure is reported and counted, never fatal.
/// Returns `(upgraded, skipped, failed)`.
///
/// **What a failure leaves behind, honestly.** An item PM can't re-fetch (offline source, expired auth) is
/// left exactly as it was — which since #371 means it keeps its existing full-body chunks rather than being
/// knocked down to its ~500-char summary first. That is strictly better to search, but it does mean the
/// next connector Sync will NOT heal it the way it used to: `summary_indexed` only fires for a row that
/// really is summary-derived, and this row isn't. So if the failure happened during a splitter/embedder
/// change, that item keeps chunks cut by the old config until another Rebuild reaches it. The signal that
/// one is owed is the retrieval stamp, which `ingest::rebuild` withholds whenever a pass had failures.
///
/// Resumable since #371, on the same pass stamp as the vault loop: an item this pass already upgraded is
/// skipped, so a rebuild interrupted at 95% doesn't re-download every connected file on the next launch —
/// the single most expensive thing an interrupted rebuild used to repeat.
async fn upgrade_index_only_to_full_body(
    app: &AppHandle,
    on_event: &ingest::ProgressSink,
    pass: &str,
) -> Result<(usize, usize, usize)> {
    let items: Vec<(i64, String, Option<String>)> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, rebuild_pass FROM documents \
             WHERE source_type = 'index_only' AND source_state = 'ok' ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let (mut upgraded, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    for (doc_id, title, item_pass) in items {
        // `Started` first even when we're about to skip — the views amend the row `Started` opened, so a
        // bare `Skipped` renders as a nameless entry.
        on_event.send(IngestEvent::Started {
            path: format!("idx://{doc_id}"),
            name: title,
        });
        if ingest::plan_rebuild_one(item_pass.as_deref(), pass) == ingest::RebuildPlan::AlreadyDone
        {
            skipped += 1;
            on_event.send(IngestEvent::Skipped {
                path: format!("idx://{doc_id}"),
                reason: "already rebuilt by the run that was interrupted".into(),
            });
            continue;
        }
        let outcome = match reindex_index_only_core(app, doc_id).await {
            Ok(_) => {
                let state = app.state::<AppState>();
                // Claim it for this pass in the same breath as loading it back. A transient failure here
                // is this ITEM's failure, not the whole pass's — a bare `?` would abort the upgrade of
                // every remaining item over one momentary DB lock.
                state.conn().and_then(|conn| {
                    ingest::stamp_rebuild_pass(&conn, doc_id, pass)?;
                    ingest::load_document(&conn, doc_id)
                })
            }
            Err(e) => Err(e),
        };
        match outcome {
            Ok(document) => {
                upgraded += 1;
                on_event.send(IngestEvent::Done {
                    document,
                    warning: None,
                });
            }
            Err(e) => {
                // Leave it as it is (the next Sync heals it) and report — never fatal.
                failed += 1;
                on_event.send(IngestEvent::Failed {
                    path: format!("idx://{doc_id}"),
                    error: e.to_string(),
                });
            }
        }
    }
    Ok((upgraded, skipped, failed))
}

/// Dev-only: drive the index-only substrate (board card 3) through its reducer, without a real
/// connector. `kind` is `add` (ingest a pasted body as a new index-only item), `update` (re-embed
/// from a new body), `delete` (→ soft source-missing), `rename` (update the external ref), or
/// `source_failure` (→ unreachable for every item of the source). The real "add a source" + change
/// detection ship with the connector cards; this routes a hand-made event through `react` +
/// `apply_actions`, so the whole observe-and-react path — Add included — is exercised. Debug only.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_apply_change_event(
    app: AppHandle,
    kind: String,
    source_id: String,
    title: Option<String>,
    body: Option<String>,
    external_ref: Option<String>,
) -> Result<()> {
    spawn_blocking_result("dev change", move || -> Result<()> {
        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
        let gateway = {
            let conn = state.conn()?;
            state.gateway_for_write(&conn)?
        };
        let (vault_root, manifest_cipher) = state.manifest_io()?;

        // The item's current persisted state (for the reducer). `None` if the source id is unknown.
        let current: Option<(String, Option<String>, Option<String>, String)> = {
            let conn = state.conn()?;
            match conn.query_row(
                "SELECT title, source_modified_at, source_content_hash, source_state \
                 FROM documents WHERE source_id = ?1 AND source_type = 'index_only'",
                params![source_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ) {
                Ok(row) => Some(row),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(e.into()),
            }
        };
        let now = {
            let conn = state.conn()?;
            ingest::iso_now(&conn)?
        };
        // The title for a fetched body: an explicit one (add), else the stored one (update).
        let item_title = title
            .clone()
            .or_else(|| current.as_ref().map(|c| c.0.clone()))
            .unwrap_or_else(|| source_id.clone());

        let (event, fetched) = match kind.as_str() {
            "add" => {
                let body = body.unwrap_or_default();
                let new_hash = ingest::hex_digest(body.as_bytes());
                (
                    index_only::ChangeEvent::Add {
                        source_id: source_id.clone(),
                        modified_at: Some(now.clone()),
                    },
                    Some(index_only::PointerInput {
                        source_id: source_id.clone(),
                        title: item_title,
                        external_ref,
                        source_modified_at: Some(now.clone()),
                        source_content_hash: Some(new_hash),
                        body,
                        // Dev affordance (pasted body) — no source folder to tag with.
                        source_parent_folder_id: None,
                        source_parent_folder_name: None,
                    }),
                )
            }
            "update" => {
                let body = body.unwrap_or_default();
                // Stand in for the source's reported content hash with a digest of the new body
                // (deterministic, so re-firing the same body is a no-op — the debounce/hash guard).
                let new_hash = ingest::hex_digest(body.as_bytes());
                (
                    index_only::ChangeEvent::Update {
                        source_id: source_id.clone(),
                        modified_at: Some(now.clone()),
                        new_content_hash: Some(new_hash.clone()),
                    },
                    Some(index_only::PointerInput {
                        source_id: source_id.clone(),
                        title: item_title,
                        external_ref: None,
                        source_modified_at: Some(now.clone()),
                        source_content_hash: Some(new_hash),
                        body,
                        // Dev affordance (pasted body) — no source folder to tag with.
                        source_parent_folder_id: None,
                        source_parent_folder_name: None,
                    }),
                )
            }
            "delete" => (
                index_only::ChangeEvent::Delete {
                    source_id: source_id.clone(),
                },
                None,
            ),
            "rename" => (
                index_only::ChangeEvent::Rename {
                    source_id: source_id.clone(),
                    new_external_ref: external_ref,
                },
                None,
            ),
            "source_failure" => (
                index_only::ChangeEvent::SourceFailure {
                    source: source_id.clone(),
                },
                None,
            ),
            other => return Err(Error::Other(format!("unknown dev event kind: {other}"))),
        };

        let item_state = current.map(|(_, smod, shash, sstate)| index_only::ItemState {
            source_id: source_id.clone(),
            source_modified_at: smod,
            source_content_hash: shash,
            source_state: index_only::SourceState::from_db(&sstate),
            // The dev harness always pastes a full body (never a summary restore), so this item is
            // never summary-derived.
            summary_indexed: false,
        });
        let actions = index_only::react(event, item_state.as_ref());
        // A single dev event: apply, then flush its manifest change immediately (no batch loop here).
        if index_only::apply_actions(&state, &gateway, &actions, fetched.as_ref())?.dirtied {
            let conn = state.conn()?;
            index_only::write_synced(&conn, &vault_root, &manifest_cipher)?;
        }
        Ok(())
    })
    .await
}

/// What a pinboard note became after ingest — enough for the board to show "in review" / "filed
/// to X" without a second query. `source_id` is `note:<widget_id>`; the document is a full vault
/// Markdown file that lives on its own (nothing reconciles a `note:` source), so it survives the
/// note being deleted.
#[derive(Serialize)]
pub struct NoteIngest {
    pub source_id: String,
    pub document_id: i64,
    pub reviewed: bool,
    pub project: String,
}

/// The title for a note-derived document: its first non-blank line, trimmed and capped by
/// characters (never splitting a codepoint), else a friendly fallback. Pure — see tests.
fn derive_title(body: &str) -> String {
    const MAX: usize = 80;
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.is_empty() {
        return "Untitled note".into();
    }
    let mut out: String = line.chars().take(MAX).collect();
    if line.chars().count() > MAX {
        out.push('…');
    }
    out
}

/// Ingest a pinboard note's text as a REAL vault Markdown document (the note is already Markdown),
/// so it flows through the review → proposal → project-importance pipeline and then shows in
/// Documents / Focus / the briefing like any document. Keyed on the note's widget id
/// (`note:<widget_id>`), so it's idempotent: an unchanged re-ingest is a no-op, and an edited note
/// re-embeds in place, KEEPING whatever project / tags / importance it was filed under. The document
/// is standalone — no reconcile watches a `note:` source, and its full body lives in the vault — so
/// deleting the note never removes it, and it's fully readable/searchable offline (not a 500-char
/// summary). See [`ingest::ingest_note_document`], which also promotes any note ingested under the
/// earlier index-only path (v2.89.0-alpha #214) in place.
#[tauri::command]
pub async fn ingest_note(
    app: AppHandle,
    widget_id: String,
    title: String,
    text: String,
) -> Result<NoteIngest> {
    spawn_blocking_result("ingest note", move || -> Result<NoteIngest> {
        let body = text.trim();
        if body.is_empty() {
            return Err(Error::Other(
                "this note is empty — nothing to ingest".into(),
            ));
        }
        // Prefer the note's own (editable) title; fall back to the first non-blank line of the body
        // for untitled notes, preserving the previous behaviour.
        let title = {
            let t = title.trim();
            if t.is_empty() {
                derive_title(body)
            } else {
                t.to_string()
            }
        };

        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
        let gateway = {
            let conn = state.conn()?;
            state.gateway_for_write(&conn)?
        };
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, manifest_cipher) = state.manifest_io()?;

        let document = ingest::ingest_note_document(
            &state,
            &gateway,
            &vault,
            &cipher,
            &vault_root,
            &manifest_cipher,
            &widget_id,
            &title,
            body,
        )?;

        Ok(NoteIngest {
            source_id: format!("note:{widget_id}"),
            document_id: document.id,
            reviewed: document.reviewed,
            project: document.project,
        })
    })
    .await
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> Result<Vec<Document>> {
    let conn = state.conn()?;
    ingest::list_documents(&conn)
}

/// Fetch a single document by id — the reader's "open by citation id" path uses this instead of
/// refetching the entire document list to resolve one id (F-48), which scales with connector estates.
#[tauri::command]
pub fn get_document(state: State<'_, AppState>, id: i64) -> Result<Document> {
    let conn = state.conn()?;
    ingest::load_document(&conn, id)
}

/// Transcribe a recorded voice clip to text for the chat box (spec §4 P1 — voice
/// input). The webview records the clip and sends it base64-encoded; we decode it
/// to a temp file inside the data dir, transcribe it locally via the sidecar's
/// Whisper model, and delete the file. An explicit user action, so it ensures the
/// engine is installed first. Fully on-device — the audio never leaves the
/// machine. All blocking, so it runs off the async runtime.
#[tauri::command]
pub async fn transcribe_audio(app: AppHandle, audio_base64: String) -> Result<String> {
    use base64::Engine;

    spawn_blocking_result("transcription", move || -> Result<String> {
        // Bound the untrusted webview payload before allocating the decode buffer
        // (every other webview input is capped). ~32 MiB of base64 ≈ 24 MiB of
        // audio — far more than a dictation clip, but it stops a hostile/oversized
        // string from ballooning memory on a low-RAM machine.
        const MAX_AUDIO_B64_CHARS: usize = 32 * 1024 * 1024;
        let b64 = audio_base64.trim();
        if b64.len() > MAX_AUDIO_B64_CHARS {
            return Err(Error::Other(
                "the recording is too large to transcribe".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::Other(format!("could not decode the recording: {e}")))?;
        if bytes.is_empty() {
            return Ok(String::new());
        }

        // Keep the clip inside PM's data dir (not the system temp) so it shares the
        // user's at-rest disk encryption. A random-named NamedTempFile deletes
        // itself on drop (RAII), so even a crash mid-transcribe can't leave the raw
        // audio behind under a predictable name.
        use std::io::Write;
        let tmp_dir = paths::data_dir(&app)?.join("runtime").join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let mut clip = tempfile::Builder::new()
            .prefix("voice-")
            .suffix(".webm")
            .tempfile_in(&tmp_dir)?;
        clip.write_all(&bytes)?;
        clip.flush()?;

        let state = app.state::<AppState>();
        state.sidecar.ensure_installed()?;
        let text = state.sidecar.transcribe(clip.path());

        // `clip` drops at end of scope, deleting the temp file on success or error.
        text
    })
    .await
}

// --- retrieval-relevance feedback (Stage-4 card 10) ---
//
// Capture only. Nothing reads these signals yet; they accrue so a learned reranker has a corpus to
// train on when that work lands. See `retrieval_feedback` for why `corrections` can't serve.

/// Rate a grounded answer (`"up"` / `"down"`), or clear the rating with `None`.
///
/// Silently no-ops on an answer that retrieved nothing — there is no relevance judgement to record
/// against an empty grounding, and failing the click would be a worse answer to a harmless action.
#[tauri::command]
pub fn rate_answer(
    state: State<'_, AppState>,
    message_id: i64,
    rating: Option<String>,
) -> Result<()> {
    let parsed = rating
        .as_deref()
        .map(retrieval_feedback::Rating::parse)
        .transpose()?;
    let conn = state.conn()?;
    retrieval_feedback::set_rating(&conn, message_id, parsed)?;
    Ok(())
}

/// Log that the user opened one of the sources an answer cited — an implicit relevance signal.
#[tauri::command]
pub fn record_citation_click(
    state: State<'_, AppState>,
    message_id: i64,
    document_id: i64,
) -> Result<()> {
    let conn = state.conn()?;
    retrieval_feedback::record_citation_click(&conn, message_id, document_id)?;
    Ok(())
}

/// The feedback already recorded for an answer, so its controls render in the right state.
#[tauri::command]
pub fn answer_feedback(
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<retrieval_feedback::AnswerFeedback> {
    let conn = state.conn()?;
    retrieval_feedback::feedback_for(&conn, message_id)
}

/// The currently-running rebuild snapshot (empty / `running:false` when idle), so the Documents tab
/// and the Settings rebuild modal can resume showing progress after the user leaves and returns —
/// the ingest sibling of [`drive_sync_status`]. Also carries the last finished run's counts, so a
/// user who returns after it completed still sees the result.
#[tauri::command]
pub fn rebuild_status(state: State<'_, AppState>) -> Result<crate::IngestJobState> {
    state
        .ingest_job
        .lock()
        .map(|s| s.clone())
        .map_err(|_| Error::Other("rebuild state poisoned".into()))
}

/// Acknowledge the last finished rebuild's counts, so the "Done — N ingested" line stops coming
/// back.
///
/// That line is a REPLAY: `rebuild_status` serves `last_report` on every mount, and the only thing
/// that ever cleared it was the START of the next rebuild (:344-350). So it outlived every tab
/// switch and only a relaunch — which builds a fresh `IngestJobState` — made it go. A user who had
/// read the result was told it again every time they came back to Documents.
///
/// Deliberately leaves `recent` alone. Clearing the rows here too would be the tidier-looking
/// "clear the finished card as a unit", and it would defeat the neighbouring fix: the common case
/// is watching a rebuild end while the view is mounted, so the rows would be dropped the instant
/// the run finished and "show me every file this pass built" would yield nothing. The banner is
/// what was unwanted; the list is what was asked for. `clear_rebuild_activity` drops both, on an
/// explicit act.
///
/// No-ops while a rebuild is running: `RebuildProgress` is a second live listener on the same
/// snapshot, so a stray acknowledge must never touch an in-flight run.
#[tauri::command]
pub fn ack_rebuild_report(state: State<'_, AppState>) -> Result<()> {
    if let Ok(mut snap) = state.ingest_job.lock() {
        if !snap.running {
            snap.last_report = None;
        }
    }
    Ok(())
}

/// Drop the whole finished-rebuild card — the counts AND the per-file rows.
///
/// Two callers, both of which mean "the previous rebuild's Activity is no longer what this tab is
/// about": the explicit dismiss on the Done line, and the start of an IMPORT. The import case
/// matters more than it looks: drag-and-drop and Add files report through a per-call `Channel`, not
/// through `ProgressSink`, so they never write `recent` at all. Without this the next mount would
/// restore the last REBUILD's rows and present them as that import's Activity.
///
/// No-ops while a rebuild is running, for the same reason as `ack_rebuild_report`.
#[tauri::command]
pub fn clear_rebuild_activity(state: State<'_, AppState>) -> Result<()> {
    if let Ok(mut snap) = state.ingest_job.lock() {
        if !snap.running {
            snap.last_report = None;
            snap.recent.clear();
            snap.recent_truncated = false;
        }
    }
    Ok(())
}

/// Resume a rebuild a previous app session started but didn't finish (the app was closed/crashed
/// mid-rebuild). Called once on launch. Returns whether a resume was kicked off.
///
/// Genuinely **continues** the interrupted pass since #371: the marker holds that pass's id, and every
/// document it managed to commit carries the same id (`documents.rebuild_pass`), so the resumed run
/// recognises them, skips them, and does only the work that was left — the guarantee the connectors' sync
/// already gave. A rebuild closed at 95% no longer re-embeds the whole vault, and no longer re-downloads
/// every connected file. No marker → nothing to resume.
///
/// **A pass is only continued if this build would still produce the same chunks.** The marker records the
/// retrieval config its run was building under, and a mismatch mints a fresh pass id instead — so the
/// resume degrades to a full rebuild rather than banking chunks the running build no longer agrees with.
/// This is the case where PM auto-updated between the interruption and the resume: without the check, a
/// new `SPLITTER_VERSION` would leave half the vault on the old boundaries and then stamp it all current.
/// A pre-v3.19 marker (a bare `"1"`) fails to parse and takes the same path — a full restart, exactly as
/// that version behaved.
#[tauri::command]
pub fn resume_rebuild(app: AppHandle) -> Result<bool> {
    let marker: Option<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, REBUILD_PENDING_KEY)?
    };
    // Cleared markers are stored as "" rather than deleted, so treat empty as nothing-to-do.
    let Some(marker) = marker.filter(|m| !m.is_empty()) else {
        return Ok(false);
    };
    // Resume the interrupted pass, or mint a fresh one when its work can no longer be trusted. Note the
    // vault's STORED stamp can't answer this: during an interrupted pass it still holds the PRE-rebuild
    // config (the stamp is only written when a run finishes), so the marker has to carry it.
    let pass = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let current = RetrievalConfig::current_for(&db::selected_embedder(&conn)?);
        RebuildMarker::resumable_pass(&marker, &current).unwrap_or_else(ingest::new_pass_id)
    };
    // Don't stack on a rebuild already running this session.
    if app
        .state::<AppState>()
        .ingest_busy
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Ok(false);
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let sink = ingest::ProgressSink::new(app2.clone());
        let _ = rebuild_core(app2, sink, pass).await;
    });
    Ok(true)
}

#[cfg(test)]
mod rebuild_marker_tests {
    use super::RebuildMarker;
    use crate::registry;
    use crate::retrieval_config::RetrievalConfig;

    fn current() -> RetrievalConfig {
        RetrievalConfig::current_for(&registry::active_embedder())
    }

    #[test]
    fn a_pass_resumes_only_under_the_config_it_was_built_with() {
        let cfg = current();
        let marker = RebuildMarker::encode("pass-a", &cfg).unwrap();

        // Same build, same config → continue the interrupted pass. This is the #371 win.
        assert_eq!(
            RebuildMarker::resumable_pass(&marker, &cfg),
            Some("pass-a".to_string())
        );

        // THE case this exists for: PM auto-updated between the interruption and the resume, and the new
        // build chunks differently. The pass's committed documents carry boundaries this build would not
        // produce, so its stamps must NOT be trusted — resume must decline and rebuild everything.
        // Any field feeding `current_for` would do; the splitter version is the one that actually moves
        // between releases.
        let mut newer = cfg.clone();
        newer.splitter_version += 1;
        assert_eq!(
            RebuildMarker::resumable_pass(&marker, &newer),
            None,
            "a pass built by a different splitter must never be resumed"
        );
    }

    #[test]
    fn a_pre_v3_19_marker_declines_to_resume_rather_than_matching_nothing() {
        // Before #371 the marker was the literal "1". It carries no pass and no config, so the only
        // honest answer is "don't trust any stamp" → a full rebuild, exactly as that version behaved.
        assert_eq!(RebuildMarker::resumable_pass("1", &current()), None);
        // Garbage must not panic its way through launch either.
        assert_eq!(RebuildMarker::resumable_pass("{not json", &current()), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_title_takes_first_non_blank_line_capped() {
        // First non-blank line, trimmed.
        assert_eq!(derive_title("  Buy milk\nand eggs"), "Buy milk");
        assert_eq!(
            derive_title("\n\n   Second para is the title"),
            "Second para is the title"
        );
        // Empty / whitespace-only → a friendly fallback (register_pointer also rejects empty bodies).
        assert_eq!(derive_title(""), "Untitled note");
        assert_eq!(derive_title("   \n  \n"), "Untitled note");
        // Long first line is capped by characters with an ellipsis (never splitting a codepoint).
        let long = "x".repeat(100);
        let title = derive_title(&long);
        assert_eq!(title.chars().count(), 81); // 80 chars + the ellipsis
        assert!(title.ends_with('…'));
        // A multi-byte first line is capped by chars, not bytes — no panic, no split codepoint.
        let emoji = "🌍".repeat(100);
        assert_eq!(
            derive_title(&emoji).chars().filter(|c| *c == '🌍').count(),
            80
        );
    }
}
