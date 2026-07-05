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
        self.embed_within_line_budget(&prefixed, None)
    }

    /// Embed documents/passages with the active embedder, applying its passage-side prefix (e5's
    /// `passage: `; a no-op for symmetric models). Used at index time, so it honours the gentle
    /// batch cap (bounded peak memory) when one is set.
    pub fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed = registry::apply_prefix(&self.embedder, EmbedRole::Passage, texts);
        self.embed_within_line_budget(&prefixed, self.embed_batch)
    }

    /// Embed `texts` in sub-batches each kept under a request-line byte budget, concatenating the
    /// vectors in input order. One sidecar request serializing all of a large document's chunks at
    /// once could exceed the line size the child *silently drops*, wedging the sidecar
    /// (F-06 / B3-1); batching keeps every request well under it while preserving the 1:1
    /// input→vector mapping. `batch` is the separate per-forward-pass memory cap, passed straight
    /// through. Empty input makes no sidecar call at all.
    fn embed_within_line_budget(
        &self,
        texts: &[String],
        batch: Option<usize>,
    ) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::with_capacity(texts.len());
        for group in split_by_byte_budget(texts, REQUEST_TEXT_BUDGET) {
            vectors.extend(self.sidecar.embed(group, &self.embedder, batch)?);
        }
        Ok(vectors)
    }

    /// Token counts under the active embedder's tokenizer — the splitter sizes chunks by this.
    /// Batched under the same request-line byte budget as embedding, for the same reason (a large
    /// document's texts would otherwise serialize into one over-cap request line), preserving the
    /// 1:1 input→count order.
    pub fn count_tokens(&self, texts: &[String]) -> Result<Vec<usize>> {
        let mut counts = Vec::with_capacity(texts.len());
        for group in split_by_byte_budget(texts, REQUEST_TEXT_BUDGET) {
            counts.extend(self.sidecar.count_tokens(group, &self.embedder)?);
        }
        Ok(counts)
    }
}

/// Per-request text-byte budget for embed / count-token batching. Kept well under the sidecar's
/// `MAX_SIDECAR_REQUEST_LINE` (48 MiB) so that even after JSON wrapping + the retrieval prefix a
/// batch's request line stays comfortably below the size the child would silently drop
/// (F-06 / B3-1). 16 MiB of text is only a handful of requests even for a very large document,
/// and the per-request overhead is dwarfed by the embedding forward pass itself.
const REQUEST_TEXT_BUDGET: usize = 16 * 1024 * 1024;

/// Group `texts` into consecutive runs whose combined byte length stays within `budget`, so no
/// single sidecar request line approaches the size the child silently drops. Order is preserved
/// and every text lands in exactly one group, so concatenating per-group results reproduces a
/// 1:1 mapping back to the input. A lone text larger than `budget` becomes its own group — it
/// can't be split without changing what gets embedded, and the request-line guard in
/// `sidecar::request` is the backstop there, but the splitter keeps individual chunks far below
/// the budget so it doesn't arise in normal ingestion. Returns no groups for empty input.
fn split_by_byte_budget(texts: &[String], budget: usize) -> Vec<&[String]> {
    let mut groups = Vec::new();
    let mut start = 0usize;
    let mut running = 0usize;
    for (i, t) in texts.iter().enumerate() {
        // Close the open group before a text that would push it over budget — unless the group is
        // empty, in which case a single oversized text has to go on its own.
        if running > 0 && running + t.len() > budget {
            groups.push(&texts[start..i]);
            start = i;
            running = 0;
        }
        running += t.len();
    }
    if start < texts.len() {
        groups.push(&texts[start..]);
    }
    groups
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(specs: &[usize]) -> Vec<String> {
        specs.iter().map(|&n| "x".repeat(n)).collect()
    }

    /// Concatenating the groups reproduces the input, in order, for every case.
    fn assert_reproduces(texts: &[String], budget: usize) -> Vec<&[String]> {
        let groups = split_by_byte_budget(texts, budget);
        let flat: Vec<&String> = groups.iter().flat_map(|g| g.iter()).collect();
        let want: Vec<&String> = texts.iter().collect();
        assert_eq!(
            flat, want,
            "groups must concatenate back to the input in order"
        );
        groups
    }

    #[test]
    fn empty_input_yields_no_groups() {
        // No groups → the gateway makes no sidecar call at all.
        assert!(split_by_byte_budget(&[], REQUEST_TEXT_BUDGET).is_empty());
    }

    #[test]
    fn everything_under_budget_is_one_group() {
        let texts = strings(&[10, 20, 30]);
        let groups = assert_reproduces(&texts, 1000);
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn oversized_batch_splits_into_budget_bounded_groups() {
        // 5 texts of 30 bytes, budget 100 → groups of at most 3 (90 ≤ 100, a 4th would be 120).
        let texts = strings(&[30, 30, 30, 30, 30]);
        let groups = assert_reproduces(&texts, 100);
        assert!(groups.len() > 1, "must split");
        for g in &groups {
            let total: usize = g.iter().map(|s| s.len()).sum();
            assert!(total <= 100, "each group stays within budget, got {total}");
        }
    }

    #[test]
    fn a_group_may_exactly_equal_the_budget() {
        // Two 50-byte texts fit exactly in a 100-byte budget; the third opens a new group.
        let texts = strings(&[50, 50, 50]);
        let groups = assert_reproduces(&texts, 100);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[1].len(), 1);
    }

    #[test]
    fn a_single_oversized_text_stands_alone() {
        // A lone text over budget can't be split; it becomes its own group rather than being
        // dropped or wedging — the request-line guard is the hard backstop beyond this.
        let texts = strings(&[10, 500, 10]);
        let groups = assert_reproduces(&texts, 100);
        // The 500-byte text is isolated in a group of exactly one.
        assert!(
            groups.iter().any(|g| g.len() == 1 && g[0].len() == 500),
            "the oversized text must be alone in its own group"
        );
    }
}
