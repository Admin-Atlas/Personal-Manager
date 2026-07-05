// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The retrieval-config stamp (spec §21.4 — retrieval foundation, PR 1). One value capturing
//! the whole *index-time* bucket — chunk rules, the embedder identity, the vector dimension —
//! so a vault can record which config produced its index. When the current config differs from
//! a vault's stored stamp, the index is stale and a one-time Rebuild brings it back in line
//! (stored via [`crate::db::get_retrieval_stamp`] / [`crate::db::set_retrieval_stamp`], surfaced
//! as the Documents-view Rebuild prompt).
//!
//! One mechanism replaces N hand-written migrations: any future change to a model or a chunk
//! rule bumps the stamp instead of needing bespoke migration code. It is also the forward
//! dependency for Stage-5 deterministic sync — a receiving device must know which config
//! produced the source index before it can rebuild one that matches.

use serde::{Deserialize, Serialize};

use crate::registry;
use crate::splitter;

/// Format version of the stamp itself (distinct from the splitter version it carries). Bump if
/// the *meaning* of a stamp field changes in a way equality wouldn't otherwise catch.
const STAMP_VERSION: u32 = 1;

/// The index-time configuration of a vault. Equality is field-wise: any difference means the
/// stored index no longer matches what this build would produce, so a Rebuild is offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// Stamp format version.
    pub version: u32,
    /// Target leaf-chunk size, in tokens.
    pub chunk_target_tokens: usize,
    /// Token overlap carried between adjacent leaf chunks.
    pub chunk_overlap_tokens: usize,
    /// Whether the splitter prepends the title + heading breadcrumb into the embedded text.
    pub prepend_headings: bool,
    /// The boundary-strategy id (e.g. `recursive-structure-v1`).
    pub boundary_strategy: String,
    /// The splitter implementation version — a change here intentionally invalidates chunks.
    pub splitter_version: u32,
    /// The active embedder's id. Vectors from two embedders are never comparable, so a change
    /// here is the single most important reason to re-embed.
    pub embedder_id: String,
    /// The vector dimension — derived from the active embedder, never hardcoded.
    pub dimension: usize,
    /// A string seam for fusion/index knobs that affect stored structure.
    pub index_params: String,
    /// The `chunks_fts` tokenisation mode — `cjk-bigram-v1` on a multilingual vault (CJK/kana/Hangul
    /// runs pre-segmented into bigrams so keyword search works, F-33), otherwise `none`. The default
    /// keeps English vaults' stamp byte-identical to before, so **only** a vault already on the
    /// multilingual embedder sees a mismatch and is offered the one-time Rebuild. Serde-defaulted so
    /// a stamp written before this field parses cleanly (to `none`) instead of failing to deserialize
    /// and forcing every vault to rebuild.
    #[serde(default = "default_fts_segmentation")]
    pub fts_segmentation: String,
}

/// The pre-field value: a stamp written before FTS segmentation existed described a plain
/// unicode61 index, i.e. no CJK segmentation — so it defaults to `"none"`, which equals what an
/// English vault produces today and therefore never triggers a spurious Rebuild.
fn default_fts_segmentation() -> String {
    "none".to_string()
}

impl RetrievalConfig {
    /// The config this build would produce *now* for a **given** embedder: its id + dimension,
    /// with the chunk fields from the splitter. This is what a vault is stamped with and compared
    /// against on open — the embedder is per-vault (PR 2), so the caller resolves the vault's
    /// selection (`db::selected_embedder`) and passes it here.
    pub fn current_for(embedder: &registry::ModelEntry) -> Self {
        RetrievalConfig {
            version: STAMP_VERSION,
            chunk_target_tokens: splitter::CHUNK_TARGET_TOKENS,
            chunk_overlap_tokens: splitter::CHUNK_OVERLAP_TOKENS,
            prepend_headings: true,
            boundary_strategy: splitter::BOUNDARY_STRATEGY.to_string(),
            splitter_version: splitter::SPLITTER_VERSION,
            embedder_id: embedder.id.to_string(),
            dimension: embedder.dimension,
            index_params: "hybrid-rrf-k60-recency90".to_string(),
            // Gated on the registry capability, never a model id (model-agnostic). This is the
            // whole delta that makes ONLY multilingual vaults rebuild for F-33 — no STAMP_VERSION
            // bump, which would drag English vaults into a rebuild too.
            fts_segmentation: if embedder.multilingual {
                "cjk-bigram-v1".to_string()
            } else {
                "none".to_string()
            },
        }
    }

    /// The config for the **default** (English) embedder — the build default, used by tests.
    /// Production resolves the per-vault embedder (`db::selected_embedder`) and calls
    /// [`current_for`], so this convenience is unused in the lib build itself.
    #[allow(dead_code)]
    pub fn current() -> Self {
        Self::current_for(&registry::active_embedder())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_is_deterministic() {
        assert_eq!(RetrievalConfig::current(), RetrievalConfig::current());
    }

    #[test]
    fn current_derives_dimension_from_the_registry() {
        let cfg = RetrievalConfig::current();
        assert_eq!(cfg.dimension, registry::active_embedder().dimension);
        assert_eq!(cfg.embedder_id, registry::active_embedder().id);
    }

    #[test]
    fn serde_round_trips() {
        let cfg = RetrievalConfig::current();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RetrievalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn a_field_change_breaks_equality() {
        let mut other = RetrievalConfig::current();
        other.splitter_version += 1;
        assert_ne!(other, RetrievalConfig::current());
    }

    #[test]
    fn current_for_takes_the_embedder_id_and_dimension() {
        let e5 = registry::lookup("intfloat/multilingual-e5-large").unwrap();
        let cfg = RetrievalConfig::current_for(&e5);
        assert_eq!(cfg.embedder_id, "intfloat/multilingual-e5-large");
        assert_eq!(cfg.dimension, 1024);
        // A different embedder ⇒ a different stamp ⇒ a Rebuild is offered.
        assert_ne!(cfg, RetrievalConfig::current());
    }

    #[test]
    fn fts_segmentation_is_gated_on_the_multilingual_embedder() {
        // English default → "none" (byte-identical to pre-field vaults, so no rebuild); the
        // multilingual embedder → the CJK-bigram marker (F-33), so those vaults — and only those —
        // see a stamp mismatch and are offered the Rebuild.
        assert_eq!(RetrievalConfig::current().fts_segmentation, "none");
        let e5 = registry::lookup("intfloat/multilingual-e5-large").unwrap();
        assert!(e5.multilingual);
        assert_eq!(
            RetrievalConfig::current_for(&e5).fts_segmentation,
            "cjk-bigram-v1"
        );
    }

    #[test]
    fn a_stamp_written_before_the_field_defaults_to_none() {
        // The load-bearing serde default: a stamp serialized before fts_segmentation existed must
        // deserialize to "none", NOT fail to parse. get_retrieval_stamp swallows a parse error into
        // None (a mismatch), so a non-defaulted field would force EVERY vault — English included —
        // to rebuild. With the default, an English vault's old stamp still compares equal.
        let mut value = serde_json::to_value(RetrievalConfig::current()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("fts_segmentation")
            .expect("field present in the current serialization");
        let back: RetrievalConfig = serde_json::from_value(value).unwrap();
        assert_eq!(back.fts_segmentation, "none");
        // …and that reconstructed stamp still equals the current English stamp → no spurious rebuild.
        assert_eq!(back, RetrievalConfig::current());
    }

    #[test]
    fn segmentation_difference_alone_breaks_equality() {
        let mut other = RetrievalConfig::current();
        other.fts_segmentation = "cjk-bigram-v1".to_string();
        assert_ne!(other, RetrievalConfig::current());
    }

    #[test]
    fn the_reranker_is_never_in_the_stamp() {
        // The reranker is query-time/stateless: toggling or swapping it must never change the
        // stamp (never trigger a Rebuild). Guard the serialized shape so a future field can't
        // sneak the reranker in.
        let json = serde_json::to_string(&RetrievalConfig::current()).unwrap();
        assert!(
            !json.to_lowercase().contains("rerank"),
            "the reranker must not appear in the index-time stamp: {json}"
        );
    }
}
