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
    /// What a token costs this model's KV cache, read from its GGUF header's attention geometry.
    /// `None` only when the generator could not read that header, and the fit then falls back to
    /// its parameter-count proxy — the committed catalogue carries it on every entry (pinned below).
    pub kv_cache: Option<CatalogKvCache>,
    pub fit: FitClass,
    pub quants: Vec<CatalogQuant>,
    /// What this model's weights are licensed under. Required, not optional: an entry with no
    /// licence must never reach a user, and the generator refuses to write one
    /// (`scripts/generate-local-catalog.mjs`). The decision behind each value lives in
    /// `src-tauri/model_licences.json`; this is the resolved copy the app reads.
    pub licence: EntryLicence,
}

/// The licence a catalogue entry's weights come under, resolved from the ledger's `terms` table so
/// the app needs no second lookup.
///
/// `open` is the only field with behaviour attached: `false` means bespoke publisher terms rather
/// than an open-source licence — Gemma 2/3, Llama 3.x, the largest Qwen 2.5 — and the UI shows
/// `summary` and asks before a download. It is disclosure, not enforcement: PM never fetches weights
/// itself, the user's own Ollama does, and they can pull the same model without PM entirely.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntryLicence {
    /// Hugging Face's own id where there is one (`apache-2.0`, `mit`, `gemma`, `llama3.2`, `qwen`).
    pub id: String,
    pub name: String,
    pub url: String,
    /// True for a recognised open-source licence; false for bespoke publisher terms.
    pub open: bool,
    /// A plain-language paragraph, written for a person to read in the download dialog.
    pub summary: String,
}

/// A catalogue entry's KV-cache geometry, in bytes at f16 (`kvFromHeader` in the generator says
/// where each figure comes from). Bridged into [`fit::KvGeometry`] by [`entry_to_spec`].
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogKvCache {
    /// What one token adds across the layers whose cache spans the whole context.
    pub bytes_per_token: f64,
    /// What one token adds across the sliding-window layers, which hold only `window` tokens.
    pub window_bytes_per_token: f64,
    /// The sliding window in tokens, or `None` when the model has no sliding layers.
    pub window: Option<u32>,
    /// A hybrid model's fixed recurrent state, f32 (0 when it has none).
    pub state_bytes: f64,
}

/// One downloadable quantization with its measured on-disk size.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogQuant {
    pub quant: String,
    pub file_gb: f64,
    pub sharded: bool,
    /// The Ollama pull target for THIS quant (`hf.co/<repo>:<QUANT>`), or `None` when the generator
    /// checked and found none offerable — a sharded GGUF, or a manifest whose model layer did not
    /// match the byte count this row measured. `None` means "checked, not offerable", never "nobody
    /// looked": the generator writes the field on every row it emits.
    ///
    /// Per-QUANT rather than per-entry on purpose. The card's fit verdict is about one specific
    /// quantization, so a single per-entry tag would download a different file from the one the card
    /// sized, and the memory figure it showed would be a lie.
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

/// Match a model the endpoint SERVES back to a catalog row, using its parameter count when the name
/// alone is not enough.
///
/// [`match_installed`] first, unchanged. What it cannot match is Ollama's own library naming: a tag
/// like `qwen2.5:latest` carries a family and no size, so it contains no catalogue key and none
/// contains it — `qwen2.5:latest`, `llama3.2:latest`, `gemma3:latest`, `phi3.5:latest` and
/// `llama3.1:latest` all matched nothing. Ollama does report the parameter count in `/api/tags`
/// (`details.parameter_size`), so this falls back to family + size:
///
/// * the id's family is the part after its last `/` and before its first `:` (`qwen2.5`);
/// * an entry's family is its repo name minus `-GGUF`, up to its first size token, with the
///   separators dropped (`Meta-Llama-3.1-8B-Instruct` → `metallama3.1`);
/// * an entry is a candidate when its family contains the id's at a boundary — so `qwen3` never
///   matches `qwen3.5` — and its size is within 15% of the reported one;
/// * the closest wins, and an exact tie matches nothing rather than one of the two at random.
///
/// `None` without a size: a family alone names several models, and PM would rather say "not in the
/// catalog" than size the wrong one.
pub fn match_served(id: &str, parameters_b: Option<f64>) -> Option<&'static CatalogEntry> {
    if let Some(entry) = match_installed(id) {
        return Some(entry);
    }
    let p = parameters_b.filter(|p| p.is_finite() && *p > 0.0)?;
    let lower = id.to_ascii_lowercase();
    let after_slash = lower.rsplit('/').next().unwrap_or(&lower);
    let base = after_slash.split(':').next().unwrap_or(after_slash);
    if base.is_empty() {
        return None;
    }

    let mut scored: Vec<(f64, &'static CatalogEntry)> = catalog()
        .entries
        .iter()
        .filter(|e| family_contains(&family(e), base))
        .map(|e| ((e.parameters_b - p).abs(), e))
        .filter(|(distance, _)| distance / p <= 0.15)
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    match scored.as_slice() {
        [] => None,
        [(d0, _), (d1, _), ..] if d0 == d1 => None,
        [(_, best), ..] => Some(*best),
    }
}

/// An entry's family for [`match_served`]: the repo name, lowercased, minus `-gguf`, split on `-` and
/// `_`, kept up to the first size token (`7b`, `500m`, or an MoE's `a3b`), and joined with nothing.
fn family(entry: &CatalogEntry) -> String {
    let name = model_key(entry).to_ascii_lowercase();
    let name = name.strip_suffix("-gguf").unwrap_or(&name);
    name.split(['-', '_'])
        .take_while(|token| !is_size_token(token))
        .collect()
}

/// `^\d+(\.\d+)?[bm]$` or `^a\d+b$`: a parameter count, or an MoE's active count.
fn is_size_token(token: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(n) = token.strip_prefix('a').and_then(|t| t.strip_suffix('b')) {
        if digits(n) {
            return true;
        }
    }
    let Some(n) = token.strip_suffix(['b', 'm']) else {
        return false;
    };
    match n.split_once('.') {
        Some((whole, frac)) => digits(whole) && digits(frac),
        None => digits(n),
    }
}

/// Whether `family` contains `base` where the next character does not continue a version number:
/// `qwen2.5` is in `qwen2.5`, and `phi3.5` in `phi3.5miniinstruct`, but `qwen3` is not in `qwen3.5`.
fn family_contains(family: &str, base: &str) -> bool {
    family.match_indices(base).any(|(at, _)| {
        family[at + base.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_digit() || c == '.'))
    })
}

/// Whether a model id names an embedding or reranking model rather than a chat model.
///
/// Embedders and rerankers cannot answer a chat turn, but every discovery path PM has hands them
/// over as ordinary ids: Ollama and LM Studio serve them on the same `/v1/models` endpoint, and the
/// on-disk crawl finds their `.gguf` files like any other. This is the one predicate that says so.
///
/// **Deliberately under-blocking.** The token list is narrower than the catalog generator's regex:
/// it drops `bge-`, which is safe against curated Hugging Face repo ids but not against a free-form
/// Ollama tag or an LM Studio publisher folder, and it never adds short tokens like `e5`, `gte` or
/// `minilm` that collide with ordinary words. The asymmetry is on purpose — a false positive makes a
/// legitimate chat model unselectable, while a false negative merely lets a chat attempt fail
/// loudly. Callers must show-and-explain rather than silently drop, so a false positive is visible
/// and self-correcting rather than a model that vanished.
pub fn is_embedding_or_reranker(id: &str) -> bool {
    const TOKENS: [&str; 5] = [
        "embed",
        "rerank",
        "sentence-transformers",
        "cross-encoder",
        "text-embedding",
    ];
    let id = id.to_ascii_lowercase();
    TOKENS.iter().any(|t| id.contains(t))
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
        projector_gb: entry.projector_gb,
        candidates,
        // Every spec PM builds for a model starts here — the card, a served copy, a file on disk —
        // so all of them are sized from the entry's real attention geometry, not the proxy.
        kv_geometry: entry.kv_cache.map(|kv| fit::KvGeometry {
            bytes_per_token: kv.bytes_per_token,
            window_bytes_per_token: kv.window_bytes_per_token,
            window: kv.window.unwrap_or(0),
            state_bytes: kv.state_bytes,
        }),
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
/// Settings key: which non-open licences the user has read and accepted, comma-separated licence
/// ids. Keyed on the LICENCE, not the model — accepting the Gemma Terms once covers every Gemma
/// model, which is what a person would expect after reading them.
pub const TERMS_ACCEPTED_KEY: &str = "local_model_terms_accepted";

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
        // Pinned, not `>=`: the version is decoration unless something compares it. Bumping it in
        // the generator without landing the matching Rust change fails here rather than at runtime.
        assert_eq!(
            cat.schema_version, 4,
            "catalog schema version must match what this module parses"
        );
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

            // Every entry is sized from its own attention geometry. An entry without one falls back
            // to the parameter-count proxy, which under-counted Phi 3.5 mini 13x and is exactly what
            // made PM recommend configs that did not fit — so a regenerated catalogue that lost the
            // figure for any entry fails here rather than quietly going back to it.
            let kv = e
                .kv_cache
                .unwrap_or_else(|| panic!("{}: no KV geometry from its GGUF header", e.repo));
            assert!(
                kv.bytes_per_token > 0.0
                    && kv.window_bytes_per_token >= 0.0
                    && kv.state_bytes >= 0.0,
                "{}: KV geometry {kv:?}",
                e.repo
            );
            assert_eq!(
                kv.window.is_some(),
                kv.window_bytes_per_token > 0.0,
                "{}: a sliding window comes with sliding layers, and only then",
                e.repo
            );

            // Generator invariant: multimodal iff a projector size is present.
            assert_eq!(
                e.multimodal,
                e.projector_gb.is_some(),
                "{}: multimodal must carry a projector size",
                e.repo
            );

            // No embedding/reranker should ever leak into a chat-model catalog. Asserted through the
            // same predicate the runtime gate uses, so the catalog invariant and the served-model
            // gate cannot drift apart — and so this pins the generator's filter rather than a
            // weaker two-token restatement of it.
            let hay = format!("{} {}", e.repo, e.architecture);
            assert!(
                !is_embedding_or_reranker(&hay),
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

                // The Ollama pull target, if the generator wrote one. Derived from THIS row rather
                // than pattern-matched: a `starts_with("hf.co/")` check would pass for free on a
                // stale or hand-edited tag pointing at another model entirely.
                if let Some(tag) = &q.ollama {
                    assert_eq!(
                        tag,
                        &format!("hf.co/{}:{}", e.repo, q.quant),
                        "{}: quant {} carries a tag that is not its own",
                        e.repo,
                        q.quant
                    );
                    assert!(
                        !q.sharded,
                        "{}: quant {} is sharded and must carry no tag — Ollama's registry refuses \
                         split GGUF, so the Download button would fail",
                        e.repo,
                        q.quant
                    );
                }
            }

            // A floor, not a total: some rows are legitimately un-offerable. The shipped bug was
            // that EVERY row was null for three releases and nothing noticed.
            assert!(
                e.quants.iter().any(|q| q.ollama.is_some()),
                "{}: no quant carries an Ollama pull tag — the Download button is dead for it",
                e.repo
            );

            // Every entry names a licence, and a restricted one carries the text the UI promises to
            // show before a download. An empty summary here would mean an empty dialog there.
            assert!(!e.licence.id.is_empty(), "{}: licence id", e.repo);
            assert!(!e.licence.name.is_empty(), "{}: licence name", e.repo);
            assert!(
                e.licence.url.starts_with("https://"),
                "{}: licence url must be https ({})",
                e.repo,
                e.licence.url
            );
            assert!(
                !e.licence.summary.trim().is_empty(),
                "{}: licence summary must not be empty",
                e.repo
            );
        }

        // The catalogue genuinely holds both kinds. If this ever reads zero restricted entries, the
        // terms flow below has quietly stopped being exercised by anything.
        assert!(
            cat.entries.iter().any(|e| !e.licence.open),
            "catalog should still contain at least one restricted-terms model"
        );
        assert!(
            cat.entries.iter().any(|e| e.licence.open),
            "catalog should still contain at least one open-licence model"
        );
    }

    #[test]
    fn the_catalogue_carries_the_attention_geometry_the_fit_was_calibrated_against() {
        // The two entries measured on a real card (fit.rs
        // `the_estimate_brackets_the_loads_measured_on_a_real_card`), the one the proxy got most
        // wrong, and a hybrid. Read off each GGUF header on 02-10-2026.
        let geometry = |repo: &str| {
            let e = catalog().entries.iter().find(|e| e.repo == repo).unwrap();
            entry_to_spec(e).kv_geometry.unwrap()
        };
        let g = |full: f64, windowed: f64, window: u32, state: f64| fit::KvGeometry {
            bytes_per_token: full,
            window_bytes_per_token: windowed,
            window,
            state_bytes: state,
        };
        // 28 layers × 4 KV heads × 128 × K,V × 2 bytes.
        assert_eq!(
            geometry("bartowski/Qwen2.5-7B-Instruct-GGUF"),
            g(57_344.0, 0.0, 0, 0.0)
        );
        // Five global layers and 29 sliding ones, window 1024.
        assert_eq!(
            geometry("ggml-org/gemma-3-4b-it-GGUF"),
            g(20_480.0, 118_784.0, 1024, 0.0)
        );
        // No grouped-query attention: 32 layers × 32 KV heads × 96 × 2 × 2.
        assert_eq!(
            geometry("bartowski/Phi-3.5-mini-instruct-GGUF"),
            g(393_216.0, 0.0, 0, 0.0)
        );
        // A hybrid: a cache on 8 of 32 layers, a fixed state on the other 24.
        assert_eq!(
            geometry("unsloth/Qwen3.5-4B-GGUF"),
            g(32_768.0, 0.0, 0, 52_690_944.0)
        );
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
            }
        }
        // A name matching nothing returns None.
        assert!(match_installed("totally-unknown-model-xyz").is_none());
        assert!(match_installed("").is_none());
    }

    #[test]
    fn ollamas_bare_library_tags_match_by_family_and_size() {
        // Ollama's own library names carry a family and no size, so the name match found nothing
        // for any of these. The sizes are the `details.parameter_size` Ollama reports for each.
        let repo = |id: &str, size: f64| match_served(id, Some(size)).map(|e| e.repo.as_str());
        for (id, size, want) in [
            ("qwen2.5:latest", 7.6, "bartowski/Qwen2.5-7B-Instruct-GGUF"),
            (
                "llama3.2:latest",
                3.2,
                "bartowski/Llama-3.2-3B-Instruct-GGUF",
            ),
            ("gemma3:latest", 4.3, "ggml-org/gemma-3-4b-it-GGUF"),
            ("phi3.5:latest", 3.8, "bartowski/Phi-3.5-mini-instruct-GGUF"),
            (
                "llama3.1:latest",
                8.0,
                "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
            ),
        ] {
            assert!(
                match_installed(id).is_none(),
                "{id}: the name alone matches nothing"
            );
            assert_eq!(repo(id, size), Some(want), "{id}");
        }

        // `qwen3` is its own family, not a prefix of `qwen3.5` / `qwen3.6`.
        assert_eq!(repo("qwen3:latest", 8.2), None);
        // The family is right but no curated size is within 15% of it.
        assert_eq!(repo("gemma2:latest", 9.2), None);
        // Without a size a family names several models, so PM matches none of them.
        assert!(match_served("qwen2.5:latest", None).is_none());
        assert!(match_served("", Some(7.6)).is_none());

        // A name the catalogue already matches is untouched by the size.
        assert_eq!(
            match_served(
                "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M",
                Some(70.0)
            )
            .map(|e| e.repo.as_str()),
            Some("bartowski/Qwen2.5-7B-Instruct-GGUF")
        );
    }

    #[test]
    fn a_catalogue_family_stops_at_its_first_size_token() {
        let fam = |repo: &str| {
            let e = catalog()
                .entries
                .iter()
                .find(|e| e.repo == repo)
                .unwrap_or_else(|| panic!("{repo} is in the catalogue"));
            family(e)
        };
        assert_eq!(fam("bartowski/Qwen2.5-7B-Instruct-GGUF"), "qwen2.5");
        assert_eq!(fam("bartowski/Llama-3.2-1B-Instruct-GGUF"), "llama3.2");
        assert_eq!(fam("ggml-org/gemma-3-4b-it-GGUF"), "gemma3");
        assert_eq!(
            fam("bartowski/Phi-3.5-mini-instruct-GGUF"),
            "phi3.5miniinstruct"
        );
        assert_eq!(
            fam("bartowski/Meta-Llama-3.1-8B-Instruct-GGUF"),
            "metallama3.1"
        );
        assert_eq!(fam("unsloth/Qwen3.5-4B-GGUF"), "qwen3.5");
        assert_eq!(fam("unsloth/Qwen3.6-35B-A3B-GGUF"), "qwen3.6");
        assert_eq!(fam("ggml-org/SmolVLM-500M-Instruct-GGUF"), "smolvlm");

        assert!(is_size_token("7b") && is_size_token("0.5b") && is_size_token("500m"));
        assert!(is_size_token("a3b"));
        assert!(!is_size_token("3") && !is_size_token("b") && !is_size_token("it"));
        assert!(!is_size_token("3.b") && !is_size_token(".5b"));
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

    #[test]
    fn embedders_and_rerankers_are_told_apart_from_chat_models() {
        // Real ids from the runners PM supports — Ollama tags, an LM Studio prefix, HF repo paths.
        for id in [
            "nomic-embed-text:latest",
            "mxbai-embed-large:latest",
            "embeddinggemma:latest",
            "qwen3-embedding:0.6b",
            "text-embedding-nomic-embed-text-v1.5",
            "xitao/bge-reranker-v2-m3:latest",
            "BAAI/bge-reranker-v2-m3/model.gguf",
            "sentence-transformers/all-MiniLM-L6-v2",
            "cross-encoder/ms-marco-MiniLM-L-6-v2",
        ] {
            assert!(is_embedding_or_reranker(id), "{id} should be gated");
        }

        // Every shipped catalog model, plus the shapes the other runners produce, must pass through.
        for id in [
            "llama3.2:1b",
            "qwen2.5:7b",
            "gemma-3-4b-it-Q4_K_M.gguf",
            "unsloth/Qwen3.6-35B-A3B-GGUF/Qwen3.6-35B-A3B-Q4_K_M.gguf",
            "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
            "unsloth/gemma-4-26B-A4B-it-GGUF",
            "Phi-3.5-mini-instruct",
        ] {
            assert!(!is_embedding_or_reranker(id), "{id} is a chat model");
        }

        // Deliberately NOT gated: `bge-` and bare `e5`/`gte`/`minilm` are safe against curated HF
        // repo ids but collide with free-form tags, and a false positive makes a real chat model
        // unselectable. Under-blocking is the chosen direction — pinned so it stays a decision.
        assert!(!is_embedding_or_reranker("bge-m3:latest"));
        assert!(!is_embedding_or_reranker("all-minilm:l6-v2"));

        for entry in catalog().entries.iter() {
            assert!(
                !is_embedding_or_reranker(&entry.repo),
                "{}: a shipped catalog model must never be gated",
                entry.repo
            );
        }
    }
}
