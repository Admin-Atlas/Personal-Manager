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

/// Tokens held back from the window for the model's own reply, when deciding whether a background
/// prompt fits. The window covers prompt AND completion; filling it with prompt leaves the reply
/// nowhere to go, and a front-truncating server resolves that by discarding prompt.
///
/// Sized to PM's largest background reply rather than its typical one: a filing batch returns an
/// index-matched array of five classifications, the re-tag vocabulary call returns up to
/// [`crate::retag::VOCAB_CEILING`] labels. Generous on purpose — this reserve is cheap and the
/// failure it prevents is silent.
pub const REPLY_RESERVE_TOKENS: i64 = 1024;

/// Per-message overhead a chat template adds around each message (role markers, turn delimiters,
/// the assistant priming at the end). Small, constant, and it must not be forgotten: a prompt sized
/// to exactly the window overflows by the scaffolding alone.
pub const PER_MESSAGE_OVERHEAD_TOKENS: i64 = 4;

/// A deliberately PESSIMISTIC token count, for deciding whether a prompt will FIT.
///
/// [`est_tokens`]'s flat ~4 chars/token is a fair average for English prose and the wrong answer for
/// what background prompts are actually made of. Measured across PM's own prompt shapes: English
/// prose ~3.98 chars/token, the JSON-and-braces its contracts ask for ~2.77, CJK ~1.69. So the
/// average under-counts exactly the content most likely to overflow.
///
/// An under-count here is not a rounding error. It is PM deciding a prompt fits, sending it, and a
/// server running with `--context-shift` discarding the FRONT of it — the system message, which is
/// where the output contract and the untrusted-data guard live — then answering 200 with a
/// `finish_reason` of `stop` and a `prompt_tokens` measured AFTER the cut. Nothing downstream can
/// see that happened, which is why this has to be wrong in the safe direction.
///
/// So it counts by class rather than by a single rate: ASCII letters and spaces at 3.2 chars/token,
/// every other ASCII character (digits, quotes, braces, commas, newlines — what JSON is made of) at
/// 1.2, and each non-ASCII character as a token of its own, plus a second for anything above the BMP
/// where one emoji really can cost several. Every rate sits below the measured one, so the answer is
/// an over-count on all three shapes — by roughly a third on prose and half on JSON, which is the
/// margin worth paying to never send a prompt that gets silently cut.
///
/// Used ONLY for fit decisions. The meter the user reads stays the measured `prompt_tokens`.
pub fn est_tokens_upper(text: &str) -> i64 {
    let (mut wordish, mut dense, mut wide, mut astral) = (0i64, 0i64, 0i64, 0i64);
    for c in text.chars() {
        if c.is_ascii_alphabetic() || c == ' ' {
            wordish += 1;
        } else if c.is_ascii() {
            dense += 1;
        } else {
            wide += 1;
            if c.len_utf8() == 4 {
                astral += 1;
            }
        }
    }
    let ceil_div = |n: i64, d: i64| (n + d - 1) / d;
    ceil_div(wordish * 10, 32) + ceil_div(dense * 10, 12) + wide + astral
}

/// [`est_tokens_upper`] over a whole message list, paying [`PER_MESSAGE_OVERHEAD_TOKENS`] per
/// message. Takes the contents as strings rather than a message type so this module keeps no
/// dependency on the wire structs.
pub fn est_messages_tokens_upper<'a, I: IntoIterator<Item = &'a str>>(contents: I) -> i64 {
    contents
        .into_iter()
        .map(|c| est_tokens_upper(c) + PER_MESSAGE_OVERHEAD_TOKENS)
        .sum()
}

/// How many prompt tokens may be sent to a server serving `window` tokens. `None` when the window is
/// unknown (nothing to size against) or so small that the reply reserve alone exhausts it — a caller
/// that gets `None` sends what it would have sent anyway, which is the pre-existing behaviour.
pub fn prompt_ceiling(window: Option<i64>) -> Option<i64> {
    let w = window?;
    let ceiling = w - REPLY_RESERVE_TOKENS;
    (ceiling > 0).then_some(ceiling)
}

/// The largest batch size in `1..=max` whose prompt fits under `ceiling`, found by halving.
///
/// `cost(n)` must return the token size of the prompt this caller would build for `n` items, and it
/// must not shrink as `n` grows (every batcher here appends, so it doesn't). Callers pass a closure
/// that BUILDS the real prompt and measures it with [`est_messages_tokens_upper`], rather than
/// estimating its parts — a parallel estimate is a second copy of the prompt's shape and would drift
/// away from the builder the first time anyone edits one. `cost` is called ~log2(max) times, and a
/// prompt builder is a few string joins, so that is cheap next to the model call it precedes.
///
/// **Never returns zero.** `None` (no ceiling — a cloud route, or a local window PM has not learned
/// yet) returns `max` unchanged, which is the pre-existing behaviour. A single item too large for
/// any ceiling comes back as a batch of one, so the gateway refuses it by name instead of the caller
/// reading an empty batch as "nothing to do" and skipping it forever.
pub fn largest_fitting(
    max: usize,
    ceiling: Option<i64>,
    mut cost: impl FnMut(usize) -> i64,
) -> usize {
    let Some(ceiling) = ceiling else { return max };
    if max <= 1 {
        return max;
    }
    if cost(max) <= ceiling {
        return max;
    }
    // `lo` is the largest size known (or assumed) to fit; `hi` is the smallest known not to.
    let (mut lo, mut hi) = (1usize, max);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if cost(mid) <= ceiling {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
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
    fn upper_estimate_never_undercounts_the_shapes_that_overflow() {
        // The three measured shapes from `est_tokens_upper`'s doc comment, each sized so the TRUE
        // token count is known within a few percent. The upper bound must sit ABOVE all of them —
        // an estimate that lands under is the bug this function exists to prevent.
        let english = "the quick brown fox jumps over the lazy dog. ".repeat(40); // 1800 chars
        let true_english = english.chars().count() as i64 * 100 / 398; // ≈3.98 chars/token
        assert!(
            est_tokens_upper(&english) > true_english,
            "prose: {} must exceed {true_english}",
            est_tokens_upper(&english)
        );

        let json = r#"{"index":1,"tags":["invoice","finance"]},"#.repeat(40);
        let true_json = json.chars().count() as i64 * 100 / 277; // ≈2.77 chars/token
        assert!(
            est_tokens_upper(&json) > true_json,
            "json: {} must exceed {true_json}",
            est_tokens_upper(&json)
        );

        let cjk =
            "\u{6211}\u{4eec}\u{9700}\u{8981}\u{8ba8}\u{8bba}\u{8fd9}\u{4e2a}\u{9879}\u{76ee}"
                .repeat(40);
        let true_cjk = cjk.chars().count() as i64 * 100 / 169; // ≈1.69 chars/token
        assert!(
            est_tokens_upper(&cjk) > true_cjk,
            "cjk: {} must exceed {true_cjk}",
            est_tokens_upper(&cjk)
        );

        // Pessimistic, but not uselessly so: an estimate that doubles the truth would refuse
        // prompts that fit and starve every background job on a modest window. Both directions are
        // pinned so a later correction to either rate has to stay inside the band.
        assert!(
            est_tokens_upper(&english) < true_english * 2,
            "prose margin runaway"
        );
        assert!(
            est_tokens_upper(&json) < true_json * 2,
            "json margin runaway"
        );
        assert!(est_tokens_upper(&cjk) < true_cjk * 2, "cjk margin runaway");

        // And it is strictly more pessimistic than the meter's average, which is the whole point.
        assert!(est_tokens_upper(&english) > est_tokens(&english));
        assert_eq!(est_tokens_upper(""), 0);
    }

    #[test]
    fn message_overhead_is_paid_per_message() {
        let one = est_messages_tokens_upper(["abcdef"]);
        let two = est_messages_tokens_upper(["abc", "def"]);
        assert_eq!(one, 2 + PER_MESSAGE_OVERHEAD_TOKENS);
        assert_eq!(
            two,
            one + PER_MESSAGE_OVERHEAD_TOKENS,
            "splitting the same text across two messages costs one more overhead"
        );
    }

    #[test]
    fn prompt_ceiling_holds_back_the_reply_and_gives_up_on_a_tiny_window() {
        assert_eq!(
            prompt_ceiling(Some(4096)),
            Some(4096 - REPLY_RESERVE_TOKENS)
        );
        assert_eq!(prompt_ceiling(None), None, "unknown window ⇒ no ceiling");
        assert_eq!(
            prompt_ceiling(Some(REPLY_RESERVE_TOKENS)),
            None,
            "a window the reply alone fills is not a budget"
        );
        assert_eq!(prompt_ceiling(Some(REPLY_RESERVE_TOKENS + 1)), Some(1));
    }

    #[test]
    fn largest_fitting_finds_the_boundary_and_honours_no_ceiling() {
        // A prompt costing 100 per item on top of a 200-token system message: 8 items = 1000, which
        // is the last size that fits a 1000 ceiling; 9 would be 1100.
        let cost = |n: usize| 200 + 100 * n as i64;
        assert_eq!(largest_fitting(20, Some(1000), cost), 8);
        assert_eq!(
            largest_fitting(5, Some(1000), cost),
            5,
            "the cap wins when it fits"
        );
        assert_eq!(
            largest_fitting(20, None, cost),
            20,
            "no ceiling ⇒ no shrinking"
        );
    }

    #[test]
    fn largest_fitting_never_returns_an_empty_batch() {
        // One item that cannot fit ANY ceiling still comes back as a batch of one. Returning 0 would
        // read to the caller as "nothing to do" and skip that document forever; sending it lets the
        // gateway refuse it by name, which is something the user can act on.
        assert_eq!(largest_fitting(5, Some(10), |n| 9_999 * n as i64), 1);
        assert_eq!(largest_fitting(1, Some(10), |_| 9_999), 1);
    }

    #[test]
    fn largest_fitting_calls_cost_a_logarithmic_number_of_times() {
        // The guard against someone "simplifying" this into a linear scan: `cost` builds a real
        // prompt, so a linear walk over a 400-title sample would build 400 of them.
        let mut calls = 0usize;
        let n = largest_fitting(1024, Some(1000), |n| {
            calls += 1;
            n as i64
        });
        assert_eq!(n, 1000);
        assert!(calls <= 12, "halving search, not a scan (was {calls})");
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
