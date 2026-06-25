// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The model registry (spec §21.4 — retrieval foundation). Every embedder and reranker the
//! pipeline can use is a declarative entry here; ingestion and retrieval reference a model by its
//! **role** (the active/selected embedder, the paired reranker), never by a hardcoded name. This is
//! the seam that turns a model swap — a multilingual mode (PR 2), a locally-trained reranker
//! (Stage 4) — into a registry edit rather than a code change. The vector dimension flows from the
//! embedder's entry, so nothing hardcodes 384.
//!
//! Pure data + lookups: no DB, no Python, fully unit-testable. The per-vault *selection* — which
//! embedder a vault uses — lives in `settings`; `db::selected_embedder` reads it and resolves it
//! here via [`embedder_or_default`].

use std::path::PathBuf;

/// What a model does in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Turns text into a vector stored in `chunk_vec` (index-time / stateful — changing it
    /// invalidates every stored vector, so a swap means a Rebuild).
    Embedder,
    /// Re-scores candidate passages at query time (stateless — swap freely, effect next query,
    /// no Rebuild).
    Reranker,
}

/// How a model is executed. One runtime today; the enum is the seam for adding a direct
/// `onnxruntime` path or a remote provider later without touching call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// Local ONNX via the Python sidecar's fastembed.
    OnnxFastembed,
}

/// Where a model's weights come from. `LocalPath` is a deliberate forward dependency for the
/// Stage-4 learned reranker (trained on-device, loaded from disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A Hugging Face repo id fastembed fetches. For a **bundled** model this is the model id
    /// itself; for a **custom** model (PR 2) it is the repo serving the ONNX export, which may
    /// differ from the model's logical `id` (e.g. an `onnx-community/…` mirror).
    HuggingFace(&'static str),
    /// A model living on disk (e.g. a locally-trained reranker). Constructed by the Stage-4
    /// learned-reranker work (and exercised in tests); a deliberate forward seam.
    #[allow(dead_code)]
    LocalPath(PathBuf),
}

/// Pooling strategy applied to a model's token outputs to get one vector. Reranking entries score
/// a pair directly and use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    Mean,
    /// Used by the multilingual `bge-m3` embedder (PR 3); a forward seam today.
    #[allow(dead_code)]
    Cls,
    None,
}

/// Which side of a retrieval pair a text is, so the right asymmetric prefix is applied: e5 needs
/// `query:` on the query and `passage:` on documents; symmetric models (bge) use neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedRole {
    Query,
    Passage,
}

/// One model the pipeline can use. Embedder entries carry a real `dimension`; reranker entries set
/// it to 0 (they emit a relevance score, not a vector).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Stable logical identifier — also the fastembed model name registered for this entry.
    pub id: &'static str,
    pub role: Role,
    /// Output vector width for embedders; 0 for rerankers.
    pub dimension: usize,
    /// The model's maximum input length in tokens — the splitter packs chunks under this.
    pub max_tokens: usize,
    /// The tokenizer identity (today the same as `id`; the splitter sizes chunks with the active
    /// embedder's tokenizer via the sidecar).
    pub tokenizer: &'static str,
    pub runtime: Runtime,
    pub source: Source,
    pub pooling: Pooling,
    /// Asymmetric retrieval prefixes (e5's `query: ` / `passage: `); `None` for symmetric models
    /// (bge, ms-marco). Applied in Rust by [`apply_prefix`] — the single source of truth — so a
    /// custom model fastembed doesn't auto-prefix is still embedded with the right input.
    pub query_prefix: Option<&'static str>,
    pub passage_prefix: Option<&'static str>,
    /// Whether to L2-normalize the output vector.
    pub normalize: bool,
    /// The ONNX file within the source repo for a **custom** model registered via fastembed's
    /// `add_custom_model` (e.g. `onnx/model.onnx`); `None` for a model fastembed bundles.
    pub model_file: Option<&'static str>,
    /// Whether this model covers many languages — drives the onboarding label and the
    /// embedder→reranker pairing ([`reranker_for`]).
    pub multilingual: bool,
    /// A short human label for the onboarding language picker.
    pub label: &'static str,
}

/// The English embedder — bundled in fastembed, 384-d, ~90 MB. The default; a vault may select a
/// multilingual one at creation (PR 2).
const BGE_SMALL_EN: ModelEntry = ModelEntry {
    id: "BAAI/bge-small-en-v1.5",
    role: Role::Embedder,
    dimension: 384,
    max_tokens: 512,
    tokenizer: "BAAI/bge-small-en-v1.5",
    runtime: Runtime::OnnxFastembed,
    source: Source::HuggingFace("BAAI/bge-small-en-v1.5"),
    pooling: Pooling::Mean,
    query_prefix: None,
    passage_prefix: None,
    normalize: true,
    model_file: None,
    multilingual: false,
    label: "English",
};

/// The multilingual embedder (PR 2) — `intfloat/multilingual-e5-small`, **384-d** so it shares the
/// existing `chunk_vec float[384]` (no schema change). Not bundled: registered via fastembed's
/// `add_custom_model` from the official ONNX export. e5 is asymmetric — the `query: ` / `passage: `
/// prefixes are mandatory and applied in Rust.
const MULTILINGUAL_E5_SMALL: ModelEntry = ModelEntry {
    id: "intfloat/multilingual-e5-small",
    role: Role::Embedder,
    dimension: 384,
    max_tokens: 512,
    tokenizer: "intfloat/multilingual-e5-small",
    runtime: Runtime::OnnxFastembed,
    source: Source::HuggingFace("intfloat/multilingual-e5-small"),
    pooling: Pooling::Mean,
    query_prefix: Some("query: "),
    passage_prefix: Some("passage: "),
    normalize: true,
    model_file: Some("onnx/model.onnx"),
    multilingual: true,
    label: "Multilingual",
};

/// The English cross-encoder reranker — bundled, ~80 MB. Paired with the English embedder. The
/// rerank stage executes in PR 2 (it was inert in PR 1) behind a Settings toggle.
const MS_MARCO_MINILM: ModelEntry = ModelEntry {
    id: "Xenova/ms-marco-MiniLM-L-6-v2",
    role: Role::Reranker,
    dimension: 0,
    max_tokens: 512,
    tokenizer: "Xenova/ms-marco-MiniLM-L-6-v2",
    runtime: Runtime::OnnxFastembed,
    source: Source::HuggingFace("Xenova/ms-marco-MiniLM-L-6-v2"),
    pooling: Pooling::None,
    query_prefix: None,
    passage_prefix: None,
    normalize: false,
    model_file: None,
    multilingual: false,
    label: "English",
};

/// The multilingual cross-encoder reranker (PR 2) — `bge-reranker-v2-m3`, paired with the
/// multilingual embedder. Not bundled: registered via `add_custom_model` from a community ONNX
/// export (int8, ~570 MB). Replaces the deprecated jina-v2-multilingual reranker.
const BGE_RERANKER_V2_M3: ModelEntry = ModelEntry {
    id: "BAAI/bge-reranker-v2-m3",
    role: Role::Reranker,
    dimension: 0,
    max_tokens: 512,
    tokenizer: "BAAI/bge-reranker-v2-m3",
    runtime: Runtime::OnnxFastembed,
    source: Source::HuggingFace("onnx-community/bge-reranker-v2-m3-ONNX"),
    pooling: Pooling::None,
    query_prefix: None,
    passage_prefix: None,
    normalize: false,
    model_file: Some("onnx/model_int8.onnx"),
    multilingual: true,
    label: "Multilingual",
};

/// The id of the default (English) embedder — the build default and the fallback for an unknown
/// per-vault selection.
const DEFAULT_EMBEDDER_ID: &str = "BAAI/bge-small-en-v1.5";
/// The id of the default (English) reranker.
const DEFAULT_RERANKER_ID: &str = "Xenova/ms-marco-MiniLM-L-6-v2";
/// The id of the multilingual reranker, paired with a multilingual embedder.
const MULTILINGUAL_RERANKER_ID: &str = "BAAI/bge-reranker-v2-m3";

/// Every registered model. The pipeline looks models up here; nothing outside this module names a
/// model directly.
pub fn all() -> Vec<ModelEntry> {
    vec![
        BGE_SMALL_EN.clone(),
        MULTILINGUAL_E5_SMALL.clone(),
        MS_MARCO_MINILM.clone(),
        BGE_RERANKER_V2_M3.clone(),
    ]
}

/// Find a registered model by id.
pub fn lookup(id: &str) -> Option<ModelEntry> {
    all().into_iter().find(|m| m.id == id)
}

/// The default (English) embedder — the build default the stamp uses in tests, and the fallback
/// when a vault has no (or an unknown) selection.
pub fn active_embedder() -> ModelEntry {
    lookup(DEFAULT_EMBEDDER_ID).expect("the default embedder is registered")
}

/// The default (English) reranker.
pub fn active_reranker() -> ModelEntry {
    lookup(DEFAULT_RERANKER_ID).expect("the default reranker is registered")
}

/// Resolve a stored embedder id to its entry, falling back to the English default if the id is
/// unknown or names a non-embedder. The per-vault selection (settings `embedding_model`) flows
/// through here, so a corrupt or stale value can never break ingest.
pub fn embedder_or_default(id: &str) -> ModelEntry {
    lookup(id)
        .filter(|m| m.role == Role::Embedder)
        .unwrap_or_else(active_embedder)
}

/// The reranker paired with an embedder — a multilingual embedder ⇒ the multilingual reranker,
/// otherwise the English one. Deriving the pair (rather than storing it) **structurally enforces**
/// the pairing rule: a multilingual reranker can never sit on the English embedder.
pub fn reranker_for(embedder: &ModelEntry) -> ModelEntry {
    if embedder.multilingual {
        lookup(MULTILINGUAL_RERANKER_ID).expect("the multilingual reranker is registered")
    } else {
        active_reranker()
    }
}

/// The embedders offered at vault creation. PR 2 ships two, both 384-d (so neither changes the
/// vector schema); a future higher-dimension embedder (PR 3) would gate this on the vault's
/// dimension.
pub fn selectable_embedders() -> Vec<ModelEntry> {
    all()
        .into_iter()
        .filter(|m| m.role == Role::Embedder)
        .collect()
}

/// Prepend the embedder's role-appropriate retrieval prefix to each text (e5's `query: ` /
/// `passage: `). A no-op for symmetric models. The one place prefixes are applied, so the sidecar
/// embeds raw text and a custom (non-auto-prefixing) model still gets the right input.
pub fn apply_prefix(embedder: &ModelEntry, role: EmbedRole, texts: &[String]) -> Vec<String> {
    let prefix = match role {
        EmbedRole::Query => embedder.query_prefix,
        EmbedRole::Passage => embedder.passage_prefix,
    };
    match prefix {
        Some(p) => texts.iter().map(|t| format!("{p}{t}")).collect(),
        None => texts.to_vec(),
    }
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
        assert!(!e.multilingual);
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
        assert!(lookup("intfloat/multilingual-e5-small").is_some());
        assert!(lookup("Xenova/ms-marco-MiniLM-L-6-v2").is_some());
        assert!(lookup("BAAI/bge-reranker-v2-m3").is_some());
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
    fn the_multilingual_embedder_is_384d_with_asymmetric_prefixes() {
        let e = lookup("intfloat/multilingual-e5-small").unwrap();
        assert_eq!(e.role, Role::Embedder);
        assert_eq!(
            e.dimension, 384,
            "e5-small must share the float[384] vec table"
        );
        assert!(e.multilingual);
        assert_eq!(e.query_prefix, Some("query: "));
        assert_eq!(e.passage_prefix, Some("passage: "));
        assert!(
            e.model_file.is_some(),
            "e5-small is custom — needs an ONNX file"
        );
        assert!(matches!(e.source, Source::HuggingFace(_)));
    }

    #[test]
    fn reranker_pairing_follows_the_embedder_language() {
        let english = lookup("BAAI/bge-small-en-v1.5").unwrap();
        let multilingual = lookup("intfloat/multilingual-e5-small").unwrap();
        assert_eq!(reranker_for(&english).id, "Xenova/ms-marco-MiniLM-L-6-v2");
        assert_eq!(reranker_for(&multilingual).id, "BAAI/bge-reranker-v2-m3");
        // The pairing can never put a multilingual reranker on an English embedder.
        assert!(!reranker_for(&english).multilingual);
        assert!(reranker_for(&multilingual).multilingual);
    }

    #[test]
    fn every_selectable_multilingual_embedder_pairs_with_a_multilingual_reranker() {
        for e in selectable_embedders() {
            assert_eq!(e.role, Role::Embedder);
            if e.multilingual {
                assert!(
                    reranker_for(&e).multilingual,
                    "{} is multilingual but its reranker isn't",
                    e.id
                );
            }
        }
    }

    #[test]
    fn selectable_embedders_all_share_the_vector_dimension() {
        // PR 2 must not introduce an off-dimension selectable embedder (that needs PR 3's vec
        // drop+recreate); they must all match the default's width.
        let dim = active_embedder().dimension;
        let selectable = selectable_embedders();
        assert!(selectable.len() >= 2, "expected English + multilingual");
        for e in selectable {
            assert_eq!(e.dimension, dim, "{} has an off-dimension width", e.id);
        }
    }

    #[test]
    fn embedder_or_default_resolves_and_falls_back() {
        assert_eq!(
            embedder_or_default("intfloat/multilingual-e5-small").id,
            "intfloat/multilingual-e5-small"
        );
        // Unknown id → English default.
        assert_eq!(
            embedder_or_default("nope/not-a-model").id,
            active_embedder().id
        );
        // A reranker id is not an embedder → English default, never the reranker.
        assert_eq!(
            embedder_or_default("Xenova/ms-marco-MiniLM-L-6-v2").id,
            active_embedder().id
        );
    }

    #[test]
    fn apply_prefix_is_asymmetric_for_e5_and_noop_for_bge() {
        let e5 = lookup("intfloat/multilingual-e5-small").unwrap();
        let bge = lookup("BAAI/bge-small-en-v1.5").unwrap();
        let texts = vec!["hello".to_string(), "world".to_string()];

        let q = apply_prefix(&e5, EmbedRole::Query, &texts);
        assert_eq!(q, vec!["query: hello", "query: world"]);
        let p = apply_prefix(&e5, EmbedRole::Passage, &texts);
        assert_eq!(p, vec!["passage: hello", "passage: world"]);

        // bge is symmetric: text is untouched for either role.
        assert_eq!(apply_prefix(&bge, EmbedRole::Query, &texts), texts);
        assert_eq!(apply_prefix(&bge, EmbedRole::Passage, &texts), texts);
    }

    #[test]
    fn source_accepts_a_local_path() {
        // The forward dependency for the Stage-4 locally-trained reranker: the registry must
        // accept an on-disk model, not only a downloadable id.
        let s = Source::LocalPath(PathBuf::from("/models/reranker.onnx"));
        assert!(matches!(s, Source::LocalPath(_)));
    }
}
