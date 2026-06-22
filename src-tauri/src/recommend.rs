// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Model recommender (spec §6) — the deterministic selection behind PM's two model
//! suggestions, kept pure (no DB, no network, no `.await`) so it unit-tests in isolation
//! like [`crate::cost::call_cost`] / [`crate::projects::derive_status`]. `commands.rs`
//! reads the cached catalogue + the curated tier list + the user denylist and feeds plain
//! values in here; it proposes, the user chooses, and the existing per-role model store
//! stays the source of truth.
//!
//! Two recommendations:
//!   * **Day-to-day** — the cheapest model (by cache-weighted *effective* cost) that still
//!     clears a reliability floor (advertises tool-calling / structured output) and a
//!     minimum context window. The cheapest that clears the floors, not the cheapest model
//!     outright — good for high-volume, low-risk sorting + everyday chat.
//!   * **Advanced** — the highest-*capability* model among long-context candidates under a
//!     cost ceiling, for high-stakes, citation-critical chat. Capability is ranked
//!     PRIMARILY by the live Artificial-Analysis `intelligence_index` from the catalogue;
//!     the curated list only adds a faithfulness bonus + a fallback rank for models the
//!     benchmark doesn't cover.
//!
//! WHY A CURATED LIST AT ALL: the OpenRouter API exposes no faithfulness / hallucination
//! metric (verified against /models and /models/:id/endpoints — neither carries one), and
//! `intelligence_index` is general capability, not grounded-RAG faithfulness. So a small
//! human-maintained list (`recommend_tiers.json`) nudges toward models known to be faithful
//! for RAG. It is intentionally curated, not computed — edit it as models change.
//!
//! PRIVACY: data-policy is NOT decided here. PM enforces Zero-Data-Retention on every
//! request at the transport layer ([`crate::openrouter`]'s request body), which is the only
//! real boundary the API offers — there is no per-model data-policy field to filter on.
//! This module only applies the optional user denylist as defense-in-depth when filtering
//! candidates.

use serde::{Deserialize, Serialize};

use crate::openrouter::ModelDetail;

/// One entry of the curated faithfulness list (`recommend_tiers.json`). Only the `id` is
/// read; the human-facing `note` on each JSON entry (and the top-level `_why_curated`
/// documentation keys) are ignored by serde — they document the file for whoever edits it.
#[derive(Debug, Clone, Deserialize)]
pub struct CuratedEntry {
    pub id: String,
}

/// The curated tier list, parsed from `recommend_tiers.json`. `advanced_faithfulness` is
/// ordered best-first.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CuratedTiers {
    #[serde(default)]
    pub advanced_faithfulness: Vec<CuratedEntry>,
}

/// One recommendation surfaced to the Settings cards. Serialised to the frontend as-is.
#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    pub model: String,
    pub name: String,
    /// One-line, plain-language reason this model was chosen.
    pub why: String,
    /// Cache-weighted effective price in USD per *million* tokens (display + sort key);
    /// `None` when the model isn't priced.
    pub effective_cost_per_mtok: Option<f64>,
    pub context_length: Option<u64>,
    /// The live capability index when the catalogue had one (Advanced only; `None` for
    /// Day-to-day or an unbenchmarked model).
    pub intelligence_index: Option<f64>,
    /// True when this model also appears in the curated faithfulness list.
    pub curated: bool,
}

// --- tunable selection constants (documented workload assumptions, not billing) ---

/// Day-to-day must clear at least this context window (tokens) — room for PM's RAG prompt
/// (system + Learning-You profile + retrieved chunks + a little history).
const DAYTODAY_MIN_CONTEXT: u64 = 16_000;
/// Advanced must be a genuinely long-context model (grounded RAG over large documents).
const ADVANCED_MIN_CONTEXT: u64 = 200_000;
/// Advanced cost guardrail: a candidate must be priced AND at or under this effective price
/// ($/Mtok). Generous on purpose — the capability index does the ranking; this only blocks
/// extreme outliers. An unpriced model is excluded (we can't guarantee it's within budget),
/// mirroring Day-to-day, so a high-capability model whose price the catalogue omits can't
/// silently defeat the ceiling.
const ADVANCED_MAX_COST_PER_MTOK: f64 = 60.0;
/// Fraction of prompt tokens assumed served from cache. PM reuses a stable prompt prefix
/// (system + profile + retrieved context), so cache reads dominate prompt cost; the
/// cache-read rate is weighted this heavily when one is published.
const CACHE_HIT_FRACTION: f64 = 0.5;
/// Effective cost blends prompt-side and completion-side $/token. PM's chat is prompt-heavy
/// (lots of grounding, comparatively short answers), so prompt is weighted above completion.
const PROMPT_WEIGHT: f64 = 0.7;
const COMPLETION_WEIGHT: f64 = 0.3;
/// Small capability bonus for being on the curated faithfulness list — enough to break a
/// near-tie toward a known-faithful model without overriding a clearly stronger index.
const CURATED_BONUS: f64 = 5.0;

/// The two recommendations from a cached catalogue + curated list + denylist. Either may be
/// `None` when nothing qualifies (e.g. an empty/stale cache) — the caller surfaces that as
/// "unavailable, showing last-known", never a silent non-compliant fallback.
pub fn recommend(
    catalogue: &[ModelDetail],
    curated: &CuratedTiers,
    denylist: &[String],
) -> (Option<Recommendation>, Option<Recommendation>) {
    let refs: Vec<&ModelDetail> = catalogue.iter().collect();
    (
        pick_day_to_day(&refs, denylist),
        pick_advanced(&refs, curated, denylist),
    )
}

/// Cache-weighted effective price in USD per token, or `None` if the model isn't priced.
/// The prompt side blends the cache-read rate (weighted by [`CACHE_HIT_FRACTION`]) with the
/// full prompt rate; the result blends prompt + completion by their workload weights.
fn effective_cost_per_token(m: &ModelDetail) -> Option<f64> {
    let prompt = m.prompt_price?;
    let completion = m.completion_price?;
    let cache_read = m.cache_read_price.unwrap_or(prompt);
    let eff_prompt = cache_read * CACHE_HIT_FRACTION + prompt * (1.0 - CACHE_HIT_FRACTION);
    Some(eff_prompt * PROMPT_WEIGHT + completion * COMPLETION_WEIGHT)
}

fn per_mtok(per_token: f64) -> f64 {
    per_token * 1_000_000.0
}

/// True when the model can be relied on for PM's structured / tool-using calls: it must
/// advertise tool-calling or structured-output support in `supported_parameters`.
fn passes_reliability(m: &ModelDetail) -> bool {
    m.supported_parameters.iter().any(|s| {
        s == "tools" || s == "response_format" || s == "structured_outputs"
    })
}

/// True when `id` is covered by a denylist entry. A bare provider ("openai") denies the
/// whole `openai/...` namespace; a slug or slug-prefix ("openai/gpt") matches as a prefix.
/// Case-insensitive.
fn is_denylisted(id: &str, denylist: &[String]) -> bool {
    let id = id.to_lowercase();
    denylist
        .iter()
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty())
        .any(|d| {
            if d.contains('/') {
                id == d || id.starts_with(&d)
            } else {
                id == d || id.starts_with(&format!("{d}/"))
            }
        })
}

/// Day-to-day: cheapest effective cost among reliable, non-denylisted, priced models that
/// clear the context floor.
fn pick_day_to_day(candidates: &[&ModelDetail], denylist: &[String]) -> Option<Recommendation> {
    let (m, cost) = candidates
        .iter()
        .filter(|m| !is_denylisted(&m.id, denylist))
        .filter(|m| m.context_length.unwrap_or(0) >= DAYTODAY_MIN_CONTEXT)
        .filter(|m| passes_reliability(m))
        .filter_map(|m| effective_cost_per_token(m).map(|c| (*m, c)))
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))?;
    let why = format!(
        "Cheapest model that still supports tool-calling and a {}K+ context — ideal for \
         high-volume sorting and everyday chat.",
        DAYTODAY_MIN_CONTEXT / 1000
    );
    Some(Recommendation {
        model: m.id.clone(),
        name: display_name(m),
        why,
        effective_cost_per_mtok: Some(per_mtok(cost)),
        context_length: m.context_length,
        intelligence_index: m.intelligence_index,
        curated: false,
    })
}

/// Advanced: highest capability among long-context, reliable, non-denylisted models under
/// the cost ceiling. Capability is the live intelligence index when present; the curated
/// list supplies a faithfulness bonus + a fallback rank for unbenchmarked models, and
/// breaks ties toward known-faithful models.
fn pick_advanced(
    candidates: &[&ModelDetail],
    curated: &CuratedTiers,
    denylist: &[String],
) -> Option<Recommendation> {
    let curated_rank = |id: &str| {
        curated
            .advanced_faithfulness
            .iter()
            .position(|e| e.id.eq_ignore_ascii_case(id))
    };

    let m = *candidates
        .iter()
        .filter(|m| !is_denylisted(&m.id, denylist))
        .filter(|m| m.context_length.unwrap_or(0) >= ADVANCED_MIN_CONTEXT)
        .filter(|m| passes_reliability(m))
        .filter(|m| {
            // Must be priced AND within the ceiling — an unpriced model can't be guaranteed
            // within budget, so it's excluded rather than silently bypassing the guardrail.
            effective_cost_per_token(m)
                .map(per_mtok)
                .is_some_and(|c| c <= ADVANCED_MAX_COST_PER_MTOK)
        })
        .max_by(|a, b| {
            let sa = capability_score(a, curated_rank(&a.id));
            let sb = capability_score(b, curated_rank(&b.id));
            sa.partial_cmp(&sb)
                .unwrap_or(std::cmp::Ordering::Equal)
                // tie-break: the cheaper effective cost wins (unpriced sorts last).
                .then_with(|| {
                    let ca = effective_cost_per_token(a).map(per_mtok).unwrap_or(f64::INFINITY);
                    let cb = effective_cost_per_token(b).map(per_mtok).unwrap_or(f64::INFINITY);
                    cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
                })
        })?;

    let rank = curated_rank(&m.id);
    let cost = effective_cost_per_token(m).map(per_mtok);
    let why = match (m.intelligence_index, rank.is_some()) {
        (Some(i), true) => format!(
            "Top-tier capability (intelligence index {i:.0}) and on PM's curated \
             faithfulness list — best for high-stakes, citation-critical chat."
        ),
        (Some(i), false) => format!(
            "Highest capability (intelligence index {i:.0}) among long-context models \
             within budget — best for high-stakes, citation-critical chat."
        ),
        (None, true) => "On PM's curated faithfulness list, with a long context window — \
             best for high-stakes, citation-critical chat."
            .to_string(),
        (None, false) => {
            "Long-context model chosen for high-stakes, citation-critical chat.".to_string()
        }
    };
    Some(Recommendation {
        model: m.id.clone(),
        name: display_name(m),
        why,
        effective_cost_per_mtok: cost,
        context_length: m.context_length,
        intelligence_index: m.intelligence_index,
        curated: rank.is_some(),
    })
}

/// Capability score for the Advanced ranking. Primary signal is the live intelligence
/// index (~0–100). A model the benchmark doesn't cover falls back to a score synthesised
/// from its curated rank (so a hand-picked model still ranks above an unknown one); an
/// unbenchmarked, uncurated model floors at 0. A small bonus rewards curated faithfulness.
fn capability_score(m: &ModelDetail, curated_rank: Option<usize>) -> f64 {
    let base = match m.intelligence_index {
        Some(i) => i,
        None => match curated_rank {
            Some(r) => (55.0 - (r as f64) * 3.0).max(0.0),
            None => 0.0,
        },
    };
    base + if curated_rank.is_some() { CURATED_BONUS } else { 0.0 }
}

fn display_name(m: &ModelDetail) -> String {
    if m.name.trim().is_empty() {
        m.id.clone()
    } else {
        m.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalogue row with sensible defaults; tests override only what they exercise.
    fn model(id: &str) -> ModelDetail {
        ModelDetail {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            context_length: Some(256_000),
            prompt_price: Some(3e-6),
            completion_price: Some(15e-6),
            cache_read_price: None,
            input_modalities: vec!["text".into()],
            supported_parameters: vec!["tools".into(), "response_format".into()],
            intelligence_index: None,
        }
    }

    fn tiers(ids: &[&str]) -> CuratedTiers {
        CuratedTiers {
            advanced_faithfulness: ids
                .iter()
                .map(|id| CuratedEntry { id: id.to_string() })
                .collect(),
        }
    }

    #[test]
    fn reliability_requires_tools_or_structured_output() {
        let mut m = model("a/b");
        assert!(passes_reliability(&m));
        m.supported_parameters = vec!["structured_outputs".into()];
        assert!(passes_reliability(&m));
        m.supported_parameters = vec!["temperature".into(), "top_p".into()];
        assert!(!passes_reliability(&m));
    }

    #[test]
    fn effective_cost_weights_cache_reads_and_is_none_when_unpriced() {
        let mut m = model("a/b");
        m.prompt_price = Some(10e-6);
        m.completion_price = Some(20e-6);
        m.cache_read_price = Some(1e-6); // a deep cache discount should pull the cost down
        let with_cache = effective_cost_per_token(&m).unwrap();
        m.cache_read_price = None; // falls back to the full prompt rate
        let without_cache = effective_cost_per_token(&m).unwrap();
        assert!(with_cache < without_cache, "cache read rate must lower effective cost");
        m.prompt_price = None;
        assert!(effective_cost_per_token(&m).is_none());
    }

    #[test]
    fn denylist_matches_provider_namespace_and_slug_prefix() {
        assert!(is_denylisted("openai/gpt-5.5", &["openai".into()]));
        assert!(is_denylisted("openai/gpt-5.5", &["openai/gpt".into()]));
        assert!(!is_denylisted("openai-mirror/x", &["openai".into()])); // boundary respected
        assert!(!is_denylisted("anthropic/claude", &["openai".into()]));
        assert!(!is_denylisted("anthropic/claude", &[" ".into(), String::new()]));
    }

    #[test]
    fn day_to_day_picks_cheapest_that_clears_the_floors() {
        let mut cheap_unreliable = model("x/cheap-unreliable");
        cheap_unreliable.prompt_price = Some(1e-7);
        cheap_unreliable.completion_price = Some(1e-7);
        cheap_unreliable.supported_parameters = vec!["temperature".into()]; // fails reliability

        let mut cheap_short = model("x/cheap-short");
        cheap_short.prompt_price = Some(1e-7);
        cheap_short.completion_price = Some(1e-7);
        cheap_short.context_length = Some(8_000); // below the floor

        let mut budget_ok = model("x/budget-ok");
        budget_ok.prompt_price = Some(5e-7);
        budget_ok.completion_price = Some(1e-6);

        let pricey = model("x/pricey"); // 3e-6 / 15e-6, also valid but dearer

        let cat = vec![cheap_unreliable, cheap_short, budget_ok, pricey];
        let (day, _) = recommend(&cat, &CuratedTiers::default(), &[]);
        assert_eq!(day.unwrap().model, "x/budget-ok");
    }

    #[test]
    fn advanced_ranks_by_intelligence_index_then_respects_floors() {
        let mut strong = model("x/strong");
        strong.intelligence_index = Some(60.0);

        let mut stronger_but_short = model("x/short");
        stronger_but_short.intelligence_index = Some(80.0);
        stronger_but_short.context_length = Some(64_000); // excluded: not long-context

        let mut weaker = model("x/weaker");
        weaker.intelligence_index = Some(40.0);

        let cat = vec![strong, stronger_but_short, weaker];
        let (_, adv) = recommend(&cat, &CuratedTiers::default(), &[]);
        let adv = adv.unwrap();
        assert_eq!(adv.model, "x/strong");
        assert_eq!(adv.intelligence_index, Some(60.0));
        assert!(!adv.curated);
    }

    #[test]
    fn curated_bonus_breaks_a_near_tie_toward_a_faithful_model() {
        let mut indexed = model("x/indexed");
        indexed.intelligence_index = Some(50.0);

        let mut curated = model("x/curated");
        curated.intelligence_index = Some(48.0); // slightly lower index...

        let cat = vec![indexed, curated];
        // ...but the curated bonus (+5) lifts it over the 2-point index gap.
        let (_, adv) = recommend(&cat, &tiers(&["x/curated"]), &[]);
        let adv = adv.unwrap();
        assert_eq!(adv.model, "x/curated");
        assert!(adv.curated);
    }

    #[test]
    fn advanced_excludes_unpriced_models_from_the_ceiling() {
        let mut unpriced_genius = model("x/unpriced");
        unpriced_genius.intelligence_index = Some(99.0);
        unpriced_genius.prompt_price = None;
        unpriced_genius.completion_price = None;
        let mut priced = model("x/priced");
        priced.intelligence_index = Some(50.0);
        let cat = vec![unpriced_genius, priced];
        // The unpriced "genius" can't be guaranteed within budget, so the priced model wins.
        let (_, adv) = recommend(&cat, &CuratedTiers::default(), &[]);
        assert_eq!(adv.unwrap().model, "x/priced");
    }

    #[test]
    fn advanced_falls_back_to_curated_rank_when_unbenchmarked() {
        // No model has an intelligence index; the curated order decides.
        let cat = vec![model("x/second"), model("x/first"), model("x/unranked")];
        let (_, adv) = recommend(&cat, &tiers(&["x/first", "x/second"]), &[]);
        assert_eq!(adv.unwrap().model, "x/first");
    }

    #[test]
    fn denylisted_models_are_never_recommended() {
        let mut cheap = model("blocked/cheap");
        cheap.prompt_price = Some(1e-8);
        cheap.completion_price = Some(1e-8);
        cheap.intelligence_index = Some(99.0);
        let ok = model("ok/model");
        let cat = vec![cheap, ok];
        let (day, adv) = recommend(&cat, &CuratedTiers::default(), &["blocked".into()]);
        assert_eq!(day.unwrap().model, "ok/model");
        assert_eq!(adv.unwrap().model, "ok/model");
    }

    #[test]
    fn empty_catalogue_yields_no_recommendation() {
        let (day, adv) = recommend(&[], &CuratedTiers::default(), &[]);
        assert!(day.is_none() && adv.is_none());
    }
}
