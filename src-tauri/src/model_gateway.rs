// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The model gateway (spec §21.4 — retrieval foundation, PR 1): the single chokepoint every
//! external model-inference call routes through. Today it just forwards to the sidecar — it is
//! deliberately the *seam*, not the cache. Caching, batching, retries, cost caps, and provider
//! swaps will live here later without touching call sites. Model ids come from the registry, so
//! the pipeline references a *role* (the active embedder / reranker), never a hardcoded name.

use crate::error::Result;
use crate::registry;
use crate::retrieval::Reranker;
use crate::sidecar::SidecarManager;
use crate::splitter::TokenCounter;

/// A borrow of the sidecar with the registry-driven model roles resolved. Cheap to construct
/// ([`crate::AppState::gateway`]), so callers make one per operation rather than storing it.
pub struct ModelGateway<'a> {
    sidecar: &'a SidecarManager,
}

impl<'a> ModelGateway<'a> {
    pub fn new(sidecar: &'a SidecarManager) -> Self {
        Self { sidecar }
    }

    /// Embed a batch with the active embedder. PR 1 has exactly one; PR 2 selects the model
    /// here from the vault's stamp.
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let _embedder = registry::active_embedder();
        self.sidecar.embed(texts)
    }

    /// Token counts under the active embedder's tokenizer — the splitter sizes chunks by this.
    pub fn count_tokens(&self, texts: &[String]) -> Result<Vec<usize>> {
        self.sidecar.count_tokens(texts)
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

/// The query-time rerank seam. **Inert in PR 1**: it resolves the active reranker but returns
/// `None`, leaving the fused order unchanged (no behaviour change for existing users). PR 2
/// runs the cross-encoder here behind a Settings toggle — stateless, so no Rebuild.
impl Reranker for ModelGateway<'_> {
    fn scores(&self, _query: &str, _passages: &[&str]) -> Result<Option<Vec<f32>>> {
        let _reranker = registry::active_reranker();
        Ok(None)
    }
}
