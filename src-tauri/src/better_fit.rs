// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "A model that fits your machine better is available" (#437) — the pure decision.
//!
//! The Workbench already scores every curated model against this machine. This decides whether any
//! of them is worth *interrupting* the user about, given what they already run. It is deliberately
//! conservative: a nag that fires on a marginal difference trains people to ignore it.
//!
//! The rules, in order:
//!
//! 1. **There must be something to improve on.** With no local model assigned to any role there is
//!    no "better than what you run", so this stays silent — pitching local AI at someone who hasn't
//!    set it up is a different feature, and not this one.
//! 2. **The baseline is the *best* model you already run**, not the first one found. Someone running
//!    a large chat model and a small background one should not be nagged about something that only
//!    beats the small one.
//! 3. **A candidate must be runnable and meaningfully bigger** — a fit PM actually computed, a
//!    verdict no worse than the one being replaced, and [`MIN_IMPROVEMENT`] more parameters.
//! 4. **A model already on disk wins.** "You already have this downloaded" is a far better
//!    suggestion than "download this", and costs the user nothing.
//! 5. **Only a model PM's pick could choose.** A candidate the pick below would exclude — one that
//!    fits system RAM but not the graphics card, or one too slow for background work — is never
//!    volunteered here either, so the notice and the pick cannot contradict each other.
//!
//! Flag, never gate: the caller surfaces this passively and the user can always ignore it. Whether
//! it is time to look at all is the *cadence*'s decision ([`crate::local_catalog::rescan_due`]),
//! which this module knows nothing about.
//!
//! ## PM's pick for this computer
//!
//! The second question this module answers, for the top of the Local AI tab: of the curated list
//! and the models the user already has, which ONE would PM run here ([`pick`])? Pure, like the
//! notice, so every rule is pinned below without a machine.
//!
//! The pick writes nothing, never filters or reorders the curated list, never changes a badge, and
//! ranks on memory residency (the ±15%/never-under memory contract) and parameter count only — never
//! on tok/s. It supersedes DECISIONS 2026-07-24 'Auto that decides for you' for this one item only,
//! by Bobby's decision.
//!
//! The rules, in order:
//!
//! 1. **Where it would run decides how it is judged** ([`basis_for`]). With a discrete graphics card
//!    a model must fit entirely on it, with the reserve PM keeps free there ([`fit::resident_fit`]):
//!    one that spills into system RAM replies many times slower. Without one — or on memory shared
//!    with the processor — the RAM fit stands, but only if a cautious estimate says it is quick
//!    enough for background work ([`background_floor_tps`]).
//! 2. **The largest eligible model wins**, ties broken by repo so the answer is stable. Eligible is a
//!    runnable config (Comfortable or Tight, never a halved context) that Ollama can fetch.
//! 3. **A model the user already has wins** when it fits the same way and nothing eligible is at
//!    least [`MIN_IMPROVEMENT`] larger — the same 15% the notice uses.
//! 4. **When nothing qualifies, say why** ([`NoPick`]). Never the least-bad option.

use serde::Serialize;

use crate::local_slot::tunables;
use crate::{fit, local_disk};

/// How much larger a candidate must be before it counts as an improvement rather than noise. 15% is
/// comfortably past the gap between neighbouring sizes of the same family (a 7B vs an 8B is not worth
/// a notice) while still catching a real step up (7B → 14B).
const MIN_IMPROVEMENT: f64 = 1.15;

/// One model this machine could run, as the comparison sees it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub repo: String,
    pub display_name: String,
    pub parameters_b: f64,
    pub verdict: fit::Verdict,
    /// Already downloaded to this machine (#449) — the strongest kind of suggestion, since acting on
    /// it costs nothing.
    pub on_disk: bool,
    /// What it would occupy, from its own `FitResult`. `None` when it could not be sized — which
    /// [`is_runnable`] already excludes, so in practice this is `Some` for anything that survives to
    /// the joint check.
    pub footprint_gb: Option<f64>,
    /// The pick could choose it: [`judge`] found a config that runs where it should and Ollama can
    /// fetch it. `false` keeps it out of [`suggest`] — rule 5 above.
    pub pick_eligible: bool,
}

/// What a suggestion has to share the machine with.
///
/// [`baseline`] picks the LARGER of the two assigned models and drops the other on the floor, so
/// without this a candidate is scored against the whole budget as though it were alone — and PM's own
/// nudge could talk someone into exactly the model-swapping the co-residency warning was added to
/// tell them about. That is the bug this type exists to close, not a readout.
#[derive(Debug, Clone, Copy)]
pub struct Beside {
    /// The footprint of the model on the role `baseline` did not pick.
    pub footprint_gb: f64,
    /// The budget the pair has to fit inside — [`fit::ram_budget_gb`], the same one the candidate's
    /// own verdict was computed against.
    pub budget_gb: f64,
}

/// The suggestion to surface, if there is one worth making.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Suggestion {
    pub repo: String,
    pub display_name: String,
    /// The model it improves on, so the copy can name both.
    pub replaces: String,
    /// It's already on this machine, so the suggestion is "use it", not "download it".
    pub already_downloaded: bool,
}

/// The best model currently assigned to any role, as the baseline to beat. `None` when nothing local
/// is assigned, or when nothing assigned could be matched to the catalog (an unknown model can't be
/// compared against, and guessing would be worse than staying quiet).
pub fn baseline<'a>(assigned: impl IntoIterator<Item = &'a Candidate>) -> Option<&'a Candidate> {
    assigned
        .into_iter()
        .max_by(|a, b| a.parameters_b.total_cmp(&b.parameters_b))
}

/// The model worth suggesting over `current`, if any.
///
/// `candidates` is every scored curated model; the caller marks the ones already on disk. Ties break
/// toward a model already downloaded, then toward the larger one, then by repo so the choice is
/// stable across calls (a suggestion that flickers between two equals is its own kind of noise).
pub fn suggest(
    current: Option<&Candidate>,
    candidates: &[Candidate],
    beside: Option<Beside>,
) -> Option<Suggestion> {
    let current = current?;
    // A baseline we couldn't score is not a baseline — no honest comparison exists.
    if !is_runnable(current.verdict) {
        return None;
    }
    candidates
        .iter()
        .filter(|c| c.repo != current.repo)
        .filter(|c| c.pick_eligible)
        .filter(|c| is_runnable(c.verdict))
        // No worse a fit than what's already running — a bigger model that only fits at a halved
        // context is not an upgrade.
        .filter(|c| rank(c.verdict) <= rank(current.verdict))
        .filter(|c| c.parameters_b >= current.parameters_b * MIN_IMPROVEMENT)
        .filter(|c| fits_beside(c, beside))
        .max_by(|a, b| {
            a.on_disk
                .cmp(&b.on_disk)
                .then(a.parameters_b.total_cmp(&b.parameters_b))
                .then_with(|| b.repo.cmp(&a.repo))
        })
        .map(|best| Suggestion {
            repo: best.repo.clone(),
            display_name: best.display_name.clone(),
            replaces: current.display_name.clone(),
            already_downloaded: best.on_disk,
        })
}

/// Whether a candidate still fits once the OTHER role's model is on the machine too.
///
/// `None` disables the check — one role on cloud, or both roles on the same model, so there is no
/// second footprint to make room for. A candidate PM could not size is REJECTED rather than waved
/// through: it cannot be shown to fit, and a suggestion is something PM volunteers unprompted, so
/// silence is the safe default. Strict, with no tolerance band, for the same reason — the band in
/// `fit::co_residency` exists so a WARNING is not over-confident, while here the conservative move is
/// to keep quiet.
fn fits_beside(c: &Candidate, beside: Option<Beside>) -> bool {
    let Some(beside) = beside else {
        return true;
    };
    c.footprint_gb
        .is_some_and(|f| f + beside.footprint_gb <= beside.budget_gb)
}

/// Whether a verdict describes a model this machine can actually run well. `HalvedContext` is
/// deliberately excluded as a *suggestion*: recommending a model that only fits by cutting the
/// context in half is not an improvement to volunteer, even though PM will happily run it if asked.
pub(crate) fn is_runnable(v: fit::Verdict) -> bool {
    matches!(v, fit::Verdict::Comfortable | fit::Verdict::Tight)
}

// --- PM's pick for this computer ---------------------------------------------------------------

/// Where PM's pick would run, which decides how a model is judged ([`judge`]) and how the copy words
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickBasis {
    /// A discrete graphics card: the model must fit on it.
    Gpu,
    /// Memory shared between the processor and graphics (Apple Silicon, an APU, an iGPU).
    Shared,
    /// No graphics card figure at all: system RAM.
    System,
}

/// The basis for this machine. Unified memory is `Shared` whatever its VRAM figure says, because
/// that figure is a slice of the same RAM — the same reading [`fit::gpu_fit`] makes of it.
pub fn basis_for(hw: &fit::FitHardware) -> PickBasis {
    if hw.unified_memory {
        PickBasis::Shared
    } else if hw.vram_gb.is_some() {
        PickBasis::Gpu
    } else {
        PickBasis::System
    }
}

/// Which of a curated card's rungs the judged config is: its highest-quality (RAM) config, or the
/// smaller one that stays on the graphics card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    Quality,
    Gpu,
}

/// Why PM is not picking a model for this computer — each one its own sentence in the UI, because
/// "nothing fits" means three different things and only one of them is about memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoPick {
    /// There is a graphics card, nothing fits on it, and something would run from system RAM.
    NothingOnGpu,
    /// Something fits, but only from system or shared memory, too slowly for background work.
    TooSlow,
    /// Nothing in the list fits the memory that is free.
    TooLittleMemory,
}

/// The slowest decode speed at which PM's largest background reply still finishes inside the
/// background budget, after the cold load that budget already allows for: [`REPLY_RESERVE_TOKENS`]
/// over (180 s − 60 s), about 8.53 tokens a second.
///
/// Decode only — prompt processing is not counted — so it is a cautious screen, not a guarantee: a
/// model that passes it may still be slow on a long prompt, and the copy says "by PM's cautious
/// estimate" for that reason. Compared only against [`fit::system_tokens_per_sec`], the deliberately
/// low system-RAM figure, never against a GPU estimate.
///
/// [`REPLY_RESERVE_TOKENS`]: crate::context_budget::REPLY_RESERVE_TOKENS
pub fn background_floor_tps() -> f64 {
    crate::context_budget::REPLY_RESERVE_TOKENS as f64
        / (tunables::BACKGROUND_TOTAL_TIMEOUT - tunables::COLD_LOAD_ALLOWANCE).as_secs_f64()
}

/// One model as the pick judges it.
#[derive(Debug, Clone)]
pub struct Judged {
    /// The config PM would run it at here, or `None` when there is no acceptable one: nothing
    /// runnable fits the card on a GPU basis, or the RAM config is not runnable or is too slow.
    pub config: Option<fit::FitResult>,
    /// Which rung `config` is. `Quality` when there is no config.
    pub rung: Rung,
    /// The RAM config is runnable. The caller narrows this to configs Ollama can also fetch, since
    /// this function knows no catalogue.
    pub ram_runnable: bool,
    /// Shared/System only: the RAM config is runnable but fails [`background_floor_tps`].
    pub too_slow: bool,
}

/// Judge one model for the pick, given its RAM fit. Pure.
///
/// On a GPU basis the config is [`fit::resident_fit`], kept only when runnable, and no speed floor
/// applies — a config resident on a discrete card is far above it. On a Shared or System basis the
/// RAM fit itself is the config, but only when [`fit::system_tokens_per_sec`] clears the floor.
/// The floor applies on Shared too: shared memory is not a faster pool than the RAM it comes from.
pub fn judge(spec: &fit::ModelSpec, hw: &fit::FitHardware, ram_fit: &fit::FitResult) -> Judged {
    let ram_runnable = is_runnable(ram_fit.verdict);
    match basis_for(hw) {
        PickBasis::Gpu => {
            let config = fit::resident_fit(spec, hw, ram_fit).filter(|g| is_runnable(g.verdict));
            let rung = match &config {
                Some(g)
                    if (g.quant, g.context, g.kv)
                        != (ram_fit.quant, ram_fit.context, ram_fit.kv) =>
                {
                    Rung::Gpu
                }
                _ => Rung::Quality,
            };
            Judged {
                config,
                rung,
                ram_runnable,
                too_slow: false,
            }
        }
        PickBasis::Shared | PickBasis::System => {
            let quick = ram_fit.quant.is_some_and(|q| {
                fit::system_tokens_per_sec(spec.active_params_b, q) >= background_floor_tps()
            });
            Judged {
                config: (ram_runnable && quick).then(|| ram_fit.clone()),
                rung: Rung::Quality,
                ram_runnable,
                too_slow: ram_runnable && !quick,
            }
        }
    }
}

/// One curated model, as the pick weighs it.
#[derive(Debug, Clone)]
pub struct CatalogueOption {
    pub repo: String,
    pub display_name: String,
    pub parameters_b: f64,
    pub judged: Judged,
    /// The Ollama tag for the judged config's quant, or `None` when there is no config or that quant
    /// cannot be fetched.
    pub tag: Option<String>,
    /// The download for the judged config's quant — weights plus any projector, which Ollama pulls
    /// with them — read from the same catalogue row as `tag`. `None` only when there is no config.
    pub download_gb: Option<f64>,
    /// Its RAM config would be fine from system memory: runnable, fetchable and over the floor. What
    /// `NothingOnGpu`'s `system_fallback` reports.
    pub system_ok: bool,
}

/// One model the user already has — served by their endpoint, or found on disk.
#[derive(Debug, Clone)]
pub struct OwnedOption {
    /// The runner's own name for it: the served id, or the on-disk name.
    pub id: String,
    pub repo: String,
    pub display_name: String,
    pub parameters_b: f64,
    /// The endpoint serves it, as opposed to it only being on disk.
    pub served: bool,
    pub source: Option<local_disk::DiskSource>,
    pub path: Option<String>,
    pub shards: u32,
    /// The figures describe the user's own file (its real size and quant), not the catalogue's
    /// guess at it.
    pub measured: bool,
    /// The judged config, as [`Judged::config`]. No rung beside it: the owned pick shows the one
    /// config, and has no card whose rungs it would point between.
    pub config: Option<fit::FitResult>,
    /// A role is already set to it.
    pub bound: bool,
}

/// The owned model a catalogue pick is weighed against, so the copy can name it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnedRef {
    pub id: String,
    pub display_name: String,
    pub served: bool,
}

/// PM's pick for this computer. The serialized shape is a contract with the TypeScript mirror: the
/// variant is named in a `kind` field (`catalogue` | `owned` | `nothing`) beside its own fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pick {
    /// A model from the curated list, to download.
    Catalogue {
        repo: String,
        display_name: String,
        rung: Rung,
        tag: String,
        fit: fit::FitResult,
        download_gb: f64,
        basis: PickBasis,
        /// The best model the user already has that fits this machine the same way, which this one
        /// is at least 15% larger than — so the copy can say why PM still points at the download.
        also_have: Option<OwnedRef>,
    },
    /// A model the user already has.
    Owned {
        id: String,
        repo: String,
        display_name: String,
        served: bool,
        source: Option<local_disk::DiskSource>,
        path: Option<String>,
        shards: u32,
        measured: bool,
        fit: fit::FitResult,
        basis: PickBasis,
    },
    /// Nothing suits this computer, and why.
    Nothing {
        reason: NoPick,
        basis: PickBasis,
        /// `NothingOnGpu` only: some model would be fine from system memory, so the copy can say
        /// "some would run, several times slower" rather than "none would".
        system_fallback: bool,
    },
}

/// PM's pick. Pure: the caller has already judged every option.
///
/// Ranks on parameter count only. Speed is never a key — ranking by GPU throughput would pick the
/// smallest model in the list every time.
pub fn pick(basis: PickBasis, catalogue: &[CatalogueOption], owned: &[OwnedOption]) -> Pick {
    // C*: the largest eligible curated model, ties to the lower repo so the answer never depends on
    // input order.
    let best = catalogue
        .iter()
        .filter_map(|o| match (&o.judged.config, &o.tag, o.download_gb) {
            (Some(config), Some(tag), Some(download_gb)) if is_runnable(config.verdict) => {
                Some((o, config, tag, download_gb))
            }
            _ => None,
        })
        .max_by(|a, b| {
            a.0.parameters_b
                .total_cmp(&b.0.parameters_b)
                .then_with(|| b.0.repo.cmp(&a.0.repo))
        });

    let mut runnable: Vec<(&OwnedOption, &fit::FitResult)> = owned
        .iter()
        .filter_map(|o| {
            o.config
                .as_ref()
                .filter(|c| is_runnable(c.verdict))
                .map(|c| (o, c))
        })
        .collect();
    // A model a role already uses first, then one the server already has, then the larger, then by
    // id so the order is total.
    runnable.sort_by(|(a, _), (b, _)| {
        b.bound
            .cmp(&a.bound)
            .then(b.served.cmp(&a.served))
            .then(b.parameters_b.total_cmp(&a.parameters_b))
            .then_with(|| a.id.cmp(&b.id))
    });

    // An owned model wins unless something eligible is at least 15% larger than it.
    let qualifies = |o: &OwnedOption| {
        best.is_none_or(|(c, ..)| o.parameters_b * MIN_IMPROVEMENT > c.parameters_b)
    };
    if let Some((o, config)) = runnable.iter().find(|(o, _)| qualifies(o)) {
        return Pick::Owned {
            id: o.id.clone(),
            repo: o.repo.clone(),
            display_name: o.display_name.clone(),
            served: o.served,
            source: o.source,
            path: o.path.clone(),
            shards: o.shards,
            measured: o.measured,
            fit: (*config).clone(),
            basis,
        };
    }

    if let Some((c, config, tag, download_gb)) = best {
        // Every runnable owned model failed only the 15% test, so the first of them is the one to
        // name beside the pick.
        let also_have = runnable.first().map(|(o, _)| OwnedRef {
            id: o.id.clone(),
            display_name: o.display_name.clone(),
            served: o.served,
        });
        return Pick::Catalogue {
            repo: c.repo.clone(),
            display_name: c.display_name.clone(),
            rung: c.judged.rung,
            tag: tag.clone(),
            fit: config.clone(),
            download_gb,
            basis,
            also_have,
        };
    }

    let reason = match basis {
        PickBasis::Gpu if catalogue.iter().any(|o| o.judged.ram_runnable) => NoPick::NothingOnGpu,
        PickBasis::Shared | PickBasis::System if catalogue.iter().any(|o| o.judged.too_slow) => {
            NoPick::TooSlow
        }
        _ => NoPick::TooLittleMemory,
    };
    Pick::Nothing {
        reason,
        basis,
        system_fallback: reason == NoPick::NothingOnGpu && catalogue.iter().any(|o| o.system_ok),
    }
}

fn rank(v: fit::Verdict) -> u8 {
    match v {
        fit::Verdict::Comfortable => 0,
        fit::Verdict::Tight => 1,
        fit::Verdict::HalvedContext => 2,
        fit::Verdict::StayOnCloud => 3,
        fit::Verdict::Unknown => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(repo: &str, params: f64, verdict: fit::Verdict, on_disk: bool) -> Candidate {
        Candidate {
            repo: repo.to_string(),
            display_name: repo.to_string(),
            parameters_b: params,
            verdict,
            on_disk,
            // Roughly a byte per param at Q8, which is only ever compared against a budget the test
            // chooses — the joint check is what it exists for, and every other test disables that
            // check by passing `None`.
            footprint_gb: Some(params),
            pick_eligible: true,
        }
    }

    #[test]
    fn nothing_is_suggested_without_a_model_to_improve_on() {
        let pool = vec![cand("big", 70.0, fit::Verdict::Comfortable, false)];
        // Pitching local AI at someone who hasn't set it up is a different feature.
        assert_eq!(suggest(None, &pool, None), None);
    }

    #[test]
    fn a_meaningfully_bigger_model_that_still_fits_is_suggested() {
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            cand("small", 7.0, fit::Verdict::Comfortable, false),
            cand("mid", 14.0, fit::Verdict::Comfortable, false),
        ];
        let s = suggest(Some(&current), &pool, None).unwrap();
        assert_eq!(s.repo, "mid");
        assert_eq!(s.replaces, "small");
        assert!(!s.already_downloaded);
    }

    #[test]
    fn a_marginal_size_difference_is_not_worth_a_notice() {
        // 7B → 8B is within the noise of one family's sizes; nagging about it trains people to
        // ignore the notice entirely.
        let current = cand("seven", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![cand("eight", 8.0, fit::Verdict::Comfortable, false)];
        assert_eq!(suggest(Some(&current), &pool, None), None);
        // 7B → 14B is a real step up.
        let pool = vec![cand("fourteen", 14.0, fit::Verdict::Comfortable, false)];
        assert!(suggest(Some(&current), &pool, None).is_some());
    }

    #[test]
    fn a_bigger_model_with_a_worse_fit_is_not_an_upgrade() {
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        // Bigger, but only at a halved context, or not at all — neither is worth volunteering.
        let pool = vec![
            cand("halved", 32.0, fit::Verdict::HalvedContext, false),
            cand("cloud", 70.0, fit::Verdict::StayOnCloud, false),
            cand("unknown", 70.0, fit::Verdict::Unknown, false),
        ];
        assert_eq!(suggest(Some(&current), &pool, None), None);

        // A tight fit is still a fit — but only when the current model isn't already comfortable.
        let tight_current = cand("small", 7.0, fit::Verdict::Tight, false);
        let pool = vec![cand("bigger", 14.0, fit::Verdict::Tight, false)];
        assert!(suggest(Some(&tight_current), &pool, None).is_some());
        assert_eq!(suggest(Some(&current), &pool, None), None);
    }

    #[test]
    fn a_model_already_on_disk_wins_over_a_bigger_download() {
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            cand("downloaded", 14.0, fit::Verdict::Comfortable, true),
            cand(
                "bigger-but-not-here",
                32.0,
                fit::Verdict::Comfortable,
                false,
            ),
        ];
        let s = suggest(Some(&current), &pool, None).unwrap();
        assert_eq!(s.repo, "downloaded");
        assert!(s.already_downloaded, "costs the user nothing to act on");
    }

    #[test]
    fn the_current_model_is_never_suggested_back_to_itself() {
        let current = cand("same", 14.0, fit::Verdict::Comfortable, false);
        let pool = vec![cand("same", 14.0, fit::Verdict::Comfortable, true)];
        assert_eq!(suggest(Some(&current), &pool, None), None);
    }

    #[test]
    fn an_unscoreable_current_model_yields_no_comparison() {
        // If we can't say how well what they run fits, we can't honestly say something fits better.
        let current = cand("mystery", 7.0, fit::Verdict::Unknown, false);
        let pool = vec![cand("big", 70.0, fit::Verdict::Comfortable, false)];
        assert_eq!(suggest(Some(&current), &pool, None), None);
    }

    #[test]
    fn the_baseline_is_the_best_model_already_running() {
        let chat = cand("chat", 14.0, fit::Verdict::Comfortable, false);
        let background = cand("background", 3.0, fit::Verdict::Comfortable, false);
        let base = baseline([&chat, &background]).unwrap();
        assert_eq!(base.repo, "chat", "the largest assigned model sets the bar");

        // So a model that only beats the small background one is not suggested.
        let pool = vec![cand("mid", 8.0, fit::Verdict::Comfortable, false)];
        assert_eq!(suggest(Some(base), &pool, None), None);

        assert!(baseline(std::iter::empty()).is_none());
    }

    #[test]
    fn a_model_that_fits_alone_but_not_beside_the_other_role_is_not_suggested() {
        // The live half of #786 item 6. `baseline` picks the LARGER of the two assigned models and
        // drops the other, so a suggestion was scored against the whole budget as though the machine
        // held nothing else — letting PM talk someone into exactly the model-swapping the warning
        // beside it was added to describe.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            current.clone(),
            cand("mid", 14.0, fit::Verdict::Comfortable, false),
        ];

        // Alone, the 14 is a clear upgrade and is offered.
        assert!(suggest(Some(&current), &pool, None).is_some());

        // Beside a 6 GB model in a 16 GB budget it no longer fits (14 + 6 > 16), so it must not be.
        let beside = Beside {
            footprint_gb: 6.0,
            budget_gb: 16.0,
        };
        assert_eq!(suggest(Some(&current), &pool, Some(beside)), None);

        // Give the pair room and the same suggestion comes back — the filter has to be about the
        // arithmetic, not about the presence of a second model.
        let roomy = Beside {
            footprint_gb: 6.0,
            budget_gb: 32.0,
        };
        assert_eq!(
            suggest(Some(&current), &pool, Some(roomy)).unwrap().repo,
            "mid"
        );
    }

    #[test]
    fn a_candidate_with_no_footprint_is_kept_quiet_rather_than_waved_through() {
        // A suggestion is volunteered, not asked for, so "cannot be shown to fit" must resolve to
        // silence. Waving it through would make the joint check decorative.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let mut unsized_big = cand("mid", 14.0, fit::Verdict::Comfortable, false);
        unsized_big.footprint_gb = None;
        let pool = vec![current.clone(), unsized_big];

        assert!(
            suggest(Some(&current), &pool, None).is_some(),
            "no check, no rejection"
        );
        let beside = Beside {
            footprint_gb: 1.0,
            budget_gb: 999.0,
        };
        assert_eq!(suggest(Some(&current), &pool, Some(beside)), None);
    }

    #[test]
    fn the_choice_is_stable_across_calls() {
        // Two equally good candidates must always resolve the same way — a suggestion that flickers
        // between them on every refresh is its own kind of noise.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let pool = vec![
            cand("alpha", 14.0, fit::Verdict::Comfortable, false),
            cand("beta", 14.0, fit::Verdict::Comfortable, false),
        ];
        let first = suggest(Some(&current), &pool, None).unwrap();
        let reversed: Vec<Candidate> = pool.into_iter().rev().collect();
        assert_eq!(suggest(Some(&current), &reversed, None).unwrap(), first);
    }

    #[test]
    fn the_notice_never_suggests_a_model_the_pick_would_exclude() {
        // Fits system RAM comfortably and is twice the size — but not on the graphics card, so the
        // pick would never choose it. The notice used to; that is the contradiction rule 5 closes.
        let current = cand("small", 7.0, fit::Verdict::Comfortable, false);
        let mut spills = cand("spills-to-ram", 14.0, fit::Verdict::Comfortable, false);
        spills.pick_eligible = false;
        assert_eq!(suggest(Some(&current), &[spills.clone()], None), None);
        spills.pick_eligible = true;
        assert!(suggest(Some(&current), &[spills], None).is_some());
    }

    // ---- PM's pick ----

    fn config(verdict: fit::Verdict) -> fit::FitResult {
        fit::FitResult {
            verdict,
            quant: Some(fit::Quant::Q4_K_M),
            context: Some(32768),
            kv: fit::KvCache::F16,
            est_memory_gb: Some(5.0),
            est_tokens_per_sec: Some(50.0),
            speed_basis: Some(fit::SpeedBasis::GpuPublished),
            notes: vec![],
        }
    }

    /// A curated option. `eligible` decides whether it has a runnable, fetchable config.
    fn option(repo: &str, params: f64, eligible: bool) -> CatalogueOption {
        CatalogueOption {
            repo: repo.to_string(),
            display_name: repo.to_string(),
            parameters_b: params,
            judged: Judged {
                config: eligible.then(|| config(fit::Verdict::Tight)),
                rung: Rung::Gpu,
                ram_runnable: true,
                too_slow: false,
            },
            tag: eligible.then(|| format!("hf.co/{repo}:Q4_K_M")),
            download_gb: eligible.then_some(params * 0.6),
            system_ok: false,
        }
    }

    fn owned(id: &str, params: f64, served: bool, bound: bool) -> OwnedOption {
        OwnedOption {
            id: id.to_string(),
            repo: format!("example/{id}"),
            display_name: id.to_string(),
            parameters_b: params,
            served,
            source: (!served).then_some(local_disk::DiskSource::Ollama),
            path: None,
            shards: 1,
            measured: true,
            config: Some(config(fit::Verdict::Comfortable)),
            bound,
        }
    }

    fn repo_of(p: &Pick) -> &str {
        match p {
            Pick::Catalogue { repo, .. } | Pick::Owned { repo, .. } => repo,
            Pick::Nothing { .. } => "",
        }
    }

    #[test]
    fn the_largest_eligible_model_is_the_pick_whatever_its_speed() {
        // The faster small model loses on size, which is the only key. Ranking by GPU throughput
        // would pick the smallest model in the list every time.
        let mut fast = option("tiny", 0.41, true);
        fast.judged.config.as_mut().unwrap().est_tokens_per_sec = Some(884.0);
        let pool = vec![
            fast,
            option("seven", 7.62, true),
            // Larger, but with no config that fits where it should.
            option("fourteen", 14.77, false),
        ];
        let p = pick(PickBasis::Gpu, &pool, &[]);
        assert!(matches!(p, Pick::Catalogue { .. }), "{p:?}");
        assert_eq!(repo_of(&p), "seven");

        // An eligible config with no Ollama tag is not eligible: the pick names a download.
        let mut untagged = option("big", 72.0, true);
        untagged.tag = None;
        let p = pick(
            PickBasis::Gpu,
            &[untagged, option("seven", 7.62, true)],
            &[],
        );
        assert_eq!(repo_of(&p), "seven");
    }

    #[test]
    fn a_tie_resolves_by_repo_whatever_the_input_order() {
        let pool = vec![option("beta", 7.0, true), option("alpha", 7.0, true)];
        let first = pick(PickBasis::Gpu, &pool, &[]);
        assert_eq!(repo_of(&first), "alpha");
        let reversed: Vec<CatalogueOption> = pool.into_iter().rev().collect();
        assert_eq!(pick(PickBasis::Gpu, &reversed, &[]), first);
    }

    #[test]
    fn a_model_you_already_have_wins_unless_the_pick_is_fifteen_percent_larger() {
        let pool = vec![option("qwen-7b", 7.62, true)];

        // The same size: PM points at the one you have.
        let p = pick(
            PickBasis::Gpu,
            &pool,
            &[owned("qwen2.5:7b", 7.62, true, false)],
        );
        match &p {
            Pick::Owned {
                id, served, basis, ..
            } => {
                assert_eq!(id, "qwen2.5:7b");
                assert!(served);
                assert_eq!(*basis, PickBasis::Gpu);
            }
            other => panic!("expected Owned, got {other:?}"),
        }

        // 3.88 × 1.15 < 7.62: the pick stands, and names the model you have beside it.
        let p = pick(
            PickBasis::Gpu,
            &pool,
            &[owned("gemma3:4b", 3.88, true, false)],
        );
        match &p {
            Pick::Catalogue {
                repo, also_have, ..
            } => {
                assert_eq!(repo, "qwen-7b");
                assert_eq!(
                    also_have.as_ref(),
                    Some(&OwnedRef {
                        id: "gemma3:4b".into(),
                        display_name: "gemma3:4b".into(),
                        served: true,
                    })
                );
            }
            other => panic!("expected Catalogue, got {other:?}"),
        }

        // An owned model with no acceptable config is neither picked nor named.
        let mut halved = owned("qwen2.5:7b", 7.62, true, false);
        halved.config = Some(config(fit::Verdict::HalvedContext));
        match pick(PickBasis::Gpu, &pool, &[halved]) {
            Pick::Catalogue { also_have, .. } => assert_eq!(also_have, None),
            other => panic!("expected Catalogue, got {other:?}"),
        }

        // With nothing eligible in the list, any runnable owned model qualifies.
        let p = pick(
            PickBasis::Gpu,
            &[option("big", 14.0, false)],
            &[owned("gemma3:4b", 3.88, true, false)],
        );
        assert!(matches!(p, Pick::Owned { .. }), "{p:?}");
    }

    #[test]
    fn among_owned_models_the_one_in_use_comes_first() {
        let pool = vec![option("qwen-7b", 7.62, true)];
        let have = vec![
            owned("larger-served", 8.0, true, false),
            owned("on-disk", 9.0, false, false),
            owned("in-use", 7.62, true, true),
        ];
        let id = |p: Pick| match p {
            Pick::Owned { id, .. } => id,
            other => panic!("expected Owned, got {other:?}"),
        };
        assert_eq!(id(pick(PickBasis::Gpu, &pool, &have)), "in-use");
        // Then served before on disk, even when the disk copy is larger.
        assert_eq!(id(pick(PickBasis::Gpu, &pool, &have[..2])), "larger-served");
        // Then the larger, then by id.
        let disk_only = vec![
            owned("b-disk", 8.0, false, false),
            owned("a-disk", 8.0, false, false),
            owned("small-disk", 7.7, false, false),
        ];
        assert_eq!(id(pick(PickBasis::Gpu, &pool, &disk_only)), "a-disk");
    }

    #[test]
    fn when_nothing_suits_the_pick_says_why() {
        // A graphics card, nothing on it, and something that would run from system RAM.
        let mut spills = option("spills", 7.62, false);
        spills.judged.ram_runnable = true;
        let p = pick(PickBasis::Gpu, &[spills.clone()], &[]);
        assert_eq!(
            p,
            Pick::Nothing {
                reason: NoPick::NothingOnGpu,
                basis: PickBasis::Gpu,
                system_fallback: false,
            }
        );
        // ... and one of those would be fine from system memory.
        spills.system_ok = true;
        assert_eq!(
            pick(PickBasis::Gpu, &[spills], &[]),
            Pick::Nothing {
                reason: NoPick::NothingOnGpu,
                basis: PickBasis::Gpu,
                system_fallback: true,
            }
        );

        // No card: something fits, but too slowly.
        let mut slow = option("slow", 72.0, false);
        slow.judged.too_slow = true;
        for basis in [PickBasis::System, PickBasis::Shared] {
            assert_eq!(
                pick(basis, &[slow.clone()], &[]),
                Pick::Nothing {
                    reason: NoPick::TooSlow,
                    basis,
                    system_fallback: false,
                }
            );
        }

        // Nothing fits at all.
        let mut none = option("none", 0.41, false);
        none.judged.ram_runnable = false;
        for basis in [PickBasis::Gpu, PickBasis::System, PickBasis::Shared] {
            assert_eq!(
                pick(basis, &[none.clone()], &[]),
                Pick::Nothing {
                    reason: NoPick::TooLittleMemory,
                    basis,
                    system_fallback: false,
                }
            );
        }
    }

    #[test]
    fn the_background_floor_is_the_largest_reply_over_what_the_budget_leaves() {
        // 1024 tokens in (180 s − 60 s cold load).
        assert_eq!(background_floor_tps(), 1024.0 / 120.0);
    }

    #[test]
    fn the_speed_floor_applies_off_the_graphics_card_only() {
        // Qwen2.5 7B at Q8_0: 4.95 tok/s from system RAM, under the 8.53 floor.
        let spec = fit::ModelSpec {
            arch: fit::Architecture::Dense,
            active_params_b: 7.62,
            target_context: 32768,
            projector_gb: None,
            candidates: vec![fit::QuantCandidate {
                quant: fit::Quant::Q8_0,
                weight_gb: 7.54,
            }],
        };
        let system = fit::FitHardware {
            available_ram_gb: 32.0,
            vram_gb: None,
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        };
        let shared = fit::FitHardware {
            vram_gb: Some(24.0),
            unified_memory: true,
            ..system
        };
        for hw in [system, shared] {
            let rf = fit::fit(&spec, &hw);
            assert!(is_runnable(rf.verdict));
            let j = judge(&spec, &hw, &rf);
            assert!(j.config.is_none(), "{hw:?}");
            assert!(j.too_slow && j.ram_runnable, "{hw:?}");
        }

        // On a discrete card it is resident, and no floor applies.
        let card = fit::FitHardware {
            vram_gb: Some(24.0),
            gpu_bandwidth_gbps: Some(1008.0),
            ..system
        };
        let rf = fit::fit(&spec, &card);
        let j = judge(&spec, &card, &rf);
        assert!(!j.too_slow);
        let config = j.config.expect("resident on a 24 GB card");
        assert_eq!(
            config, rf,
            "the quality config already fits with the reserve"
        );
        assert_eq!(j.rung, Rung::Quality);

        // A model that clears the floor from system RAM is judged on its RAM fit as it stands.
        let small = fit::ModelSpec {
            active_params_b: 3.82,
            candidates: vec![fit::QuantCandidate {
                quant: fit::Quant::Q8_0,
                weight_gb: 3.78,
            }],
            ..spec
        };
        let rf = fit::fit(&small, &system);
        let j = judge(&small, &system, &rf);
        assert_eq!(j.config, Some(rf));
        assert!(!j.too_slow);
    }

    #[test]
    fn a_config_that_had_to_shrink_to_stay_on_the_card_is_the_gpu_rung() {
        // 8 GB card, plenty of RAM: the quality config is Q8_0 off the card, the resident one is the
        // Q4_K_M that fits with the reserve.
        let spec = fit::ModelSpec {
            arch: fit::Architecture::Dense,
            active_params_b: 7.0,
            target_context: 32768,
            projector_gb: None,
            candidates: vec![
                fit::QuantCandidate {
                    quant: fit::Quant::Q8_0,
                    weight_gb: 7.5,
                },
                fit::QuantCandidate {
                    quant: fit::Quant::Q4_K_M,
                    weight_gb: 4.4,
                },
            ],
        };
        let hw = fit::FitHardware {
            available_ram_gb: 22.0,
            vram_gb: Some(8.0),
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        };
        let rf = fit::fit(&spec, &hw);
        let j = judge(&spec, &hw, &rf);
        assert_eq!(j.rung, Rung::Gpu);
        assert_eq!(j.config.unwrap().quant, Some(fit::Quant::Q4_K_M));
    }

    #[test]
    fn the_pick_serializes_the_shape_the_frontend_mirrors() {
        let p = pick(
            PickBasis::Gpu,
            &[option("qwen-7b", 7.62, true)],
            &[owned("gemma3:4b", 3.88, true, false)],
        );
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "catalogue");
        assert_eq!(v["rung"], "gpu");
        assert_eq!(v["basis"], "gpu");
        assert_eq!(v["tag"], "hf.co/qwen-7b:Q4_K_M");
        assert_eq!(v["fit"]["speed_basis"], "gpu_published");
        assert_eq!(v["also_have"]["id"], "gemma3:4b");
        assert_eq!(v["also_have"]["served"], true);

        let v = serde_json::to_value(pick(
            PickBasis::Gpu,
            &[],
            &[owned("disk-model", 7.0, false, false)],
        ))
        .unwrap();
        assert_eq!(v["kind"], "owned");
        assert_eq!(v["source"], "ollama");
        assert_eq!(v["measured"], true);
        assert_eq!(v["path"], serde_json::Value::Null);

        let v = serde_json::to_value(pick(PickBasis::System, &[], &[])).unwrap();
        assert_eq!(v["kind"], "nothing");
        assert_eq!(v["reason"], "too_little_memory");
        assert_eq!(v["basis"], "system");
        assert_eq!(v["system_fallback"], false);
        for (reason, wire) in [
            (NoPick::NothingOnGpu, "nothing_on_gpu"),
            (NoPick::TooSlow, "too_slow"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), wire);
        }
        assert_eq!(serde_json::to_value(PickBasis::Shared).unwrap(), "shared");
        assert_eq!(serde_json::to_value(Rung::Quality).unwrap(), "quality");
    }
}
