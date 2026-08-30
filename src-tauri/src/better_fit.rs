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
//!
//! Flag, never gate: the caller surfaces this passively and the user can always ignore it. Whether
//! it is time to look at all is the *cadence*'s decision ([`crate::local_catalog::rescan_due`]),
//! which this module knows nothing about.

use crate::fit;

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
fn is_runnable(v: fit::Verdict) -> bool {
    matches!(v, fit::Verdict::Comfortable | fit::Verdict::Tight)
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
}
