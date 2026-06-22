// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure cost maths for the usage logger (spec §11.2 / §17.1) — the deterministic
//! pieces of "a live cost tracker synced to OpenRouter pricing". No DB, no network,
//! no `.await`: `commands.rs` gathers `usage_log` rows and the cached `model_pricing`
//! under a scoped lock and feeds plain values in here, so these rules unit-test in
//! isolation (in the spirit of [`crate::retrieval::decay_factor`] /
//! [`crate::projects::derive_status`]).
//!
//! Cost is **never stored** — it's derived from token counts × the cached per-token
//! price at read time, so a later price correction reprices history, and a model
//! that isn't in the price cache yet shows as "unknown" rather than an understated $0.

/// Cached pricing older than this many hours is re-pulled on read (check-on-read,
/// mirroring [`crate::briefing`]'s staleness rule — no scheduler, no model call).
pub const PRICING_STALE_HOURS: f64 = 24.0;

/// USD cost of one model call from its token counts and the model's per-token prices.
/// `None` when either price is unknown (the model isn't priced in the cache yet) —
/// the caller renders that as "unknown", distinct from a real $0 (a free model).
/// Missing token counts count as 0; negative counts (shouldn't happen) clamp to 0.
pub fn call_cost(
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    prompt_price: Option<f64>,
    completion_price: Option<f64>,
) -> Option<f64> {
    let pp = prompt_price?;
    let cp = completion_price?;
    let pt = prompt_tokens.unwrap_or(0).max(0) as f64;
    let ct = completion_tokens.unwrap_or(0).max(0) as f64;
    Some(pt * pp + ct * cp)
}

/// True when pricing has never been fetched (`None`) or its newest fetch is older
/// than [`PRICING_STALE_HOURS`]. `hours_since_fetch` is computed in SQL by the caller
/// (the `julianday` idiom), so this stays pure.
pub fn pricing_is_stale(hours_since_fetch: Option<f64>) -> bool {
    hours_since_fetch.is_none_or(|h| h >= PRICING_STALE_HOURS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_cost_multiplies_known_prices_and_is_none_when_unknown() {
        // 1000 prompt @ $3/M + 500 completion @ $15/M (per-token 3e-6 / 15e-6).
        let c = call_cost(Some(1000), Some(500), Some(3e-6), Some(15e-6)).unwrap();
        assert!((c - (1000.0 * 3e-6 + 500.0 * 15e-6)).abs() < 1e-12);
        // A missing price → unknown (None), never silently zero.
        assert!(call_cost(Some(1000), Some(500), None, Some(15e-6)).is_none());
        // A genuinely free model ($0 prices) is a real 0.0, distinct from unknown.
        assert_eq!(call_cost(Some(1000), Some(500), Some(0.0), Some(0.0)), Some(0.0));
        // Missing token counts count as 0.
        assert_eq!(call_cost(None, None, Some(3e-6), Some(15e-6)), Some(0.0));
    }

    #[test]
    fn pricing_staleness_boundary() {
        assert!(pricing_is_stale(None)); // never fetched
        assert!(pricing_is_stale(Some(24.0))); // exactly the cutoff reads as stale
        assert!(pricing_is_stale(Some(48.5)));
        assert!(!pricing_is_stale(Some(1.0)));
        assert!(!pricing_is_stale(Some(23.9)));
    }
}
