// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Model-aware context budgeting (board card 7D, #143) — the **pure** logic behind the chat context-usage
//! meter and its ~80% alert. No DB, no `AppHandle`, no network: the command layer (`commands.rs`) reads the
//! numbers (the selected model's window from `model_pricing`, the measured `last_prompt_tokens`, the
//! un-summarised tail) and the action engine (`chat_summary::compress_now`) reclaims context; everything
//! *decided* from those numbers lives here so it is unit-testable without a model or a live store.
//!
//! The card's locked decisions, made concrete:
//!   * **Measured, not estimated.** The meter's numerator is the exact `prompt_tokens` OpenRouter reported
//!     for the last reply (persisted on `chat_sessions`), over the selected model's `context_length`. We
//!     only *estimate* tokens ([`est_tokens`]) for two non-headline jobs: the no-recursion guard below, and
//!     the optimistic post-compress meter nudge — never for the percentage the user reads.
//!   * **"Meaningfully larger model available"** = a catalogue model whose window is at least
//!     [`UPGRADE_MULTIPLE`]× the current one, and only while the current window is below
//!     [`WINDOW_CAP_FOR_UPGRADE`] (the card's "suppress Upgrade on a 1M+ window"). Deterministic, so the
//!     UI's show/hide is not a guess.
//!   * **Don't recursively compress.** Once the rolling summary alone exceeds [`SUMMARY_CAP_FRAC`] of the
//!     window, folding more raw in can't meaningfully reclaim and would push toward summarising the summary
//!     (lossy). At that point [`compress_plan`] reports *unavailable*, routing the alert to Continue/Upgrade
//!     only — "the summary is getting big" becomes just another flavour of "context is filling up".

use serde::Serialize;

/// Fraction of the window at which the chat surfaces the compress/continue/upgrade alert.
pub const ALERT_FRAC: f64 = 0.80;
/// A model is "meaningfully larger" iff its window is at least this multiple of the current one.
pub const UPGRADE_MULTIPLE: i64 = 2;
/// At or above this window the Upgrade option is suppressed entirely (the card's 1M+ rule).
pub const WINDOW_CAP_FOR_UPGRADE: i64 = 1_000_000;
/// Compress always keeps at least this many most-recent turn-pairs verbatim (never folds the live tail).
pub const COMPRESS_FLOOR_PAIRS: usize = 3;
/// The no-recursion guard: a summary occupying this fraction of the window (or more) is "too big to
/// compress further" — offer Upgrade instead of degrading the summary.
pub const SUMMARY_CAP_FRAC: f64 = 0.5;

/// A larger-context model the user could switch to, for the Upgrade option. Minimal — the picker UI already
/// has the full catalogue; this is just the shortlist the alert surfaces.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub context_length: i64,
}

/// Whether Compress can meaningfully reclaim context right now, and if so how many oldest pairs would fold.
/// A flat struct (not a tagged enum) so the frontend reads it as a plain object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompressDecision {
    pub available: bool,
    /// How many oldest un-summarised pairs would be folded (0 when unavailable).
    pub foldable: usize,
    /// Why Compress is unavailable, for the UI to explain the Continue/Upgrade-only alert.
    pub reason: Option<String>,
}

impl CompressDecision {
    fn available(foldable: usize) -> Self {
        Self {
            available: true,
            foldable,
            reason: None,
        }
    }
    fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            foldable: 0,
            reason: Some(reason.to_string()),
        }
    }
}

/// A cheap token estimate (~4 chars/token). Used ONLY for the no-recursion guard and the optimistic
/// post-compress meter nudge — never for the headline meter, which is the measured `prompt_tokens`.
pub fn est_tokens(text: &str) -> i64 {
    let chars = text.chars().count() as i64;
    (chars + 3) / 4
}

/// The meter's reading: measured prompt tokens over the selected model's window, in `[0, ∞)` (the UI caps the
/// bar at 100%). `None` when either input is unknown — a custom model with no catalogued window, or a
/// conversation with no reply yet — so the meter honestly shows "unknown" rather than a fabricated number.
pub fn usage_percent(used: Option<i64>, window: Option<i64>) -> Option<f64> {
    match (used, window) {
        (Some(u), Some(w)) if w > 0 && u >= 0 => Some(u as f64 / w as f64),
        _ => None,
    }
}

/// Whether the meter is in alert territory. `None` percent (unknown window/usage) is never an alert.
pub fn is_alerting(percent: Option<f64>) -> bool {
    percent.is_some_and(|p| p >= ALERT_FRAC)
}

/// The larger-context models worth offering, given the current window and the catalogue. Empty when the
/// current window is already ≥ [`WINDOW_CAP_FOR_UPGRADE`] (suppress Upgrade) or nothing is ≥ the ×multiple
/// floor. Nearest-larger first (sorted ascending by window), capped at a short list.
pub fn upgrade_options(current_window: i64, catalogue: &[ModelOption]) -> Vec<ModelOption> {
    if current_window >= WINDOW_CAP_FOR_UPGRADE {
        return Vec::new();
    }
    let floor = current_window.saturating_mul(UPGRADE_MULTIPLE);
    let mut larger: Vec<ModelOption> = catalogue
        .iter()
        .filter(|m| m.context_length >= floor)
        .cloned()
        .collect();
    larger.sort_by_key(|m| m.context_length);
    larger.dedup_by(|a, b| a.id == b.id);
    larger.truncate(3);
    larger
}

/// Whether Compress can meaningfully reclaim context, and how much would fold. `window` may be unknown (a
/// custom model) — then we can still compress on the foldable-pairs test, we just can't apply the
/// summary-size guard. The two unavailable reasons map to the card's two fall-throughs: nothing left to fold
/// (the tail is already at the floor), and the summary is too big to compress further (route to Upgrade).
pub fn compress_plan(
    uncovered_pairs: usize,
    summary_tokens_est: i64,
    window: Option<i64>,
) -> CompressDecision {
    if let Some(w) = window {
        if w > 0 && summary_tokens_est as f64 >= w as f64 * SUMMARY_CAP_FRAC {
            return CompressDecision::unavailable(
                "the running summary is already large — switching to a bigger-context model keeps more detail than compressing further",
            );
        }
    }
    let foldable = uncovered_pairs.saturating_sub(COMPRESS_FLOOR_PAIRS);
    if foldable == 0 {
        return CompressDecision::unavailable("the recent turns are already minimal");
    }
    CompressDecision::available(foldable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(id: &str, window: i64) -> ModelOption {
        ModelOption {
            id: id.into(),
            name: id.into(),
            context_length: window,
        }
    }

    #[test]
    fn usage_percent_known_unknown_and_boundary() {
        assert_eq!(usage_percent(Some(160_000), Some(200_000)), Some(0.8));
        assert_eq!(usage_percent(None, Some(200_000)), None, "no reply yet");
        assert_eq!(
            usage_percent(Some(1), None),
            None,
            "custom model, no window"
        );
        assert_eq!(usage_percent(Some(1), Some(0)), None, "guard div-by-zero");
        // Just under vs at the alert line.
        assert!(!is_alerting(usage_percent(Some(159_999), Some(200_000))));
        assert!(is_alerting(usage_percent(Some(160_000), Some(200_000))));
        assert!(!is_alerting(None), "unknown is never an alert");
    }

    #[test]
    fn upgrade_requires_double_and_hides_at_one_million() {
        let cat = vec![
            opt("small", 128_000),
            opt("same", 200_000),
            opt("bigger", 400_000),
            opt("huge", 1_000_000),
        ];
        // On a 200k window: only ≥400k qualifies (≥2×); 'same' (200k) and 'small' do not.
        let up = upgrade_options(200_000, &cat);
        assert_eq!(
            up.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["bigger", "huge"],
            "≥2× only, nearest-larger first"
        );
        // Already on a 1M window ⇒ Upgrade suppressed entirely, even though larger models exist.
        assert!(
            upgrade_options(1_000_000, &cat).is_empty(),
            "1M+ suppresses Upgrade"
        );
        // Nothing ≥2× ⇒ empty.
        assert!(upgrade_options(600_000, &cat).is_empty());
    }

    #[test]
    fn upgrade_caps_the_shortlist_and_orders_by_window() {
        let cat = vec![
            opt("a", 500_000),
            opt("b", 800_000),
            opt("c", 600_000),
            opt("d", 900_000),
            opt("e", 700_000),
        ];
        let up = upgrade_options(200_000, &cat); // floor 400k: all qualify
        assert_eq!(up.len(), 3, "shortlist capped at 3");
        assert_eq!(
            up.iter().map(|m| m.context_length).collect::<Vec<_>>(),
            vec![500_000, 600_000, 700_000],
            "nearest-larger first"
        );
    }

    #[test]
    fn compress_available_when_there_is_a_foldable_batch() {
        // 10 uncovered pairs, small summary, 200k window: 10 - FLOOR(3) = 7 foldable.
        let d = compress_plan(10, est_tokens("- one short line"), Some(200_000));
        assert_eq!(
            d,
            CompressDecision::available(7),
            "folds all but the verbatim floor"
        );
    }

    #[test]
    fn compress_unavailable_at_the_floor_and_when_summary_is_huge() {
        // At the floor: nothing left to fold.
        assert!(!compress_plan(COMPRESS_FLOOR_PAIRS, 10, Some(200_000)).available);
        assert!(!compress_plan(2, 10, Some(200_000)).available);
        // No-recursion guard: a summary occupying ≥ half the window is "too big to compress further".
        let huge_summary_est = 200_000 / 4 * 2; // ~half the 200k window in estimated tokens
        let d = compress_plan(50, huge_summary_est, Some(200_000));
        assert!(!d.available, "summary too big ⇒ route to Upgrade");
        assert!(d.reason.unwrap().contains("bigger-context model"));
        // Unknown window: the size guard can't apply, but foldable pairs still allow compress.
        assert!(compress_plan(10, 9_999_999, None).available);
    }
}
