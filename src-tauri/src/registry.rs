// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The model registry (spec §21.4 — retrieval foundation, PR 1). Every embedder and
//! reranker the pipeline can use is a declarative entry here; ingestion and retrieval
//! reference a model by its **role** (the active embedder / the active reranker), never
//! by a hardcoded name. This is the seam that turns a model swap — a better embedder, a
//! multilingual mode (PR 2), a locally-trained reranker (Stage 4) — into a registry edit
//! rather than a code change. The vector dimension flows from the active embedder's
//! entry, so nothing hardcodes 384.
//!
//! Pure data + lookups: no DB, no Python, fully unit-testable.

use std::path::PathBuf;

/// What a model does in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Turns text into a vector stored in `chunk_vec` (index-time / stateful — changing
    /// it invalidates every stored vector, so a swap means a Rebuild).
    Embedder,
    /// Re-scores candidate passages at query time (stateless — swap freely, effect next
    /// query, no Rebuild).
    Reranker,
}

/// How a model is executed. One runtime today; the enum is the seam for adding a direct
/// `onnxruntime` path or a remote provider later without touching call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// Local ONNX via the Python sidecar's fastembed.
    OnnxFastembed,
}

/// Where a model's weights come from. `LocalPath` is a deliberate forward dependency for
/// the Stage-4 learned reranker (trained on-device, loaded from disk), so the registry
/// never assumes a downloadable id — registering that model becomes one new entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A Hugging Face repo id fastembed knows how to fetch (bundled, or custom in PR 2).
    HuggingFace(&'static str),
    /// A model living on disk (e.g. a locally-trained reranker). Constructed by the Stage-4
    /// learned-reranker work (and exercised in tests); a deliberate forward seam in PR 1.
    #[allow(dead_code)]
    LocalPath(PathBuf),
}

/// Pooling strategy applied to a model's token outputs to get one vector. Reranking
/// entries score a pair directly and use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    Mean,
    /// Used by the multilingual `bge-m3` embedder added in PR 2; a forward seam in PR 1.
    #[allow(dead_code)]
    Cls,
    None,
}

/// One model the pipeline can use. Embedder entries carry a real `dimension`; reranker
/// entries set it to 0 (they emit a relevance score, not a vector).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Stable identifier — also the fastembed model name when `source` is `HuggingFace`.
    pub id: &'static str,
    pub role: Role,
    /// Output vector width for embedders; 0 for rerankers.
    pub dimension: usize,
    /// The model's maximum input length in tokens — the splitter packs chunks under this.
    pub max_tokens: usize,
    /// The tokenizer identity (today the same as `id`; the splitter sizes chunks with the
    /// active embedder's tokenizer via the sidecar).
    pub tokenizer: &'static str,
    pub runtime: Runtime,
    pub source: Source,
    pub pooling: Pooling,
    /// A required input prefix (e.g. e5's `query:` / `passage:`); `None` for bge / ms-marco.
    pub prefix: Option<&'static str>,
    /// Whether to L2-normalize the output vector.
    pub normalize: bool,
}

/// The English embedder — bundled in fastembed, 384-d, ~90 MB. The default and, in PR 1,
/// the only active embedder; PR 2 lets a vault select a multilingual one at creation.
const BGE_SMALL_EN: ModelEntry = ModelEntry {
    id: "BAAI/bge-small-en-v1.5",
    role: Role::Embedder,
    dimension: 384,
    max_tokens: 512,
    tokenizer: "BAAI/bge-small-en-v1.5",
    runtime: Runtime::OnnxFastembed,
    source: Source::HuggingFace("BAAI/bge-small-en-v1.5"),
    pooling: Pooling::Mean,
    prefix: None,
    normalize: true,
};

/// The English cross-encoder reranker — bundled in fastembed, ~80 MB. Registered as the
/// default reranker; in PR 1 the rerank stage is **inert** (it executes in PR 2 with the
/// Settings toggle), so this entry is read but never run yet.
const MS_MARCO_MINILM: ModelEntry = ModelEntry {
    id: "Xenova/ms-marco-MiniLM-L-6-v2",
    role: Role::Reranker,
    dimension: 0,
    max_tokens: 512,
    tokenizer: "Xenova/ms-marco-MiniLM-L-6-v2",
    runtime: Runtime::OnnxFastembed,
    source: Source::HuggingFace("Xenova/ms-marco-MiniLM-L-6-v2"),
    pooling: Pooling::None,
    prefix: None,
    normalize: false,
};

/// The id of the default (and, in PR 1, only active) embedder.
const DEFAULT_EMBEDDER_ID: &str = "BAAI/bge-small-en-v1.5";
/// The id of the default reranker (registered now; executed in PR 2).
const DEFAULT_RERANKER_ID: &str = "Xenova/ms-marco-MiniLM-L-6-v2";

/// Every registered model. The pipeline looks models up here; nothing outside this module
/// names a model directly.
pub fn all() -> Vec<ModelEntry> {
    vec![BGE_SMALL_EN.clone(), MS_MARCO_MINILM.clone()]
}

/// Find a registered model by id.
pub fn lookup(id: &str) -> Option<ModelEntry> {
    all().into_iter().find(|m| m.id == id)
}

/// The active embedder — the one whose vectors are stored at ingest and used to embed a
/// query. PR 1 keeps this the English default (the dimension flows from here); PR 2 reads
/// a per-vault override from the retrieval stamp.
pub fn active_embedder() -> ModelEntry {
    lookup(DEFAULT_EMBEDDER_ID).expect("the default embedder is registered")
}

/// The active reranker. Read (e.g. by the gateway) but not executed in PR 1 — the rerank
/// stage is inert until PR 2 wires the cross-encoder + a Settings toggle.
pub fn active_reranker() -> ModelEntry {
    lookup(DEFAULT_RERANKER_ID).expect("the default reranker is registered")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_embedder_is_the_384d_english_model() {
        let e = active_embedder();
        assert_eq!(e.role, Role::Embedder);
        assert_eq!(e.dimension, 384);
        assert_eq!(e.id, "BAAI/bge-small-en-v1.5");
        assert!(e.max_tokens >= e.dimension); // sanity: a token budget, not a vector width
    }

    #[test]
    fn active_reranker_is_a_reranker_with_no_dimension() {
        let r = active_reranker();
        assert_eq!(r.role, Role::Reranker);
        assert_eq!(r.dimension, 0);
    }

    #[test]
    fn lookup_finds_registered_models_and_misses_unknown() {
        assert!(lookup("BAAI/bge-small-en-v1.5").is_some());
        assert!(lookup("Xenova/ms-marco-MiniLM-L-6-v2").is_some());
        assert!(lookup("nope/not-a-model").is_none());
    }

    #[test]
    fn every_entry_role_matches_its_dimension_convention() {
        for m in all() {
            match m.role {
                Role::Embedder => assert!(m.dimension > 0, "{} is an embedder with 0 dim", m.id),
                Role::Reranker => assert_eq!(m.dimension, 0, "{} is a reranker with a dim", m.id),
            }
        }
    }

    #[test]
    fn source_accepts_a_local_path() {
        // The forward dependency for the Stage-4 locally-trained reranker: the registry
        // must accept an on-disk model, not only a downloadable id.
        let s = Source::LocalPath(PathBuf::from("/models/reranker.onnx"));
        assert!(matches!(s, Source::LocalPath(_)));
    }
}
