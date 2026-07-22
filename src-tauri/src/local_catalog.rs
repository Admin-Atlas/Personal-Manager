// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The curated local-model catalog (#296): a small, in-repo table of GGUF models with their real
//! per-quant sizes, architecture, context window, and (for MoE) active-parameter count. It is
//! generated from Hugging Face by `scripts/generate-local-catalog.mjs` and embedded at compile time
//! via `include_str!` — so it ships and auto-updates with the binary, no runtime file or network.
//!
//! This module only *reads* the embedded JSON. It bridges catalog rows into `fit::ModelSpec` for
//! scoring, answers the context-window "catalog rung" for the endpoint window ladder, and best-effort
//! matches a user's installed model name back to a catalog row.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::fit;

/// The whole catalog file, stamp + entries.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    // Schema-shape fields: carried so `deny_unknown_fields` validates the whole file (a drift guard)
    // and the parse-guard test can assert on them. Not otherwise consumed this stage.
    #[allow(dead_code)]
    pub schema_version: u32,
    /// Monotonic content version — the app compares it against the last one it evaluated to know a
    /// shipped update carried a fresher catalog (drives rescan-on-catalog-update).
    pub catalog_version: u32,
    #[allow(dead_code)]
    pub content_hash: String,
    /// When the catalog content last changed (UTC date) — surfaced to the Workbench.
    pub generated_at: String,
    #[allow(dead_code)]
    pub source: String,
    pub entries: Vec<CatalogEntry>,
}

/// One curated model.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub repo: String,
    pub display_name: String,
    pub architecture: String,
    pub role_hint: Option<String>,
    pub parameters_b: f64,
    /// Active params (== total for dense; the smaller MoE figure, read from the GGUF header).
    pub active_parameters_b: f64,
    pub context_length: u32,
    pub multimodal: bool,
    pub reasoning: Option<bool>,
    /// The vision projector's size in GB, when multimodal. The generator guarantees a multimodal
    /// entry always carries a projector size (it drops the flag otherwise), so this is `Some` iff
    /// `multimodal`.
    pub projector_gb: Option<f64>,
    pub fit: FitClass,
    pub quants: Vec<CatalogQuant>,
    pub install: InstallHints,
}

/// One downloadable quantization with its measured on-disk size.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogQuant {
    pub quant: String,
    pub file_gb: f64,
    pub sharded: bool,
}

/// Optional per-runtime install hints (Ollama has no catalog API, so these are curated, often null).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallHints {
    pub ollama: Option<String>,
}

/// Whether the app can compute a trustworthy fit for this entry (`unknown` = an unmodelled arch we
/// won't guess at — surfaced honestly, never scored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FitClass {
    Computed,
    Unknown,
}

static CATALOG_JSON: &str = include_str!("../local_models.json");
static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// The parsed catalog (parsed once). Panics only if the *committed* JSON is malformed — which the
/// parse-guard test below prevents from ever landing.
pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON)
            .expect("committed local_models.json must be valid (see parse-guard test)")
    })
}

/// The catalog rung of the endpoint window ladder: a matched model's advertised context window, or
/// `None` (the ladder then falls to its conservative default). Best-effort name matching.
pub fn context_window_for(model_id: &str) -> Option<u32> {
    match_installed(model_id).map(|e| e.context_length)
}

/// Best-effort match of an installed/served model name back to a catalog row. Endpoints report names
/// in many shapes — an Ollama tag (`qwen2.5:7b`), a file path, a bare repo name — so we compare on an
/// alphanumeric-only normalization and accept a containment match either way, preferring the longest.
pub fn match_installed(model_id: &str) -> Option<&'static CatalogEntry> {
    let q = normalize(model_id);
    if q.is_empty() {
        return None;
    }
    catalog()
        .entries
        .iter()
        .filter_map(|e| {
            let key = normalize(&strip_gguf(model_key(e)));
            if key.is_empty() {
                return None;
            }
            if q == key || q.contains(&key) || key.contains(&q) {
                Some((key.len(), e))
            } else {
                None
            }
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, e)| e)
}

/// Bridge a catalog row into a `fit::ModelSpec` for scoring. Quant labels the fit calculator doesn't
/// know are dropped (the catalog's curated quants are all known — pinned by a test).
pub fn entry_to_spec(entry: &CatalogEntry) -> fit::ModelSpec {
    let candidates = entry
        .quants
        .iter()
        .filter_map(|q| {
            fit::Quant::from_label(&q.quant).map(|quant| fit::QuantCandidate {
                quant,
                weight_gb: q.file_gb,
            })
        })
        .collect();
    fit::ModelSpec {
        arch: arch_from(
            &entry.architecture,
            entry.active_parameters_b,
            entry.parameters_b,
        ),
        active_params_b: entry.active_parameters_b,
        target_context: entry.context_length,
        multimodal: entry.multimodal,
        projector_gb: entry.projector_gb,
        candidates,
    }
}

// The catalog identity used for name matching: prefer the repo's last path segment (what tools echo).
fn model_key(entry: &CatalogEntry) -> String {
    entry
        .repo
        .rsplit('/')
        .next()
        .unwrap_or(&entry.repo)
        .to_string()
}

fn strip_gguf(name: String) -> String {
    name.trim_end_matches("-GGUF")
        .trim_end_matches("-gguf")
        .to_string()
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Map a catalog architecture string to the fit calculator's coarse family. MoE is detected from the
/// arch name OR from active < total (some MoE arches don't say "moe", e.g. gemma4 A4B).
fn arch_from(arch: &str, active_b: f64, total_b: f64) -> fit::Architecture {
    let a = arch.to_ascii_lowercase();
    if a.contains("mamba") || a.contains("ssm") || a.contains("rwkv") || a.contains("jamba") {
        fit::Architecture::Ssm
    } else if a.contains("moe") || active_b + 0.01 < total_b {
        fit::Architecture::Moe
    } else {
        fit::Architecture::Dense
    }
}

// --- rescan cadence (#296): when to re-check whether a better-fitting model has appeared ----------

/// Settings key: how often to re-evaluate the catalog against the machine.
pub const RESCAN_CADENCE_KEY: &str = "local_model_rescan_cadence";
/// Settings key: the catalog version we last evaluated (drives on-catalog-update).
pub const CATALOG_VERSION_SEEN_KEY: &str = "local_model_catalog_version_seen";
/// Settings key: the last rescan time (RFC3339), for the weekly/monthly cadences.
pub const LAST_RESCAN_KEY: &str = "local_model_last_rescan";

/// How often to re-check the catalog for a better-fitting model. Passive — never a modal or a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanCadence {
    /// Only when a shipped app update carried a fresher catalog (the default).
    OnCatalogUpdate,
    Weekly,
    Monthly,
    /// Never automatically — the user re-checks by hand.
    Manual,
}

impl RescanCadence {
    /// The default when the setting is absent: re-check on a catalog update (least noisy).
    pub fn from_setting(s: Option<&str>) -> Self {
        match s {
            Some("weekly") => Self::Weekly,
            Some("monthly") => Self::Monthly,
            Some("manual") => Self::Manual,
            _ => Self::OnCatalogUpdate,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::OnCatalogUpdate => "on-catalog-update",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Manual => "manual",
        }
    }
}

/// Pure: is a rescan due? Timestamps are unix seconds. `Manual` never auto-fires; `OnCatalogUpdate`
/// fires when the shipped catalog is newer than the version last evaluated; `Weekly`/`Monthly` fire
/// once enough time has elapsed (a never-evaluated machine is always due).
pub fn rescan_due(
    cadence: RescanCadence,
    seen_version: Option<u32>,
    current_version: u32,
    last_rescan_secs: Option<i64>,
    now_secs: i64,
) -> bool {
    match cadence {
        RescanCadence::Manual => false,
        RescanCadence::OnCatalogUpdate => seen_version.is_none_or(|seen| current_version > seen),
        RescanCadence::Weekly => elapsed_at_least(last_rescan_secs, now_secs, 7),
        RescanCadence::Monthly => elapsed_at_least(last_rescan_secs, now_secs, 30),
    }
}

fn elapsed_at_least(last: Option<i64>, now: i64, days: i64) -> bool {
    match last {
        None => true,
        Some(t) => now - t >= days * 86_400,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_catalog_parses_and_holds_its_invariants() {
        let cat = catalog();
        assert!(cat.schema_version >= 1);
        assert!(
            cat.catalog_version >= 1,
            "catalog needs a monotonic version stamp"
        );
        assert!(
            cat.content_hash.starts_with("sha256:"),
            "catalog needs a content hash stamp"
        );
        assert!(!cat.entries.is_empty(), "catalog must not be empty");

        for e in &cat.entries {
            assert!(e.parameters_b > 0.0, "{}: params", e.repo);
            assert!(
                e.active_parameters_b > 0.0 && e.active_parameters_b <= e.parameters_b + 1e-6,
                "{}: active {} must be 0<active<=total {}",
                e.repo,
                e.active_parameters_b,
                e.parameters_b
            );
            assert!(e.context_length >= 256, "{}: context", e.repo);
            assert!(!e.quants.is_empty(), "{}: needs at least one quant", e.repo);

            // Generator invariant: multimodal iff a projector size is present.
            assert_eq!(
                e.multimodal,
                e.projector_gb.is_some(),
                "{}: multimodal must carry a projector size",
                e.repo
            );

            // No embedding/reranker should ever leak into a chat-model catalog.
            let hay = format!("{} {}", e.repo, e.architecture).to_lowercase();
            assert!(
                !hay.contains("embed") && !hay.contains("rerank"),
                "{}: embedding/reranker leaked into the catalog",
                e.repo
            );

            // Every curated quant label must be known to the fit calculator — this pins the generator's
            // quant set and fit::Quant in lockstep (a new quant in one needs the other).
            for q in &e.quants {
                assert!(
                    fit::Quant::from_label(&q.quant).is_some(),
                    "{}: quant label {} not known to fit::Quant",
                    e.repo,
                    q.quant
                );
                assert!(q.file_gb > 0.0, "{}: quant {} size", e.repo, q.quant);
            }
        }
    }

    #[test]
    fn entry_to_spec_yields_scorable_specs() {
        for e in &catalog().entries {
            let spec = entry_to_spec(e);
            assert!(
                !spec.candidates.is_empty(),
                "{}: no scorable candidates",
                e.repo
            );
            // Dense ⇒ active == total; MoE ⇒ active < total. Either way the spec's active matches.
            assert!((spec.active_params_b - e.active_parameters_b).abs() < 1e-6);
        }
    }

    #[test]
    fn moe_entries_map_to_the_moe_arch() {
        // At least one known MoE entry exists and maps correctly (active < total ⇒ Moe).
        let moe = catalog()
            .entries
            .iter()
            .find(|e| e.active_parameters_b + 0.01 < e.parameters_b);
        if let Some(e) = moe {
            assert_eq!(
                arch_from(&e.architecture, e.active_parameters_b, e.parameters_b),
                fit::Architecture::Moe
            );
        }
    }

    #[test]
    fn installed_names_match_across_shapes() {
        // Pick a real entry and prove several name shapes resolve to it.
        let entry = catalog()
            .entries
            .iter()
            .find(|e| e.repo.contains("Qwen2.5-7B"));
        if let Some(e) = entry {
            for name in [
                "Qwen2.5-7B-Instruct",
                "qwen2.5-7b-instruct",
                "bartowski/Qwen2.5-7B-Instruct-GGUF",
            ] {
                assert_eq!(
                    match_installed(name).map(|m| &m.repo),
                    Some(&e.repo),
                    "failed to match {name}"
                );
                assert_eq!(context_window_for(name), Some(e.context_length));
            }
        }
        // A name matching nothing returns None (the ladder falls through to its default).
        assert!(match_installed("totally-unknown-model-xyz").is_none());
        assert!(context_window_for("").is_none());
    }

    #[test]
    fn rescan_cadence_parses_with_a_sensible_default() {
        assert_eq!(
            RescanCadence::from_setting(None),
            RescanCadence::OnCatalogUpdate
        );
        assert_eq!(
            RescanCadence::from_setting(Some("garbage")),
            RescanCadence::OnCatalogUpdate
        );
        assert_eq!(
            RescanCadence::from_setting(Some("weekly")),
            RescanCadence::Weekly
        );
        assert_eq!(
            RescanCadence::from_setting(Some("manual")),
            RescanCadence::Manual
        );
        // Round-trips through the stored string.
        for c in [
            RescanCadence::OnCatalogUpdate,
            RescanCadence::Weekly,
            RescanCadence::Monthly,
            RescanCadence::Manual,
        ] {
            assert_eq!(RescanCadence::from_setting(Some(c.as_setting())), c);
        }
    }

    #[test]
    fn rescan_due_honours_each_cadence() {
        let day = 86_400;
        // Manual never auto-fires.
        assert!(!rescan_due(RescanCadence::Manual, None, 5, None, 999 * day));
        // On-catalog-update: due when the shipped catalog outranks what we've evaluated.
        assert!(rescan_due(RescanCadence::OnCatalogUpdate, None, 3, None, 0)); // never evaluated
        assert!(rescan_due(
            RescanCadence::OnCatalogUpdate,
            Some(2),
            3,
            None,
            0
        )); // newer catalog
        assert!(!rescan_due(
            RescanCadence::OnCatalogUpdate,
            Some(3),
            3,
            None,
            0
        )); // already current
            // Weekly/Monthly on elapsed time.
        assert!(rescan_due(
            RescanCadence::Weekly,
            Some(3),
            3,
            Some(0),
            8 * day
        ));
        assert!(!rescan_due(
            RescanCadence::Weekly,
            Some(3),
            3,
            Some(0),
            3 * day
        ));
        assert!(rescan_due(
            RescanCadence::Monthly,
            Some(3),
            3,
            Some(0),
            31 * day
        ));
        assert!(!rescan_due(
            RescanCadence::Monthly,
            Some(3),
            3,
            Some(0),
            20 * day
        ));
    }
}
