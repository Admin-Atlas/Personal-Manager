// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sorting review queue, the tag vocabulary, retagging, and the filing writes that
//! commit a review decision to both the database and the vault.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::blocking::spawn_blocking_result;
use crate::error::{Error, Result};
use crate::ingest::{self, Document};
use crate::llm_gateway::{self, Role};
use crate::retag;
use crate::review::{self, ReviewDecision, ReviewEvent};
use crate::{db, entities, index_only, openrouter, preferences, vault, AppState};

use super::prefs::spawn_preferences_migration;
use super::spend::log_background_usage;
use super::vault_writes::finish_vault_transaction;

// --- archivist: sorting review & organisation (Step 4) ---

/// Every tag in the registry — projects and free-form labels alike — with its kind and how many
/// documents carry it (#276). Feeds the composer's `@` autocomplete, which is the only way a user
/// discovers that pinning a tag is possible at all.
#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<crate::tags::TagSummary>> {
    let conn = state.conn()?;
    crate::tags::list_all(&conn)
}

/// Distinct project labels across all documents — feeds the review project picker
/// and biases the AI proposal toward projects that already exist.
#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<String>> {
    let conn = state.conn()?;
    // The tag registry, not `SELECT DISTINCT project FROM documents` (#275). A superset of the old
    // answer in two ways that both matter to a picker: a project whose documents are all merely
    // LINKED to it has no `documents.project` row to be distinct over, and a project that exists as
    // triage only — a deadline or a milestone, no files yet — never appeared at all.
    crate::tags::project_names(&conn)
}

/// Documents still awaiting the sorting review (`reviewed = 0`).
#[tauri::command]
pub fn review_queue(state: State<'_, AppState>) -> Result<Vec<Document>> {
    let conn = state.conn()?;
    ingest::review_queue(&conn)
}

/// The COUNT of documents awaiting review — the sidebar badge reads this instead of fetching the whole
/// queue just to take its `.length` on every view change (F-47).
#[tauri::command]
pub fn review_queue_count(state: State<'_, AppState>) -> Result<i64> {
    let conn = state.conn()?;
    ingest::review_queue_count(&conn)
}

/// One cached AI proposal keyed by document — what `cached_proposals` returns so the Review tab can
/// repaint on load without a model call. `proposal` mirrors the streamed `ReviewEvent::Proposed`
/// payload, so the frontend seeds it through exactly the same path.
#[derive(serde::Serialize)]
pub struct CachedProposal {
    pub document_id: i64,
    pub proposal: review::Proposal,
}

/// The AI proposals persisted for documents still awaiting review (the v39 cache). The Review tab
/// reads this on load so re-opening the app never re-asks the model for proposals it already has —
/// only genuinely un-proposed documents reach `propose_metadata`.
#[tauri::command]
pub fn cached_proposals(state: State<'_, AppState>) -> Result<Vec<CachedProposal>> {
    let conn = state.conn()?;
    Ok(review::cached_proposals(&conn)?
        .into_iter()
        .map(|(document_id, proposal)| CachedProposal {
            document_id,
            proposal,
        })
        .collect())
}

/// A document's connector parent-folder, as a filing hint — trimmed, with blank treated as absent.
/// It is passed to `review::propose` as its own argument so it lands in the USER message beside the
/// document it describes. It used to be folded into the global profile preamble, which put it in the
/// SYSTEM message: untrusted content in instructions position, and a per-document string inside the
/// cached prefix that defeated prompt caching (#509).
fn folder_context(folder: Option<&str>) -> Option<&str> {
    folder.map(str::trim).filter(|f| !f.is_empty())
}

/// Propose project/tags/importance for the unreviewed documents, on demand (so a
/// big folder import doesn't auto-fire model calls). Proposals stream back over
/// `on_event`; they're transient — the user confirms them via `commit_review`.
/// Runs on the background API key; never holds the DB lock across a model call.
#[tauri::command]
pub async fn propose_metadata(
    app: AppHandle,
    document_ids: Option<Vec<i64>>,
    on_event: Channel<ReviewEvent>,
) -> Result<()> {
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };

    // Bound the (untrusted webview) id list: it expands to one SQL placeholder
    // each, so an unbounded list would blow SQLITE_MAX_VARIABLE_NUMBER. Far above
    // any real review selection.
    const MAX_PROPOSE_IDS: usize = 10_000;
    if document_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > MAX_PROPOSE_IDS)
    {
        return Err(Error::Other("too many documents selected at once".into()));
    }

    struct Pending {
        id: i64,
        title: String,
        body: String,
        /// The Drive folder this document was found in, if any — folded into the per-document profile
        /// preamble as one plain-text line (NULL for non-Drive and pre-v29 rows).
        folder: Option<String>,
    }

    // Gather the documents + existing projects + tags + learned profile (+ the seed inputs) under a
    // short lock, then drop it before any network call (rule #4).
    let (pending, projects, tags, profile, backlog_titles, stored_seed) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        // Global + context filing preferences only: the target project isn't chosen until the model
        // proposes it, so per-project preferences have nothing to key on yet (a deferred refinement).
        // Still a strict improvement on dumping the whole blob (§4.5).
        let profile = preferences::preferences_preamble(&conn, preferences::PrefContext::global())?;
        // Hand the model CANONICAL project names only (one per entity) — never the raw
        // `DISTINCT project`, which would offer variants like "PM"/"Atlas - PM" as co-equal.
        let projects = entities::canonical_project_names(&conn)?;
        // The same courtesy for tags: name the vocabulary that exists so the model reuses it rather
        // than coining a near-duplicate. Grouping is the entire point of a label, and a label that
        // groups one document does nothing.
        let tags = crate::tags::common_group_tags(&conn)?;
        // The seed decision below is about the STORE, not this call: since #607 the live arrival
        // path proposes five documents at a time, so a per-call count can never reach the threshold.
        // Both reads are skipped entirely once the store has a vocabulary of its own — which is the
        // only case where the seed can matter.
        let (backlog_titles, stored_seed) = if tags.is_empty() {
            (
                review::unreviewed_titles(&conn)?,
                db::get_setting(&conn, review::SEED_VOCAB_KEY)?,
            )
        } else {
            (Vec::new(), None)
        };
        let pending = {
            // Body sent to the filing model. For an index-only doc the chunks' `content` column is a
            // fixed placeholder (`INDEX_ONLY_BODY_PLACEHOLDER` — the body bytes are never stored), so
            // read its `stored_summary` instead; otherwise the model would classify off the title +
            // folder alone. Vault docs (`source_type` != 'index_only') have NULL `stored_summary`, so
            // they fall through to their first chunk's real content exactly as before.
            let base_sql = "SELECT d.id, d.title, \
                    COALESCE( \
                        CASE WHEN d.source_type = 'index_only' THEN NULLIF(d.stored_summary, '') END, \
                        (SELECT content FROM chunks c WHERE c.document_id = d.id ORDER BY ordinal LIMIT 1), \
                        '' \
                    ), \
                    d.source_parent_folder_name \
             FROM documents d WHERE d.reviewed = 0";

            let pending_sql = if let Some(ids) = document_ids.as_ref() {
                if ids.is_empty() {
                    format!("{base_sql} AND 1=0 ORDER BY d.ingested_at DESC, d.id DESC")
                } else {
                    let placeholders = std::iter::repeat_n("?", ids.len())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{base_sql} AND d.id IN ({placeholders}) ORDER BY d.ingested_at DESC, d.id DESC")
                }
            } else {
                format!("{base_sql} ORDER BY d.ingested_at DESC, d.id DESC")
            };

            let mut stmt = conn.prepare(&pending_sql)?;
            if let Some(ids) = document_ids.as_ref().filter(|ids| !ids.is_empty()) {
                stmt.query_map(rusqlite::params_from_iter(ids), |r| {
                    Ok(Pending {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                        folder: r.get(3)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            } else {
                stmt.query_map([], |r| {
                    Ok(Pending {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                        folder: r.get(3)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        (
            pending,
            projects,
            tags,
            profile,
            backlog_titles,
            stored_seed,
        )
    };

    let mut proposed = 0;
    let mut usage_rows: Vec<(Option<String>, openrouter::Usage, llm_gateway::CallMeta)> =
        Vec::new();

    // A store with NO tags yet has no vocabulary to reuse — and the list above is fixed for the
    // whole run, because it lives in the cached system prefix and must stay byte-identical (#509).
    // So a first import of any size would have every batch invent its own labels with only the five
    // documents in front of it in view: exactly the fragmentation #580 exists to repair, produced
    // on day one, and repairable only by a paid pass the user has to know to run.
    //
    // Seeding closes that. One cheap titles-only call — the same one the re-tag pass uses — chooses
    // a vocabulary with EVERY unreviewed document in view, and the whole import files against it.
    // Only when there is nothing established to reuse: an existing vocabulary is the user's, and
    // replacing it with a freshly-invented one would be the opposite of the point.
    //
    // Best-effort: a failed or unusable seed leaves the run exactly as it behaved before this
    // existed. Below the threshold it is not worth a call — a handful of documents cannot show a
    // theme, and the labels would be as one-off as the ones being avoided.
    //
    // Both the threshold and the titles are measured against the STORE's unreviewed backlog, not
    // this call's slice of it. #607 moved first-import proposals onto five-document arrival batches,
    // which left the old `pending.len()` gate comparing 5 against 20: it could never open, and the
    // fragmentation #581 exists to prevent came back untouched. Persisting what the first batch
    // settles on is the other half — without it the repaired gate would bill one vocabulary call per
    // five documents AND hand each batch a different list, which is worse than the bug.
    let tags = match review::seed_plan(
        tags.is_empty(),
        pending.len(),
        backlog_titles.len(),
        stored_seed.as_deref(),
    ) {
        review::SeedPlan::None => tags,
        review::SeedPlan::Reuse(seeded) => seeded,
        review::SeedPlan::Ask => {
            let max = retag::vocab_max(backlog_titles.len());
            // Sized to the local server's served window when there is one. This is the first
            // background call a fresh store makes, and at 400 uncapped titles it is also the one
            // most likely to overflow — see `retag::sample_titles_within`.
            let ceiling = llm_gateway::prompt_ceiling_for(&app, &plan);
            let sample = retag::sample_titles_within(&backlog_titles, max, ceiling);
            let messages = retag::vocabulary_messages(&sample, max);
            match llm_gateway::complete(&app, &plan, &messages, false).await {
                Ok(outcome) => {
                    let seeded = retag::parse_vocabulary(&outcome.completion.text, max);
                    usage_rows.push((
                        outcome.completion.model.clone(),
                        outcome.completion.usage,
                        outcome.meta,
                    ));
                    if !seeded.is_empty() {
                        // Its OWN scope, and taken after the model call has returned: the DB mutex
                        // is non-reentrant and must never be held across an await (rule #4).
                        let state = app.state::<AppState>();
                        let conn = state.conn()?;
                        if let Err(e) = db::set_setting(
                            &conn,
                            review::SEED_VOCAB_KEY,
                            &serde_json::to_string(&seeded).unwrap_or_default(),
                        ) {
                            // Logged rather than swallowed: this one write is the only thing between
                            // a 200-file import and forty billable vocabulary calls.
                            eprintln!("review: seed vocabulary not persisted ({e})");
                        }
                    }
                    seeded
                }
                Err(_) => Vec::new(),
            }
        }
    };

    // Documents are classified a batch at a time: one call proposes for several, which is where
    // most of the saving is (the instructions + canonical projects + profile are sent once per call,
    // not once per document). The global profile goes in as its own argument so it stays in the
    // cached system prefix; each document's folder rides in the user message beside it, as data
    // (#509). A folder BIASES its own document's proposal but never pre-assigns a project — the
    // review checkpoint is unchanged.
    //
    // The batch is sized DOWN when the answering server's window can't hold five documents plus the
    // run-wide system prefix; `review::BATCH_SIZE` is still the ceiling.
    let filing_ceiling = llm_gateway::prompt_ceiling_for(&app, &plan);
    let mut cursor = 0usize;
    while cursor < pending.len() {
        let all: Vec<review::DocInput<'_>> = pending[cursor..]
            .iter()
            .take(review::BATCH_SIZE)
            .map(|p| review::DocInput {
                title: &p.title,
                body: &p.body,
                folder: folder_context(p.folder.as_deref()),
            })
            .collect();
        let take = review::batch_within(&all, &projects, &tags, profile.as_deref(), filing_ceiling);
        let chunk = &pending[cursor..cursor + take];
        cursor += take;
        let docs: Vec<review::DocInput<'_>> = all.into_iter().take(take).collect();
        let mut outcome =
            review::propose_batch(&app, &plan, &docs, &projects, &tags, profile.as_deref()).await;
        let batch_error = outcome.error.clone();
        // The served model per document, for the proposal cache's `model` column (UI/debug only).
        // Starts as whichever model answered the batch; a retried document overwrites its own slot,
        // since an auto-switch fallback may have served it from a different model.
        let batch_model = outcome.usage.as_ref().and_then(|(_, m, _)| m.clone());
        let mut served: Vec<Option<String>> = vec![batch_model; chunk.len()];
        if let Some((usage, model, meta)) = outcome.usage.take() {
            usage_rows.push((model, usage, meta));
        }

        // Any document the batch didn't answer for is retried on its own before we give up on it.
        // This is what makes batching safe on a cheap model: it can lose track part-way through a
        // multi-document reply and still degrade to one-call-per-document, never to a wrong answer
        // silently attached to the wrong file.
        for (i, slot) in outcome.proposals.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            let mut retry = review::propose_batch(
                &app,
                &plan,
                &docs[i..=i],
                &projects,
                &tags,
                profile.as_deref(),
            )
            .await;
            served[i] = retry.usage.as_ref().and_then(|(_, m, _)| m.clone());
            let retry_error = retry.error.clone();
            if let Some((usage, model, meta)) = retry.usage.take() {
                usage_rows.push((model, usage, meta));
            }
            *slot = retry.proposals.into_iter().next().flatten().or_else(|| {
                // Batch and retry both came back empty. Surface the call error if there was one,
                // otherwise say plainly that the reply couldn't be read — the document stays in the
                // queue as Unsorted for manual filing either way.
                Some(review::Proposal::fallback(
                    retry_error
                        .or_else(|| batch_error.clone())
                        .unwrap_or_else(|| {
                            "Could not auto-classify (unreadable model reply).".to_string()
                        }),
                ))
            });
        }

        for ((p, proposal), model) in chunk.iter().zip(outcome.proposals).zip(&served) {
            let Some(mut proposal) = proposal else {
                continue;
            };
            // Resolve the model's project string to its canonical form for display (a known variant
            // is shown, and later committed, as the canonical name — the variant never surfaces),
            // and persist the finished proposal to the regenerable cache so re-opening the app
            // repaints it instead of re-billing the model. One short lock, dropped before the next
            // model call (rule #4).
            {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                proposal.project = entities::resolve_to_canonical(&conn, &proposal.project)?;
                review::cache_proposal(&conn, p.id, &proposal, model.as_deref())?;
            }
            let _ = on_event.send(ReviewEvent::Proposed {
                document_id: p.id,
                proposal,
            });
            proposed += 1;
        }
    }
    log_background_usage(&app, plan.models(), &usage_rows);
    let _ = on_event.send(ReviewEvent::Finished { proposed });
    Ok(())
}

/// A re-tag pass's streamed progress (#580).
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RetagEvent {
    /// The vocabulary the first call settled on, so the user sees what the rest of the pass is
    /// working from while it runs — and can stop it if it looks wrong.
    Vocabulary {
        tags: Vec<String>,
    },
    Progress {
        done: usize,
        total: usize,
    },
    Finished {
        changed: usize,
    },
    /// A phase has stopped, however it stopped. Emitted by [`retag::RetagRunGuard`]'s `Drop`, so no
    /// exit path can forget it — including the `?` sites, which is what the previous shape got
    /// wrong. `Finished` says a pass SUCCEEDED and carries its result; `Ended` says nobody is
    /// working any more, and is the only thing a re-entering view can rely on to release itself.
    ///
    /// Both phases emit it. The first phase used to emit nothing at all: it sent `Vocabulary` and
    /// returned, the guard cleared `running` silently, and a Teach tab that mounted while the model
    /// call was in flight adopted `running: true` and then never heard another word — shimmering
    /// with every control disabled for the life of that mount.
    Ended {
        phase: crate::RetagPhase,
        /// The failure, if it was one, so a view that was away while the pass died says why instead
        /// of reverting silently to idle.
        error: Option<String>,
    },
}

/// How much a re-tag pass would cover, so the UI can state the cost BEFORE anything is billed.
#[derive(Serialize)]
pub struct RetagScope {
    pub documents: i64,
    /// Model calls this would make: one for the vocabulary, then one per batch.
    pub calls: i64,
}

/// The re-tag pass's live snapshot (empty / `running:false` when idle), so Teach → Tags can resume
/// showing progress after the user leaves and returns — the retag sibling of [`rebuild_status`].
///
/// This is the piece that was missing. Both phases already emitted everything needed, but over a
/// per-call `Channel` that only the invoking component could hear; a tab switch dropped that
/// subscription and there was no way to rejoin a pass that was still running. It also carries the
/// finished vocabulary and the last pass's change count, so returning after either phase completed
/// still shows the result.
///
/// [`rebuild_status`]: super::rebuild_status
#[tauri::command]
pub fn retag_status(state: State<'_, AppState>) -> Result<crate::RetagJobState> {
    state
        .retag_job
        .lock()
        .map(|s| s.clone())
        .map_err(|_| Error::Other("re-tag state poisoned".into()))
}

#[tauri::command]
pub fn retag_scope(state: State<'_, AppState>) -> Result<RetagScope> {
    let conn = state.conn()?;
    let documents: i64 = conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0))?;
    let batch = retag::ASSIGN_BATCH as i64;
    let batches = (documents + batch - 1) / batch;
    Ok(RetagScope {
        documents,
        calls: if documents == 0 { 0 } else { batches + 1 },
    })
}

/// One document as the re-tag passes see it.
struct RetagDoc {
    id: i64,
    title: String,
    body: String,
}

/// Every document with the text the re-tag passes judge it by, under ONE short lock (rule #4).
///
/// The body mirrors the filing pass's COALESCE: an index-only document's chunk content is a fixed
/// placeholder, so its stored summary is the only real text there is.
fn retag_documents(app: &AppHandle) -> Result<Vec<RetagDoc>> {
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, \
                COALESCE( \
                    CASE WHEN d.source_type = 'index_only' THEN NULLIF(d.stored_summary, '') END, \
                    (SELECT content FROM chunks c WHERE c.document_id = d.id ORDER BY ordinal LIMIT 1), \
                    '' \
                ) \
         FROM documents d ORDER BY d.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RetagDoc {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Pass 1 alone: propose a tag vocabulary for the whole library and hand it back UNUSED (#579).
///
/// Split out from the labelling pass so the vocabulary is the user's to edit before anything is
/// labelled from it. That ordering is the point: the vocabulary is the one decision the whole pass
/// turns on, it is forty-odd words rather than a thousand documents, and reviewing it costs seconds
/// — whereas reviewing the CONSEQUENCES of a bad vocabulary means reading every proposal. Teach
/// exists to let someone correct how PM understands their things; this is that, for tags.
///
/// Nothing is written and nothing is staged. Runs on the background key.
#[tauri::command]
pub async fn propose_retag_vocabulary(app: AppHandle) -> Result<Vec<String>> {
    let state = app.state::<AppState>();
    // Ahead of `begin` on purpose: a start that was refused never opened a phase, so it must not
    // end one either — emitting `Ended` here would release a view watching the pass that IS running.
    let Some(_busy) = crate::BusyGuard::acquire(&state.retag_busy) else {
        return Err(Error::Other(
            "a re-tag pass is already running — it keeps going while you're on other tabs, and              Teach → Tags shows where it has got to."
                .into(),
        ));
    };
    let sink = retag::RetagSink::new(app.clone());
    // One model call over sampled titles: there is nothing countable here, so the phase is
    // honestly indeterminate rather than a bar sitting at zero.
    //
    // The body is split out so the guard can see how it ended. Every exit in it — six of them,
    // four being `?` sites — now emits `Ended` on the way past, which is what a Teach tab that
    // mounted mid-call has to hear before it can stop shimmering.
    let mut run = sink.begin(crate::RetagPhase::Vocabulary, None);
    let out = propose_vocabulary_inner(&app, &sink).await;
    run.record(&out);
    out
}

async fn propose_vocabulary_inner(app: &AppHandle, sink: &retag::RetagSink) -> Result<Vec<String>> {
    let Some(plan) = llm_gateway::resolve(app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };
    let docs = retag_documents(app)?;
    if docs.is_empty() {
        return Ok(Vec::new());
    }
    let titles: Vec<String> = docs.iter().map(|d| d.title.clone()).collect();
    let max = retag::vocab_max(docs.len());
    let ceiling = llm_gateway::prompt_ceiling_for(app, &plan);
    let messages =
        retag::vocabulary_messages(&retag::sample_titles_within(&titles, max, ceiling), max);
    // No cache_prefix: one call per pass, so there is no prefix to reuse.
    let outcome = llm_gateway::complete(app, &plan, &messages, false).await?;
    let vocabulary = retag::parse_vocabulary(&outcome.completion.text, max);
    log_background_usage(
        app,
        plan.models(),
        &[(
            outcome.completion.model.clone(),
            outcome.completion.usage,
            outcome.meta,
        )],
    );
    if vocabulary.is_empty() {
        return Err(Error::Other(
            "the model did not return a usable tag vocabulary — nothing has been changed".into(),
        ));
    }
    // Into the snapshot as well as the return value: this call is BILLED, and leaving the tab
    // while it ran used to throw the result away.
    sink.send(RetagEvent::Vocabulary {
        tags: vocabulary.clone(),
    });
    Ok(vocabulary)
}

/// Pass 2: label every document from the GIVEN vocabulary, staging the results (#580).
///
/// The vocabulary is a parameter rather than something this re-derives, so what labels the library
/// is exactly what the user approved — including any tags they added and minus any they struck out.
/// It is normalised and de-duplicated here rather than trusted verbatim: it has been through a text
/// input, and `parse_assignments` matches against it, so a stray `Tax ` would silently match
/// nothing.
///
/// Proposals are STAGED, never applied — `commit_retag` is the only thing that writes.
/// Runs on the background key and never holds the DB lock across a model call (rule #4).
#[tauri::command]
pub async fn apply_retag_vocabulary(app: AppHandle, vocabulary: Vec<String>) -> Result<()> {
    let state = app.state::<AppState>();
    // Held across BOTH phases. `retag_assign` opens by clearing every staged proposal, so a second
    // pass started over a first — which a tab switch made possible, since it reset the component's
    // own `working` flag — would wipe the first's half-staged work.
    //
    // Ahead of `begin`, for the same reason as the vocabulary phase: a refused start ends nothing.
    let Some(_busy) = crate::BusyGuard::acquire(&state.retag_busy) else {
        return Err(Error::Other(
            "a re-tag pass is already running — it keeps going while you're on other tabs, and              Teach → Tags shows where it has got to."
                .into(),
        ));
    };
    let sink = retag::RetagSink::new(app.clone());
    let mut run = sink.begin(crate::RetagPhase::Labelling, None);
    let out = apply_vocabulary_inner(&app, &sink, vocabulary).await;
    run.record(&out);
    out
}

/// The labelling pass proper. Nine exits, five of them `?` sites inside `retag_assign` — hence the
/// split: the guard ends the phase on every one of them.
async fn apply_vocabulary_inner(
    app: &AppHandle,
    sink: &retag::RetagSink,
    vocabulary: Vec<String>,
) -> Result<()> {
    let Some(plan) = llm_gateway::resolve(app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };

    let mut vocabulary: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for raw in &vocabulary {
            let t = retag::normalize_tag(raw);
            if !t.is_empty() && !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen
    };
    if vocabulary.is_empty() {
        return Err(Error::Other(
            "a re-tag pass needs at least one tag to label documents with".into(),
        ));
    }

    let docs = retag_documents(app)?;
    if docs.is_empty() {
        sink.send(RetagEvent::Finished { changed: 0 });
        return Ok(());
    }
    // The count is known here and not a moment earlier, but `begin` has to run before any of the
    // exits above it. So the total is published as soon as it exists: without this the bar
    // shimmers through the whole first model call — for the user who started it as much as for one
    // returning to the tab — even though PM already knows it is about to label 165 documents.
    sink.send(RetagEvent::Progress {
        done: 0,
        total: docs.len(),
    });
    // The cap still applies to a hand-edited list: it bounds the cached prefix, and an unbounded
    // vocabulary is the failure this whole feature exists to undo.
    vocabulary.truncate(retag::vocab_max(docs.len()));
    sink.send(RetagEvent::Vocabulary {
        tags: vocabulary.clone(),
    });

    let mut usage_rows: Vec<(Option<String>, openrouter::Usage, llm_gateway::CallMeta)> =
        Vec::new();
    retag_assign(app, &plan, &docs, &vocabulary, sink, &mut usage_rows).await?;
    log_background_usage(app, plan.models(), &usage_rows);
    Ok(())
}

/// Pass 2, shared: label every document from `vocabulary` and STAGE the result.
///
/// Starting a pass replaces any previous one — a half-reviewed set of proposals from an older
/// vocabulary would mix two vocabularies in one accept, which is the thing being fixed.
///
/// Never holds the DB lock across a model call (rule #4): the staging write for each batch takes
/// the lock and drops it before the next call goes out.
async fn retag_assign(
    app: &AppHandle,
    plan: &llm_gateway::RoutePlan,
    docs: &[RetagDoc],
    vocabulary: &[String],
    sink: &retag::RetagSink,
    usage_rows: &mut Vec<(Option<String>, openrouter::Usage, llm_gateway::CallMeta)>,
) -> Result<()> {
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        retag::clear(&conn, None)?;
    }

    let total = docs.len();
    let mut done = 0usize;
    let ceiling = llm_gateway::prompt_ceiling_for(app, plan);
    let mut cursor = 0usize;
    while cursor < docs.len() {
        let all: Vec<retag::RetagInput<'_>> = docs[cursor..]
            .iter()
            .take(retag::ASSIGN_BATCH)
            .map(|d| retag::RetagInput {
                title: &d.title,
                body: &d.body,
            })
            .collect();
        let take = retag::assign_batch_within(&all, vocabulary, ceiling);
        let chunk = &docs[cursor..cursor + take];
        cursor += take;
        let inputs: Vec<retag::RetagInput<'_>> = all.into_iter().take(take).collect();
        let messages = retag::assign_messages(&inputs, vocabulary);
        // cache_prefix: the system message holds only the vocabulary + instructions, identical for
        // every call in the run, so the provider serves it from cache (#509).
        let assignments = match llm_gateway::complete(app, plan, &messages, true).await {
            Ok(outcome) => {
                usage_rows.push((
                    outcome.completion.model.clone(),
                    outcome.completion.usage,
                    outcome.meta,
                ));
                retag::parse_assignments(&outcome.completion.text, chunk.len(), vocabulary)
            }
            // Best-effort, like the filing pass: a failed batch leaves those documents unproposed
            // rather than sinking the run. They keep the tags they have.
            Err(_) => vec![None; chunk.len()],
        };

        {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            for (d, tags) in chunk.iter().zip(assignments) {
                if let Some(tags) = tags {
                    retag::stage(&conn, d.id, &tags)?;
                }
            }
        }
        done += chunk.len();
        sink.send(RetagEvent::Progress { done, total });
    }

    // Count what the user will actually be shown, not what the model answered for. Staging a
    // proposal identical to the tags a document already carries is the overwhelmingly common
    // outcome on a well-tagged library, and counting those made the pass report "165 changed"
    // above a list of three. `pending` applies the same order-insensitive comparison the proposals
    // list does, so the number and the list can no longer disagree.
    let changed = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        retag::pending(&conn)?.len()
    };
    sink.send(RetagEvent::Finished { changed });
    Ok(())
}

/// The staged proposals that would actually change something, newest pass only.
#[tauri::command]
pub fn list_tag_proposals(state: State<'_, AppState>) -> Result<Vec<retag::TagProposalRow>> {
    let conn = state.conn()?;
    retag::pending(&conn)
}

/// Throw away a staged pass without applying any of it.
#[tauri::command]
pub fn discard_tag_proposals(state: State<'_, AppState>) -> Result<()> {
    {
        let conn = state.conn()?;
        retag::clear(&conn, None)?;
    }
    retag::clear_last_changed(&state);
    Ok(())
}

/// Apply staged re-tags to the chosen documents — **tags and nothing else** (#580).
///
/// Deliberately not routed through `commit_review`, which writes project + importance + tags
/// together and calls `log_corrections`. These documents are already reviewed and their filing is
/// the user's; sending them back through the review path would re-propose curated projects, land
/// blanks in Unsorted, and write corrections into the learning corpus that the user never made.
///
/// Each document's own project / importance / reviewed / last_activity are read and passed straight
/// back, exactly as `rewrite_documents` does for a rename — the write still goes through
/// `write_document_truth` (INVARIANTS I-02) so the vault frontmatter is rewritten and the change
/// survives the next Rebuild. `FilingActivity::Suppress`: a maintenance sweep is not per-project
/// engagement, and logging one observation per document would read as a burst of it.
///
/// All-or-nothing, like a review commit: the DB transaction and every vault file roll back together.
#[tauri::command]
pub async fn commit_retag(app: AppHandle, document_ids: Vec<i64>) -> Result<usize> {
    spawn_blocking_result("re-tag commit", move || -> Result<usize> {
        let state = app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, manifest_cipher) = state.manifest_io()?;

        let mut conn = state.conn()?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        let result: Result<usize> = (|| {
            let staged = retag::staged_for(&tx, &document_ids)?;
            let applied = rewrite_document_tags(
                &tx,
                &vault,
                &cipher,
                &vault_root,
                &manifest_cipher,
                &staged,
                &mut written,
            )?;
            let ids: Vec<i64> = staged.iter().map(|(id, _)| *id).collect();
            retag::clear(&tx, Some(&ids))?;
            Ok(applied)
        })();

        let out = finish_vault_transaction(tx, written, None, result);
        // After the transaction, and with the DB guard released first. Only on success: a commit
        // that rolled back leaves the proposals staged, so the count is still true of them.
        drop(conn);
        if out.is_ok() {
            retag::clear_last_changed(&state);
        }
        out
    })
    .await
}

/// Rewrite these documents' TAGS and nothing else, through the one filing writer (I-02).
///
/// The single seam behind every bulk tag change — accepting a re-tag pass, deleting a label
/// everywhere, folding two labels into one. Each document's own project / linked projects /
/// importance / reviewed / last_activity are read and passed straight back, so the only field that
/// can move is the one the caller asked to move. Going through `write_document_truth` is what makes
/// the change stick: `documents.tags` is the DB mirror, the vault's `tags:` line is the truth, and a
/// DB-only write is silently undone by the next Rebuild.
///
/// `FilingActivity::Suppress` throughout — tag maintenance is not per-project engagement, and one
/// observation per document would read as a burst of it (B6-6).
///
/// Appends to `written` rather than returning it, so a caller that fails midway still has every
/// file it touched available to roll back.
fn rewrite_document_tags(
    tx: &Connection,
    vault: &std::path::Path,
    cipher: &vault::MarkdownCipher,
    vault_root: &std::path::Path,
    manifest_cipher: &index_only::ManifestCipher,
    updates: &[(i64, Vec<String>)],
    written: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
) -> Result<usize> {
    let mut applied = 0usize;
    // See `commit_review`: the manifest is regenerated whole, so pushing it per document makes a bulk
    // sweep quadratic in library size. Deleting a label across a large index-only corpus is exactly
    // that shape, so this seam batches too and flushes once below (#722).
    let mut deferred_manifest = false;
    for (doc_id, tags) in updates {
        let row: Option<(String, Option<String>, i64, String)> = tx
            .query_row(
                "SELECT project, importance, reviewed, COALESCE(last_activity, ingested_at) \
                 FROM documents WHERE id = ?1",
                params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        // A document deleted since the caller read its id is skipped, not an error: these are bulk
        // maintenance sweeps, and one missing row must not strand the rest.
        let Some((project, importance, reviewed, last_activity)) = row else {
            continue;
        };
        let linked = crate::tags::linked_projects(tx, *doc_id, &project)?;
        let w = ingest::write_document_truth(
            tx,
            vault,
            cipher,
            *doc_id,
            &project,
            &linked,
            tags,
            importance.as_deref(),
            reviewed != 0,
            &last_activity,
            vault_root,
            manifest_cipher,
            ingest::FilingActivity::Suppress,
            ingest::ManifestWrite::Batched,
        )?;
        deferred_manifest |= w.is_none();
        written.extend(w);
        applied += 1;
    }
    // One push for the whole sweep, after every document's memberships have landed in the join the
    // manifest is regenerated from.
    if deferred_manifest {
        written.push(ingest::flush_manifest_batch(
            tx,
            vault_root,
            manifest_cipher,
        )?);
    }
    Ok(applied)
}

/// Remove a free-form tag from every document that carries it (#579).
///
/// "Everywhere" is the whole point, and it is three places, not one: the vault front-matter (the
/// truth), `documents.tags` (its mirror), and the `tags`/`document_tags` registry that search and
/// `@tag` read. Deleting only the registry row would leave the label in the vault, and the next
/// Rebuild would bring it straight back.
///
/// All-or-nothing: the DB transaction and every vault file roll back together, so a failure partway
/// through cannot leave half a library carrying a tag the other half has lost.
#[tauri::command]
pub async fn delete_tag(app: AppHandle, name: String) -> Result<usize> {
    spawn_blocking_result("tag delete", move || -> Result<usize> {
        let state = app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, manifest_cipher) = state.manifest_io()?;
        let mut conn = state.conn()?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        let result: Result<usize> = (|| {
            let norm = crate::tags::normalize(&name);
            let updates: Vec<(i64, Vec<String>)> =
                crate::tags::documents_with_group_tag(&tx, &name)?
                    .into_iter()
                    .map(|(id, tags)| {
                        let kept = tags
                            .into_iter()
                            .filter(|t| crate::tags::normalize(t) != norm)
                            .collect();
                        (id, kept)
                    })
                    .collect();
            let applied = rewrite_document_tags(
                &tx,
                &vault,
                &cipher,
                &vault_root,
                &manifest_cipher,
                &updates,
                &mut written,
            )?;
            // The registry row survives the rewrites (write_document_truth maintains the join, never
            // the tag table), so it has to go explicitly or the label lingers in the `@` menu and in
            // search as a tag that matches nothing.
            crate::tags::prune_orphan_group_tags(&tx)?;
            Ok(applied)
        })();

        finish_vault_transaction(tx, written, None, result)
    })
    .await
}

/// Rename a free-form tag everywhere, FOLDING into `new` if that tag already exists (#579).
///
/// Rename and fold are deliberately one operation rather than two, because from where the user
/// stands they are the same act — "these are the same thing, use this name" — and which one it is
/// depends only on whether the other name happens to exist yet. Splitting them would mean the button
/// changed meaning based on a fact the user has to look up first.
///
/// The fold arm has to deduplicate: a document carrying BOTH `tax` and `taxes` must come out with
/// one `tax`, not two identical labels.
#[tauri::command]
pub async fn rename_tag(app: AppHandle, old: String, new: String) -> Result<usize> {
    spawn_blocking_result("tag rename", move || -> Result<usize> {
        let state = app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, manifest_cipher) = state.manifest_io()?;
        let mut conn = state.conn()?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        let result: Result<usize> = (|| {
            let target = crate::retag::normalize_tag(&new);
            let old_norm = crate::tags::normalize(&old);
            if target.is_empty() || old_norm.is_empty() || old_norm == crate::tags::normalize(&new)
            {
                return Ok(0);
            }
            let updates: Vec<(i64, Vec<String>)> =
                crate::tags::documents_with_group_tag(&tx, &old)?
                    .into_iter()
                    .map(|(id, tags)| {
                        let mut out: Vec<String> = Vec::with_capacity(tags.len());
                        for t in tags {
                            let swapped = if crate::tags::normalize(&t) == old_norm {
                                target.clone()
                            } else {
                                t
                            };
                            // The fold arm: a document already carrying both names must not come out
                            // with the survivor twice.
                            if !out.iter().any(|k| {
                                crate::tags::normalize(k) == crate::tags::normalize(&swapped)
                            }) {
                                out.push(swapped);
                            }
                        }
                        (id, out)
                    })
                    .collect();
            let applied = rewrite_document_tags(
                &tx,
                &vault,
                &cipher,
                &vault_root,
                &manifest_cipher,
                &updates,
                &mut written,
            )?;
            crate::tags::prune_orphan_group_tags(&tx)?;
            Ok(applied)
        })();

        finish_vault_transaction(tx, written, None, result)
    })
    .await
}

/// Resolve a user-confirmed project name to its entity (creating a genuinely new one only if the
/// name resolves to nothing), returning the entity's canonical name + id. Blank falls back to the
/// always-present "Unsorted" entity, so a document always lands on a real entity.
fn resolve_canonical(conn: &Connection, name: &str) -> Result<(String, i64)> {
    let name = if name.trim().is_empty() {
        "Unsorted"
    } else {
        name.trim()
    };
    let id = entities::resolve_project(conn, name, true)?
        .ok_or_else(|| Error::Other("could not resolve project".into()))?;
    Ok((entities::canonical_name(conn, id)?, id))
}

/// Capture a model-proposed name the user corrected away as a forward-going alias of the chosen
/// entity — the rule that stops the variant recurring. The merge guard: a proposed name that
/// already resolves to a *different* entity is a merge, not an alias, so it is surfaced (logged in
/// PR 1; a Teach-tab button in PR 2), never silently folded (§1.5).
fn capture_alias(conn: &Connection, chosen_id: i64, proposed: &str) -> Result<()> {
    let proposed = proposed.trim();
    if proposed.is_empty() {
        return Ok(());
    }
    match entities::resolve_project(conn, proposed, false)? {
        Some(other) if other == chosen_id => {} // same entity — nothing new to learn
        Some(_) => eprintln!(
            "entities: \"{proposed}\" already names another project — surfaced as a merge \
             candidate, not folded"
        ),
        None => {
            if let entities::AddAlias::Conflict(_) = entities::add_alias(conn, chosen_id, proposed)?
            {
                eprintln!("entities: \"{proposed}\" is owned by another project — not folded");
            }
        }
    }
    Ok(())
}

/// Commit a review pass: for each decision, log the fields the user changed from
/// the AI proposal, then write the confirmed metadata to the vault + DB and mark
/// the document reviewed. Blocking (file rewrites), so it runs off the runtime.
#[tauri::command]
pub async fn commit_review(app: AppHandle, decisions: Vec<ReviewDecision>) -> Result<()> {
    let blocking_app = app.clone();
    spawn_blocking_result("commit", move || -> Result<usize> {
        let state = blocking_app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, rules_cipher) = state.rules_io()?;
        let (_, manifest_cipher) = state.manifest_io()?;

        // The whole pass is all-or-nothing: corrections, alias rules, vault rewrites, and the
        // `reviewed` flags commit together, or the DB transaction rolls back and every vault file
        // (plus the rules file) we touched is restored. Otherwise a failure partway through would
        // leave earlier docs marked reviewed (dropped from the queue on retry, their corrections
        // never re-logged) and mid-batch vault/DB drift.
        let mut conn = state.conn()?;
        let now = ingest::iso_now(&conn)?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        // Set when any decision took the index-only arm, which under `Batched` wrote its row and left
        // the shared manifest to us. False for an all-vault batch, where there is no manifest to push.
        let mut deferred_manifest = false;

        let result: Result<usize> = (|| {
            let mut logged = 0usize;
            for d in &decisions {
                let title: String = tx
                    .query_row(
                        "SELECT title FROM documents WHERE id = ?1",
                        params![d.document_id],
                        |r| r.get(0),
                    )
                    .unwrap_or_default();
                logged += review::log_corrections(&tx, d, &title)?;
                let importance = review::normalize_importance(d.importance.clone());
                // Resolve the confirmed project to its entity (creating a genuinely new one), and
                // write its CANONICAL name to the vault + DB cache — never a variant (invariant #2).
                let (canonical, entity_id) = resolve_canonical(&tx, &d.project)?;
                // Review confirms ONE project — the model proposes one, and extra memberships are
                // added by hand elsewhere — so this surface carries the existing ones across rather
                // than passing an empty list, which would silently unlink a document from
                // everywhere else the moment it was re-reviewed. The document is still homed at its
                // PRE-review project here (usually the Unsorted inbox); `linked_projects` excludes
                // that as well as `canonical`, or approving a file would link it to the inbox
                // forever — in the vault, so a Rebuild would reproduce it.
                let linked = crate::tags::linked_projects(&tx, d.document_id, &canonical)?;
                let w = ingest::write_document_truth(
                    &tx,
                    &vault,
                    &cipher,
                    d.document_id,
                    &canonical,
                    &linked,
                    &d.tags,
                    importance.as_deref(),
                    true,
                    &now,
                    &vault_root,
                    &manifest_cipher,
                    ingest::FilingActivity::Record,
                    // The manifest is regenerated whole from the mirror, so pushing it per document
                    // meant N whole-corpus scans and N whole-file encrypt+fsync cycles inside the
                    // held DB guard — quadratic in library size, and what made a 200-document
                    // Approve-all freeze the window (#722). The row still lands here; the file is
                    // flushed once below.
                    ingest::ManifestWrite::Batched,
                )?;
                deferred_manifest |= w.is_none();
                written.extend(w);
                entities::reassign_document(&tx, d.document_id, entity_id)?;
                // This document is leaving the review queue — drop its cached proposal (belt-and-braces
                // alongside the ON DELETE CASCADE that covers an actual deletion). Inside the tx, so it
                // rolls back with everything else if the commit fails.
                review::drop_cached_proposal(&tx, d.document_id)?;
                // Capture the model's corrected-away name as a forward-going alias (merge-guarded),
                // so the same variant resolves to this canonical next time instead of recurring.
                // A correction is also a deliberate vouch for the chosen entity — record it as
                // confirmed STATE (accepting the proposal unchanged does not confirm).
                if d.project.trim() != d.proposed_project.trim() {
                    capture_alias(&tx, entity_id, &d.proposed_project)?;
                    entities::set_confirmed(&tx, entity_id)?;
                }
            }
            // The batch's one manifest push. AFTER the loop, so every document's memberships are in
            // the join `mirror_items` reads — flushing earlier would write a manifest missing them,
            // and `reconcile_on_open` believes the FILE at the next launch, so that is data loss
            // rather than a stale readout. Before the tail, so its snapshot joins the rollback set.
            //
            // `ingest::flush_manifest_batch`, never `connector_sync::flush_manifest` — that one takes
            // `state.conn()` itself and this closure is already holding it. The DB mutex is not
            // reentrant, so the wrong call here is a permanent freeze, not a slow path.
            if deferred_manifest {
                written.push(ingest::flush_manifest_batch(
                    &tx,
                    &vault_root,
                    &manifest_cipher,
                )?);
            }
            Ok(logged)
        })();

        finish_vault_transaction(tx, written, Some((&vault_root, &rules_cipher)), result)
    })
    .await?;

    // The legacy correction→blob distiller is retired: the free-text "Learning You" profile is
    // frozen and the structured preference model (§4.5) replaces it. `corrections` keep logging
    // above — they feed the entity-alias loop and are the seam for the deferred Stage-5
    // inferred-preference learning. The one thing still owed once is migrating the legacy blob into
    // records; attempt it here too (a guaranteed-unlocked moment) — idempotent + best-effort.
    spawn_preferences_migration(app);
    Ok(())
}

/// Edit one already-reviewed document's metadata (the after-the-fact "this is
/// Project 2, not 3"). Logs the change against the currently stored values.
#[tauri::command]
pub async fn set_document_metadata(
    app: AppHandle,
    document_id: i64,
    project: String,
    also_projects: Vec<String>,
    tags: Vec<String>,
    importance: Option<String>,
) -> Result<Document> {
    let importance = review::normalize_importance(importance);
    spawn_blocking_result("update", move || -> Result<Document> {
        let state = app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, rules_cipher) = state.rules_io()?;
        let (_, manifest_cipher) = state.manifest_io()?;

        // Log the correction + rewrite the vault file + update the row atomically, restoring the
        // vault file (and rules file) if the DB side fails (the file writes land first). This is a
        // *reassignment* (one document moves), not a merge: no alias rule is captured — the prior
        // value is the document's own canonical, not a model-proposed variant.
        let mut conn = state.conn()?;
        let now = ingest::iso_now(&conn)?;
        let tx = conn.transaction()?;
        let mut written: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

        let work = (|| -> Result<()> {
            let (cur_project, cur_tags_json, cur_importance, title): (
                String,
                String,
                Option<String>,
                String,
            ) = tx.query_row(
                "SELECT project, tags, importance, title FROM documents WHERE id = ?1",
                params![document_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            let decision = ReviewDecision {
                document_id,
                project: project.clone(),
                tags: tags.clone(),
                importance: importance.clone(),
                proposed_project: cur_project,
                proposed_tags: serde_json::from_str(&cur_tags_json).unwrap_or_default(),
                proposed_importance: cur_importance,
                // This path keeps logging: unlike a hand-filed review row, the values it compares
                // against are the document's genuine stored before-state, so the difference is a
                // real after-the-fact correction. (The field is named for the Review view's case,
                // which is where it decides anything.)
                had_proposal: true,
            };
            review::log_corrections(&tx, &decision, &title)?;
            // Resolve to the canonical name + entity (a typed-in new project creates one), write the
            // canonical to the vault + DB cache, and repoint `entity_id`.
            let (canonical, entity_id) = resolve_canonical(&tx, &project)?;
            // The extra memberships are resolved through the SAME seam as the home, so a project
            // typed into the pill editor mints (or matches) exactly one entity however it is cased,
            // and the vault only ever records canonical names — never a variant (invariant #2).
            // Anything that resolves back to the home is dropped rather than stored twice.
            let mut linked: Vec<String> = Vec::new();
            for name in &also_projects {
                if name.trim().is_empty() {
                    continue;
                }
                let (other, _) = resolve_canonical(&tx, name)?;
                if crate::tags::normalize(&other) != crate::tags::normalize(&canonical)
                    && !linked
                        .iter()
                        .any(|p| crate::tags::normalize(p) == crate::tags::normalize(&other))
                {
                    linked.push(other);
                }
            }
            written.extend(ingest::write_document_truth(
                &tx,
                &vault,
                &cipher,
                document_id,
                &canonical,
                &linked,
                &tags,
                importance.as_deref(),
                true,
                &now,
                &vault_root,
                &manifest_cipher,
                ingest::FilingActivity::Record,
                // One document, one call — there is no later flush to defer to.
                ingest::ManifestWrite::PerDocument,
            )?);
            entities::reassign_document(&tx, document_id, entity_id)?;
            // A deliberate after-the-fact metadata edit vouches for the chosen entity — confirm it.
            entities::set_confirmed(&tx, entity_id)?;
            Ok(())
        })();

        // The rules file is persisted inside the tail (the resolve above may have created an
        // entity). The helper CONSUMES `tx`, which is what releases the borrow `conn` needs below.
        finish_vault_transaction(tx, written, Some((&vault_root, &rules_cipher)), work)?;
        ingest::load_document(&conn, document_id)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_context_trims_and_drops_blanks() {
        assert_eq!(folder_context(Some("Taxes 2025")), Some("Taxes 2025"));
        assert_eq!(folder_context(Some("  Taxes 2025  ")), Some("Taxes 2025"));
        // A document with no folder concept (vault / chat / photo), and a blank one, add nothing.
        assert_eq!(folder_context(None), None);
        assert_eq!(folder_context(Some("   ")), None);
        assert_eq!(folder_context(Some("")), None);
    }
}
