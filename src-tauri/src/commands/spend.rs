// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The cost logger and the spend summary it feeds.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::llm_gateway;
use crate::{cost, openrouter, AppState};

// --- cost logger (spec §11.2 / §17.1) ---

/// Spend for one model over a window. `cost_usd` is `None` when the model isn't in
/// the price cache yet — surfaced as "—", never an understated $0.
#[derive(Serialize)]
pub struct ModelSpend {
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Option<f64>,
}

/// The Settings "Usage & cost" payload: per-model spend over two windows + totals,
/// plus when the cached pricing was last refreshed.
#[derive(Serialize)]
pub struct CostSummary {
    pub last_30d: Vec<ModelSpend>,
    pub all_time: Vec<ModelSpend>,
    pub total_30d_usd: Option<f64>,
    pub total_all_time_usd: Option<f64>,
    pub pricing_updated_at: Option<String>,
}

/// Per-model spend (trailing 30 days + all time) joined against the cached OpenRouter
/// prices. CHECK-ON-READ: if the price cache is empty or older than a day, refresh it
/// from the public catalogue first (no key, no model call, no scheduler — mirrors the
/// briefing's staleness rule). Read-mostly; safe on every Settings open.
#[tauri::command]
pub async fn cost_summary(app: AppHandle) -> Result<CostSummary> {
    // Best-effort refresh: if it fails (offline, etc.) still return the summary —
    // token counts come from the local log and need no network; only the priced
    // costs fall back to "unknown". The explicit "Refresh prices" button surfaces
    // the error instead.
    let _ = ensure_pricing_fresh(&app).await;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    build_cost_summary(&conn)
}

/// Force a re-pull of OpenRouter's public pricing into the cache, then return the
/// refreshed summary (the Settings "Refresh prices" action).
#[tauri::command]
pub async fn refresh_pricing(app: AppHandle) -> Result<CostSummary> {
    refresh_pricing_now(&app).await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    build_cost_summary(&conn)
}

/// Reconstruct a view of the catalogue from the daily price/signal cache (`model_pricing`,
/// extended in migration v8). Reading from the cache — not a live fetch — is what lets the
/// chat context meter work offline. Only the **latest refresh batch** is in scope
/// (`fetched_at = MAX(fetched_at)`): a model that has left OpenRouter keeps an older
/// timestamp and is excluded. (The cost-summary join reads `model_pricing` unfiltered, so
/// historical spend on a now-removed model is still priced.)
///
/// Note this cache is **not** ZDR-filtered — that filter lives in `openrouter::list_models`,
/// on the picker. This feeds the context meter, which only needs a window size for a model
/// the user already has selected.
pub(super) fn cached_catalogue(conn: &Connection) -> Result<Vec<openrouter::ModelDetail>> {
    let mut stmt = conn.prepare(
        "SELECT model, COALESCE(name, ''), context_length, prompt_price, completion_price, \
                cache_read_price, supported_parameters, input_modalities, intelligence_index \
         FROM model_pricing \
         WHERE fetched_at = (SELECT MAX(fetched_at) FROM model_pricing)",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let context_length: Option<i64> = r.get(2)?;
            let supported: Option<String> = r.get(6)?;
            let modalities: Option<String> = r.get(7)?;
            Ok(openrouter::ModelDetail {
                id: r.get(0)?,
                name: r.get(1)?,
                description: String::new(),
                context_length: context_length.map(|v| v as u64),
                prompt_price: r.get(3)?,
                completion_price: r.get(4)?,
                cache_read_price: r.get(5)?,
                input_modalities: modalities
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                supported_parameters: supported
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                intelligence_index: r.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Append a `usage_log` row — best-effort: cost logging must never fail a model call,
/// so errors are swallowed. `model = None` is allowed (an unreported served model). `meta` tags the
/// row with how it was served (provider / latency / fallback reason, migration v37) so the Usage &
/// cost table and the Local AI tab can tell local from cloud spend. `pub(crate)` so the chat
/// housekeeping modules (summary / title / prefs) route their rows through it too.
pub(crate) fn log_usage(
    conn: &Connection,
    kind: &str,
    model: Option<&str>,
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
) {
    // One row, tagged with how it was served (provider / latency / fallback, the v37 columns).
    let fallback = meta.fallback.as_ref().map(|f| f.as_log_str());
    // Best-effort: accounting must NEVER fail a model call, so we don't propagate the error. But we do
    // NOT swallow it silently — a rejected insert here almost always means a schema mismatch (a store
    // missing the v37 columns), the exact class of bug v36 hid for months by pairing a rejecting CHECK
    // with a silent `let _ =`. Surface it at once so a mismatch shows up in seconds, not as months of
    // missing cost data.
    if let Err(e) = conn.execute(
        "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, \
         provider, latency_ms, fallback_reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            model,
            kind,
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.cost,
            meta.provider.as_str(),
            meta.latency_ms as i64,
            fallback
        ],
    ) {
        eprintln!(
            "usage_log: could not record a '{kind}' usage row — cost/usage accounting will be \
             incomplete ({e})"
        );
    }
}

/// Write collected background usage rows under one short lock (best-effort), each attributed to its
/// served model (or the requested primary when none was reported) and tagged with how it was served.
pub(super) fn log_background_usage(
    app: &AppHandle,
    models: &[String],
    rows: &[(Option<String>, openrouter::Usage, llm_gateway::CallMeta)],
) {
    if rows.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let Ok(conn) = state.conn() else { return };
    for (served, usage, meta) in rows {
        let model = served
            .as_deref()
            .or_else(|| models.first().map(String::as_str));
        log_usage(&conn, "background", model, usage, meta);
    }
}

/// Refresh the cached pricing when it's stale (check-on-read). Resolves staleness
/// under a short lock, then does the network fetch + upsert without holding it (rule #4).
async fn ensure_pricing_fresh(app: &AppHandle) -> Result<()> {
    let stale = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let hours: Option<f64> = conn
            .query_row(
                "SELECT (julianday('now') - julianday(replace(MAX(fetched_at),'Z',''))) * 24.0 FROM model_pricing",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        cost::pricing_is_stale(hours)
    };
    if stale {
        refresh_pricing_now(app).await?;
    }
    Ok(())
}

/// Pull the public OpenRouter catalogue (no key) and upsert every model's prices into the cache,
/// which the cost logger reads. Also caches the cache-read rate, context length, supported params
/// and capability indices: those fed the model recommender, DELETED in v3.18.0-alpha (#369), and
/// are write-only today — migration v8's columns are append-only and the dev inspector reads them.
/// Never holds the DB lock across the network call (rule #4).
async fn refresh_pricing_now(app: &AppHandle) -> Result<()> {
    let models = openrouter::fetch_catalogue().await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let tx = conn.unchecked_transaction()?;
    // One timestamp for the whole batch, so every model in this pull shares an identical
    // `fetched_at`. That lets the recommender read only the latest batch (a model that left
    // OpenRouter keeps an older timestamp and drops out of candidacy — see `cached_catalogue`),
    // and keeps the staleness check exact.
    let fetched_at: String =
        tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?;
    for m in &models {
        let supported =
            serde_json::to_string(&m.supported_parameters).unwrap_or_else(|_| "[]".into());
        let modalities = serde_json::to_string(&m.input_modalities).unwrap_or_else(|_| "[]".into());
        tx.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, name, context_length, \
                cache_read_price, supported_parameters, input_modalities, intelligence_index, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(model) DO UPDATE SET \
                prompt_price = ?2, completion_price = ?3, name = ?4, context_length = ?5, \
                cache_read_price = ?6, supported_parameters = ?7, input_modalities = ?8, \
                intelligence_index = ?9, fetched_at = ?10",
            params![
                m.id,
                m.prompt_price,
                m.completion_price,
                m.name,
                m.context_length.map(|v| v as i64),
                m.cache_read_price,
                supported,
                modalities,
                m.intelligence_index,
                fetched_at,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Assemble the cost summary from `usage_log` × the cached `model_pricing`.
fn build_cost_summary(conn: &Connection) -> Result<CostSummary> {
    let last_30d = spend_rows(conn, true)?;
    let all_time = spend_rows(conn, false)?;
    let total_30d_usd = total_cost(&last_30d);
    let total_all_time_usd = total_cost(&all_time);
    let pricing_updated_at: Option<String> = conn
        .query_row("SELECT MAX(fetched_at) FROM model_pricing", [], |r| {
            r.get(0)
        })
        .ok()
        .flatten();
    Ok(CostSummary {
        last_30d,
        all_time,
        total_30d_usd,
        total_all_time_usd,
        pricing_updated_at,
    })
}

/// Per-model token sums + request counts (optionally only the last 30 days), priced
/// from the cache; ordered by request count desc. Rows with a NULL model are excluded.
fn spend_rows(conn: &Connection, last_30d: bool) -> Result<Vec<ModelSpend>> {
    let window = if last_30d {
        "AND u.created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days')"
    } else {
        ""
    };
    // Split the token sums by whether the row carried OpenRouter's reported cost, so cost is
    // computed ROW-ADDITIVELY: real reported spend (`SUM(cost_usd)` over the rows that have it) plus
    // a tokens × cached-price estimate for ONLY the rows that don't. The earlier all-or-nothing rule
    // abandoned the whole group's real cost the moment a single pre-feature row (NULL `cost_usd`) was
    // present — so a model with both old and new calls fell back to the estimate and went blank when
    // it wasn't in the price cache. Additive costing keeps the known real spend visible regardless.
    let sql = format!(
        "SELECT u.model, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price, \
                SUM(u.cost_usd), COUNT(u.cost_usd), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.prompt_tokens     ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.completion_tokens ELSE 0 END), 0) \
         FROM usage_log u LEFT JOIN model_pricing p ON p.model = u.model \
         WHERE u.model IS NOT NULL {window} \
         GROUP BY u.model \
         ORDER BY COUNT(*) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map([], |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let completion_tokens: i64 = r.get(2)?;
            let request_count: i64 = r.get(3)?;
            let prompt_price: Option<f64> = r.get(4)?;
            let completion_price: Option<f64> = r.get(5)?;
            let reported_cost: Option<f64> = r.get(6)?; // SUM(cost_usd); NULL when no call reported one
            let reported_count: i64 = r.get(7)?; // calls in this group that reported an actual cost
            let est_prompt_tokens: i64 = r.get(8)?; // tokens from ONLY the rows lacking a reported cost
            let est_completion_tokens: i64 = r.get(9)?;
            // Estimate the unreported rows (tokens × cached price); `None` when that model isn't
            // priced. Some(0.0) when every row reported an actual cost (nothing left to estimate).
            let estimate = if request_count - reported_count > 0 {
                cost::call_cost(
                    Some(est_prompt_tokens),
                    Some(est_completion_tokens),
                    prompt_price,
                    completion_price,
                )
            } else {
                Some(0.0)
            };
            // Real reported spend is always honoured; the estimate only fills in the rows that
            // lacked a reported cost. "Unknown" (`None`) survives only when NOTHING is known — no
            // reported cost and the leftover rows are unpriced — never just because of an old row.
            let cost_usd = match (reported_cost, estimate) {
                (Some(actual), Some(est)) => Some(actual + est),
                (Some(actual), None) => Some(actual), // real cost known; unpriced remainder omitted
                (None, Some(est)) => Some(est),
                (None, None) => None,
            };
            Ok(ModelSpend {
                model: r.get(0)?,
                prompt_tokens,
                completion_tokens,
                request_count,
                cost_usd,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // Rank by cost (most expensive first); unpriced models (unknown cost) sort last,
    // then by request count — so the breakdown reads as a spend ranking.
    rows.sort_by(|a, b| {
        let ak = a.cost_usd.unwrap_or(f64::NEG_INFINITY);
        let bk = b.cost_usd.unwrap_or(f64::NEG_INFINITY);
        bk.partial_cmp(&ak)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.request_count.cmp(&a.request_count))
    });
    Ok(rows)
}

/// Total spend across rows: `Some(0)` with no usage, `None` when there's usage but no
/// model is priced yet, else the sum of the priced rows (unpriced models shown "—").
fn total_cost(rows: &[ModelSpend]) -> Option<f64> {
    if rows.is_empty() {
        return Some(0.0);
    }
    let known: Vec<f64> = rows.iter().filter_map(|r| r.cost_usd).collect();
    if known.is_empty() {
        return None;
    }
    Some(known.iter().sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::shared::temp_db;

    /// Cost is ROW-ADDITIVE: a model's real reported spend always shows, with an estimate filling in
    /// only the rows that lacked one. The earlier all-or-nothing rule went blank for any model that
    /// mixed a reported call with an older unreported one and wasn't in the price cache — this pins
    /// the fix.
    #[test]
    fn spend_rows_adds_real_cost_and_estimates_only_unreported_rows() {
        let (_dir, conn) = temp_db();
        let priced_now = |model: &str| {
            conn.execute(
                "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                params![model, 3e-6_f64, 15e-6_f64],
            )
            .unwrap();
        };
        let log = |model: &str, pt: i64, ct: i64, cost: Option<f64>| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd) \
                 VALUES (?1, 'chat', ?2, ?3, ?4)",
                params![model, pt, ct, cost],
            )
            .unwrap();
        };

        // Priced model, mixed rows: a reported $0.05 call + an older unreported one (1000/500 tokens).
        priced_now("vendor/priced");
        log("vendor/priced", 2000, 1000, Some(0.05));
        log("vendor/priced", 1000, 500, None);

        // Unpriced model, mixed rows — the regression case: a reported $0.02 call + an unreported one.
        log("vendor/unpriced", 100, 100, Some(0.02));
        log("vendor/unpriced", 100, 100, None);

        // Unpriced model, only an old unreported row — genuinely unknown.
        log("vendor/unknown", 100, 100, None);

        let rows = spend_rows(&conn, false).unwrap();
        let cost_of = |m: &str| rows.iter().find(|r| r.model == m).unwrap().cost_usd;

        // Reported 0.05 + estimate(1000·3e-6 + 500·15e-6 = 0.0105) = 0.0605.
        assert!((cost_of("vendor/priced").unwrap() - 0.0605).abs() < 1e-9);
        // The fix: the real reported cost shows as a floor even though the model isn't priced —
        // never blank. The unpriced unreported row is omitted (unknown), not understated to $0.
        assert!((cost_of("vendor/unpriced").unwrap() - 0.02).abs() < 1e-9);
        // Nothing known at all → still "unknown".
        assert!(cost_of("vendor/unknown").is_none());

        // And the grand total is the sum of the known rows (0.0605 + 0.02), not blank.
        assert!((total_cost(&rows).unwrap() - 0.0805).abs() < 1e-9);
    }
}
