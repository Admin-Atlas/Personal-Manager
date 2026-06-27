// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The model gateway (spec §21.4 — retrieval foundation): the single chokepoint every external
//! model-inference call routes through. It forwards to the sidecar and applies the registry's
//! model semantics (the asymmetric retrieval prefixes), so call sites stay model-agnostic.
//! Caching, batching, retries, cost caps, and provider swaps will live here later without touching
//! call sites.
//!
//! A gateway carries the **resolved** models for an operation — the vault's selected embedder and
//! the reranker paired with it (`registry::reranker_for`). The caller resolves them once (a short
//! `db::selected_embedder` read) and hands them in, so the gateway never needs the DB lock and can
//! be used for the off-lock rerank. Cheap to construct per operation.

use crate::error::Result;
use crate::registry::{self, EmbedRole, ModelEntry};
use crate::retrieval::Reranker;
use crate::sidecar::SidecarManager;
use crate::splitter::TokenCounter;

/// A borrow of the sidecar plus this operation's resolved embedder + reranker.
pub struct ModelGateway<'a> {
    sidecar: &'a SidecarManager,
    embedder: ModelEntry,
    reranker: ModelEntry,
    /// Max chunks embedded per sidecar forward pass at index time; `None` = the embedder's own
    /// default (max throughput). Set small in "gentle" indexing mode to cap peak memory. Only
    /// affects passage (document) embedding — query embedding is a single text, never batched down.
    embed_batch: Option<usize>,
}

impl<'a> ModelGateway<'a> {
    /// Build a gateway from an operation's resolved models (the vault's embedder + the reranker
    /// paired with it via [`registry::reranker_for`]).
    pub fn new(sidecar: &'a SidecarManager, embedder: ModelEntry, reranker: ModelEntry) -> Self {
        Self {
            sidecar,
            embedder,
            reranker,
            embed_batch: None,
        }
    }

    /// Cap the index-time embedding batch (the "gentle" memory lever). Fluent so call sites read
    /// top-down: `state.gateway(&conn)?.with_embed_batch(db::indexing_embed_batch(&conn))`.
    pub fn with_embed_batch(mut self, batch: Option<usize>) -> Self {
        self.embed_batch = batch;
        self
    }

    /// This gateway's resolved embedder — so ingest can read its dimension (the index guard) and
    /// stamp the vault with it.
    pub fn embedder(&self) -> &ModelEntry {
        &self.embedder
    }

    /// Embed search queries with the active embedder, applying its query-side prefix (e5's
    /// `query: `; a no-op for symmetric models). Used at query time. A query is a single text, so
    /// the gentle batch cap never applies here — always the embedder default.
    pub fn embed_query(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed = registry::apply_prefix(&self.embedder, EmbedRole::Query, texts);
        self.sidecar.embed(&prefixed, &self.embedder, None)
    }

    /// Embed documents/passages with the active embedder, applying its passage-side prefix (e5's
    /// `passage: `; a no-op for symmetric models). Used at index time, so it honours the gentle
    /// batch cap (bounded peak memory) when one is set.
    pub fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed = registry::apply_prefix(&self.embedder, EmbedRole::Passage, texts);
        self.sidecar
            .embed(&prefixed, &self.embedder, self.embed_batch)
    }

    /// Token counts under the active embedder's tokenizer — the splitter sizes chunks by this.
    pub fn count_tokens(&self, texts: &[String]) -> Result<Vec<usize>> {
        self.sidecar.count_tokens(texts, &self.embedder)
    }
}

/// The splitter sizes chunks by tokens through this adapter, so its core stays Python-free and
/// unit-testable (tests inject a whitespace counter instead).
impl TokenCounter for ModelGateway<'_> {
    fn count(&self, texts: &[&str]) -> Result<Vec<usize>> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.count_tokens(&owned)
    }
}

/// The query-time rerank seam — runs the paired cross-encoder on the candidate passages. It is
/// **best-effort**: any sidecar failure (e.g. the model is still downloading on first use)
/// degrades to `Ok(None)`, so the fused order stands and search never breaks. The caller gates
/// *whether* to rerank (the Settings toggle) by passing the gateway or `None`; when called, this
/// always executes. Runs off the DB lock (the caller drops the conn guard before reranking).
impl Reranker for ModelGateway<'_> {
    fn scores(&self, query: &str, passages: &[&str]) -> Result<Option<Vec<f32>>> {
        match self.sidecar.rerank(query, passages, &self.reranker) {
            Ok(scores) => Ok(Some(scores)),
            Err(_) => Ok(None),
        }
    }
}
