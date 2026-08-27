// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streaming chat against OpenRouter (spec §6 — one key, any model, swappable).
//! The API key is read from the keychain on the Rust side and never reaches the
//! webview.

use std::collections::HashSet;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MODELS_ENDPOINT: &str = "https://openrouter.ai/api/v1/models";
/// The authoritative list of Zero-Data-Retention endpoints, keyed by `model_id`. Public, like
/// the catalogue. This is what lets PM filter the picker to models it can actually serve
/// rather than only failing closed at request time.
const ZDR_ENDPOINT: &str = "https://openrouter.ai/api/v1/endpoints/zdr";

/// One shared HTTP client for every OpenRouter call (F-16). A fresh `Client::builder().build()` per
/// request threw away the connection pool each time; a single `LazyLock` client reuses pooled TLS
/// connections across catalogue fetches, streamed chats, and background completions. It carries only
/// the two client-level bounds that suit all three sites — `connect_timeout` (establishing the socket)
/// and `read_timeout` (silence *between* chunks, reset on every byte received). The per-request TOTAL
/// deadline is deliberately NOT here: it differs per call (30 s catalogue, 120 s `complete`, and none
/// for a healthy stream — see `stream_chat`), so each site sets its own `.timeout()` on the request
/// builder. Building a client that only sets timeouts is effectively infallible, hence `expect`.
static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("reqwest client with static timeouts should build")
});

#[derive(Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Bound *and* single-line one untrusted field on its way into a prompt.
///
/// Two jobs, and they are the same job. The **clip** is sizing: a title is a filename, filenames run
/// to 255 characters, and a prompt built from four hundred of them is a prompt no local server will
/// hold. The **collapse** is what makes clipping correct: every prompt that carries these fields
/// builds them into a line-oriented block — `=== Document N ===`, `Title: …`, one title per line —
/// so an embedded CR/LF in a value forges a structural line the model reads as PM's own framing. A
/// document title is untrusted (an HTML `<title>`, PDF metadata, a filename in a shared Drive
/// folder) and nothing sanitises it between ingest and the prompt: `ingest::yaml_quote` collapses on
/// the way into the vault manifest, not on the way into `documents.title`.
///
/// Only for fields that are supposed to be ONE line. Document bodies are not, and are handled by the
/// untrusted-data framing instead.
pub fn clip_prompt_line(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Token usage OpenRouter reports for one completion. Any field may be absent
/// (a provider that doesn't report usage, or a degraded/early-terminated response),
/// so the cost logger stores NULLs and shows the spend as unknown rather than zero.
/// `cost` is OpenRouter's own usage-accounting figure (actual USD charged, net of any
/// prompt-cache discount) — preferred over the local tokens × price estimate when present.
#[derive(Clone, Copy, Default, Debug)]
pub struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cost: Option<f64>,
}

/// The result of a chat call: the assembled text, the model that actually served it
/// (may differ from the requested primary when an auto-switch fallback fires), and
/// token usage when reported. Shared by the streaming and non-streaming paths so the
/// cost logger can attribute spend to the model that actually ran.
pub struct Completion {
    pub text: String,
    pub model: Option<String>,
    pub usage: Usage,
    /// The model stopped because it hit its token ceiling (`finish_reason == "length"`), not because
    /// it had finished. The text is real and worth keeping — but it is not a finished answer, and
    /// this was previously dropped on the floor, so a reply that trailed off mid-sentence was stored
    /// as a complete turn and later quoted as one. Not an error: the caller marks the turn honest.
    pub truncated: bool,
}

impl Completion {
    /// The reply, but only when it can be treated as a FINISHED answer — `None` otherwise.
    ///
    /// There are two ways a reply is not one, and to every parser in this tree they look identical
    /// to a reply that simply had nothing to say:
    ///
    ///   * the model hit its token ceiling mid-sentence (`truncated`), so the JSON has no closing
    ///     bracket and the prose has no last clause; or
    ///   * it returned nothing at all.
    ///
    /// Both arrive as HTTP 200 with text attached. Every structured parser here is deliberately
    /// defensive — `parse_chat_preferences`, `parse_assignments`, `parse_vocabulary`, `parse_batch`
    /// all degrade to an empty result rather than raise — which is right against a reliable model
    /// and wrong against an unreliable one, because the callers then write that empty result down as
    /// a real finding and advance a cursor past the turns it came from. Two outcomes PM could not
    /// tell apart: "I read those forty turn-pairs and there was nothing worth learning", and "I lost
    /// the thread". Only the first should ever be recorded.
    ///
    /// This is the one check that separates them, and it belongs before the parse rather than
    /// inside it: the parsers are pure functions over a string and cannot see `finish_reason`.
    ///
    /// It does NOT catch a prompt cut at the FRONT — a `--context-shift` server answers `stop` and
    /// reports its `prompt_tokens` after the cut, so nothing in the response says it happened. That
    /// is what the pre-flight sizing in #797 is for; the two are complementary halves of "the reply
    /// PM got back is not the reply it asked for".
    pub fn usable_text(&self) -> Option<&str> {
        if self.truncated {
            return None;
        }
        let text = self.text.trim();
        (!text.is_empty()).then_some(text)
    }

    /// Why [`Self::usable_text`] said no, phrased for a log line or an error the user may see.
    /// `None` when the reply was usable.
    pub fn unusable_reason(&self) -> Option<&'static str> {
        if self.truncated {
            Some("the model's reply was cut off before it finished")
        } else if self.text.trim().is_empty() {
            Some("the model returned an empty reply")
        } else {
            None
        }
    }
}

/// The error a stream chunk reports, if any.
///
/// Mid-stream failures do NOT arrive as an HTTP status — the response was already 200 and some
/// tokens may already have been emitted. They arrive as a `data:` event carrying an `error` object.
/// Ignoring it meant the loop just ran out of chunks and returned the truncated text as a
/// SUCCESSFUL completion, which the caller then persisted as a complete assistant turn.
///
/// Pure, so the shape-matching — the part that can silently be wrong — is testable without a socket.
fn chunk_error(value: &serde_json::Value) -> Option<Error> {
    let err = value.get("error")?;
    let detail = err
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("the model stream reported an error");
    // The code is advisory here (the transport said 200). Routing through `request_error` means a
    // ZDR refusal that arrives mid-stream gets the same actionable copy as one at request time.
    let status = err
        .get("code")
        .and_then(|c| c.as_u64())
        .and_then(|c| u16::try_from(c).ok())
        .and_then(|c| reqwest::StatusCode::from_u16(c).ok())
        .unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
    Some(request_error(status, detail))
}

/// Whether a chunk says the model stopped because it ran out of room, rather than because it had
/// finished. Any other reason ("stop", "tool_calls", …) is a normal end.
fn is_length_stop(value: &serde_json::Value) -> bool {
    value["choices"][0]["finish_reason"].as_str() == Some("length")
}

/// Extract token usage from a response/chunk's `usage` object (absent fields → None). `cost` is
/// present only when we asked for usage accounting (`usage.include`) and the provider reports it.
fn parse_usage(value: &serde_json::Value) -> Usage {
    Usage {
        prompt_tokens: value["usage"]["prompt_tokens"].as_i64(),
        completion_tokens: value["usage"]["completion_tokens"].as_i64(),
        cost: value["usage"]["cost"].as_f64(),
    }
}

/// One model from OpenRouter's public catalogue, trimmed to what the Settings
/// picker needs: identity, pricing, context window, and input modalities (so the
/// UI can tag vision-capable models). Prices are USD **per token**; the frontend
/// renders them per-million.
#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_length: Option<u64>,
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub input_modalities: Vec<String>,
}

/// The fuller per-model record the daily price refresh caches. On top of pricing it carries
/// the prompt-cache read rate (PM reuses stable prompt prefixes, so cache reads dominate
/// effective cost), the model's `supported_parameters`, and the Artificial-Analysis
/// `intelligence_index` from the catalogue's `benchmarks` block — a *general-capability*
/// signal, NOT a faithfulness metric. Every field is optional because the catalogue is
/// sparse: most models carry no benchmarks, and some report no price.
///
/// The last three fields are write-only today (populated by the price refresh, surfaced in
/// the dev inspector) — they fed the model recommender, removed in v3.18.0-alpha. They are
/// kept because migration v8's columns are append-only and the dev inspector reads them.
#[derive(Clone, Debug, Default)]
pub struct ModelDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_length: Option<u64>,
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub cache_read_price: Option<f64>,
    pub input_modalities: Vec<String>,
    pub supported_parameters: Vec<String>,
    pub intelligence_index: Option<f64>,
}

/// Fetch the full OpenRouter model catalogue as the richer [`ModelDetail`] the price cache
/// needs. This endpoint is public (no API key required), but we still go through Rust so the
/// webview never talks to OpenRouter directly. Sorted newest-first by OpenRouter; we preserve
/// order. **Unfiltered** — [`list_models`] applies the ZDR filter for the picker.
pub async fn fetch_catalogue() -> Result<Vec<ModelDetail>> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<RawModel>,
    }
    #[derive(Deserialize)]
    struct RawModel {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        context_length: Option<u64>,
        #[serde(default)]
        pricing: Option<RawPricing>,
        #[serde(default)]
        architecture: Option<RawArch>,
        #[serde(default)]
        supported_parameters: Vec<String>,
        // Sparse and schema-loose across models (present on ~1 in 7), so parsed as a free
        // Value and probed defensively rather than against a fixed struct.
        #[serde(default)]
        benchmarks: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct RawPricing {
        // OpenRouter sends prices as decimal strings, e.g. "0.000003".
        prompt: Option<String>,
        completion: Option<String>,
        // The prompt-cache read rate, where the model supports prompt caching.
        input_cache_read: Option<String>,
    }
    #[derive(Deserialize)]
    struct RawArch {
        #[serde(default)]
        input_modalities: Vec<String>,
    }

    let response = HTTP
        .get(MODELS_ENDPOINT)
        .timeout(std::time::Duration::from_secs(30))
        .header(
            "HTTP-Referer",
            "https://github.com/Admin-Atlas/Personal-Manager",
        )
        .header("X-Title", "PM")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "OpenRouter models request failed ({status}): {detail}"
        )));
    }

    let parsed: Resp = response.json().await?;
    let models = parsed
        .data
        .into_iter()
        .map(|m| {
            let (prompt_price, completion_price, cache_read_price) = match m.pricing {
                Some(p) => (
                    parse_price(p.prompt),
                    parse_price(p.completion),
                    parse_price(p.input_cache_read),
                ),
                None => (None, None, None),
            };
            let intelligence_index = parse_intelligence_index(m.benchmarks.as_ref());
            ModelDetail {
                id: m.id,
                name: m.name,
                description: m.description,
                context_length: m.context_length,
                prompt_price,
                completion_price,
                cache_read_price,
                input_modalities: m
                    .architecture
                    .map(|a| a.input_modalities)
                    .unwrap_or_default(),
                supported_parameters: m.supported_parameters,
                intelligence_index,
            }
        })
        .collect();
    Ok(models)
}

/// Pull the Artificial-Analysis `intelligence_index` out of a model's sparse `benchmarks`
/// block, tolerating absence and non-numeric values (→ None). A general-capability proxy
/// only — the API has no faithfulness metric.
fn parse_intelligence_index(benchmarks: Option<&serde_json::Value>) -> Option<f64> {
    benchmarks
        .and_then(|b| b.get("artificial_analysis"))
        .and_then(|aa| aa.get("intelligence_index"))
        .and_then(|v| v.as_f64())
}

/// Fetch the set of model ids that have at least one Zero-Data-Retention endpoint.
///
/// `chat_body` pins `zdr: true` on every request, which makes a model with no ZDR endpoint
/// **uncallable** rather than merely less private — so this is the set PM can actually serve.
/// Public/unauthenticated, like the catalogue. Match key is `model_id`.
async fn fetch_zdr_model_ids() -> Result<HashSet<String>> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<RawEndpoint>,
    }
    #[derive(Deserialize)]
    struct RawEndpoint {
        model_id: String,
    }

    let response = HTTP
        .get(ZDR_ENDPOINT)
        .timeout(std::time::Duration::from_secs(30))
        .header(
            "HTTP-Referer",
            "https://github.com/Admin-Atlas/Personal-Manager",
        )
        .header("X-Title", "PM")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "OpenRouter ZDR endpoint request failed ({status}): {detail}"
        )));
    }

    let parsed: Resp = response.json().await?;
    Ok(parsed.data.into_iter().map(|e| e.model_id).collect())
}

/// True when a model id is one of OpenRouter's **router pseudo-models** (`openrouter/free`,
/// `openrouter/auto`, …). These never appear in the ZDR endpoint feed because they aren't
/// concrete endpoints — they pick an underlying model per request, at which point our
/// `provider: {zdr, data_collection}` pin applies to whatever they resolve to. So they are
/// safe under PM's enforcement and must survive the filter; dropping them would remove
/// `openrouter/free` (a real, in-use model) from the picker.
fn is_router_model(id: &str) -> bool {
    id.starts_with("openrouter/")
}

/// Keep only models PM can actually serve: those with a ZDR endpoint, plus the routers.
/// Pure so the policy is unit-testable without the network.
fn retain_zdr_servable(models: Vec<ModelDetail>, zdr_ids: &HashSet<String>) -> Vec<ModelDetail> {
    models
        .into_iter()
        .filter(|m| zdr_ids.contains(&m.id) || is_router_model(&m.id))
        .collect()
}

/// Fetch the catalogue trimmed to the [`ModelInfo`] the Settings model picker shows,
/// **filtered to what PM's privacy enforcement can actually serve** (spec §6).
///
/// PM pins `zdr: true` + `data_collection: "deny"` on every request ([`chat_body`]), so a
/// model with no compliant endpoint doesn't degrade — it 404s. Offering such a model in the
/// picker is therefore offering a broken choice, which is exactly how the removed recommender
/// could serve `anthropic/claude-fable-5` (6 endpoints, 0 ZDR). Filtering here makes that
/// class of failure unreachable from the list.
///
/// **Fails closed**: if the ZDR list can't be fetched we return the error rather than fall
/// back to the unfiltered catalogue — showing every model on a network blip would silently
/// reintroduce the non-servable ones. The picker surfaces the error; typing a custom model id
/// still works (spec §6 — PM is never locked to the catalogue), and a non-compliant one fails
/// closed at request time with the hint from [`request_error`].
///
/// Only the ZDR axis is filtered on, not `data_collection`. Verified live (15-07-2026): all
/// 42 ZDR-serving providers also report `training: false`, so the deny axis excludes nothing
/// the ZDR axis hasn't already — and the per-request pin enforces both regardless. That keeps
/// this on documented, stable API surface (`/api/v1/endpoints/zdr`) rather than the
/// undocumented frontend provider feed.
pub async fn list_models() -> Result<Vec<ModelInfo>> {
    let (catalogue, zdr_ids) = tokio::try_join!(fetch_catalogue(), fetch_zdr_model_ids())?;
    Ok(retain_zdr_servable(catalogue, &zdr_ids)
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            description: m.description,
            context_length: m.context_length,
            prompt_price: m.prompt_price,
            completion_price: m.completion_price,
            input_modalities: m.input_modalities,
        })
        .collect())
}

/// Parse a price string ("0.000003") into a float, discarding anything that isn't
/// a real non-negative number (OpenRouter uses "-1" / absent for "not priced").
fn parse_price(s: Option<String>) -> Option<f64> {
    s.and_then(|s| s.parse::<f64>().ok()).filter(|v| *v >= 0.0)
}

/// Turn a failed chat-completions response into an error, adding an actionable hint when
/// the failure looks like "no provider meets PM's zero-data-retention requirement". PM
/// enforces ZDR on every request (see `chat_body`), so a model with no compliant endpoint
/// fails here rather than leaking data — but without the hint the user would see only an
/// opaque 404/no-endpoints string and not realise the *model* (not PM) is the problem.
fn request_error(status: reqwest::StatusCode, detail: &str) -> Error {
    let lower = detail.to_lowercase();
    let zdr_related = lower.contains("data policy")
        || lower.contains("data_collection")
        || lower.contains("no endpoints")
        || lower.contains("no allowed providers")
        || lower.contains("zero data")
        || lower.contains("zdr");
    if zdr_related {
        Error::Other(format!(
            "OpenRouter request failed ({status}): {detail}. PM enforces zero-data-retention on \
             every request, so this model may have no compliant provider — pick another model, or \
             turn on auto-switch with a compliant fallback."
        ))
    } else {
        Error::Other(format!("OpenRouter request failed ({status}): {detail}"))
    }
}

/// Build the request body, picking single-model vs. fallback-routing form. With
/// one model we send `"model"`; with several we send `"models"` (an ordered
/// fallback list — OpenRouter advances to the next on a rate-limit/quota/provider
/// error, which is how auto-switch works). Callers guarantee a non-empty list.
fn chat_body(
    models: &[String],
    messages: &[ChatMessage],
    stream: bool,
    cache_through: Option<usize>,
) -> serde_json::Value {
    // Build the messages array by hand so we can optionally mark a stable system prefix with an
    // ephemeral prompt-cache breakpoint. `cache_through` is the index of the LAST message of the
    // stable prefix: OpenRouter forwards `cache_control` to providers that support prompt caching
    // (e.g. Anthropic), which cache everything up to AND INCLUDING that message and bill the reused
    // prefix at the cheap cache-read rate — a big saving when the same prefix repeats across calls (the
    // per-document review loop marks index 0; chat marks the last of its profile/agenda/summary block,
    // keeping the per-turn grounding + history AFTER the breakpoint where they stay uncached). Providers
    // without caching, or a prefix below their minimum cacheable size, simply ignore it. Non-marked
    // messages serialise exactly as before.
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if cache_through == Some(i) {
                serde_json::json!({
                    "role": m.role,
                    "content": [{
                        "type": "text",
                        "text": m.content,
                        "cache_control": { "type": "ephemeral" },
                    }],
                })
            } else {
                serde_json::json!({ "role": m.role, "content": m.content })
            }
        })
        .collect();

    let mut body = serde_json::json!({
        "messages": msgs,
        "stream": stream,
        // Usage accounting: have OpenRouter report the actual USD cost (net of any prompt-cache
        // discount) so the cost logger can store real spend, not just a tokens × price estimate.
        "usage": { "include": true },
    });
    if stream {
        // Ask OpenRouter to emit a final usage chunk so we can log token spend.
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    if models.len() == 1 {
        body["model"] = serde_json::json!(models[0]);
    } else {
        body["models"] = serde_json::json!(models);
    }
    // PRIVACY (spec §6) — enforce Zero-Data-Retention on EVERY request, whichever model the
    // user picked. `zdr: true` keeps the request on endpoints that don't retain prompts;
    // `data_collection: "deny"` blocks providers that train on / store data. Two distinct
    // axes: some providers don't train but do retain (abuse-scanning/legal), so neither
    // implies the other and we pin both.
    //
    // OpenRouter combines these with the account-level policy using OR semantics — a
    // per-request flag can only *add* enforcement, never weaken it — so this is safe
    // regardless of the user's account config and is the real privacy boundary. It stays
    // the boundary even though `list_models` now filters the picker against
    // /api/v1/endpoints/zdr: that filter is reachability (don't offer a model we can't
    // serve), while this pin is enforcement, and it still covers ids that bypass the picker
    // — a custom-typed model, a stored id, DEFAULT_MODEL, or whatever a router resolves to.
    // (The catalogue's model objects still carry no data-policy field; /endpoints/zdr is a
    // separate feed, keyed by model_id.) If no compliant endpoint exists
    // for a model the request fails closed (privacy-preserving) and auto-switch falls
    // through to the next model in the list.
    body["provider"] = serde_json::json!({ "zdr": true, "data_collection": "deny" });
    body
}

/// POST a streaming chat completion. `on_token` is called with each text delta as
/// it arrives. Returns the assembled reply plus the model that actually served it
/// (which can differ from the first requested model when a fallback fires).
pub async fn stream_chat<F>(
    api_key: &str,
    models: &[String],
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    mut on_token: F,
) -> Result<Completion>
where
    F: FnMut(&str),
{
    // `cache_through` marks the end of the stable system prefix (the profile/agenda/rolling-summary
    // block, card 7C) so providers cache it and bill it cheaply turn after turn; the per-turn grounding
    // and growing history sit after it and stay uncached. `None` (no stable block) preserves the old
    // fully-uncached behaviour.
    let body = chat_body(models, messages, true, cache_through);

    // A long, healthy streamed reply can legitimately run for many minutes (a slow or reasoning model,
    // a large grounded answer). Bounding the WHOLE request with a total `.timeout` therefore severs a
    // reply mid-stream — the user watches tokens arrive, then gets an error, and the aborted turn is
    // never persisted → via F-02 the conversation wedges (F-16 / B4-5). The total deadline was only ever
    // meant to bound a hung connection, not a healthy slow stream. Replace it with the two signals that
    // actually indicate a dead connection: `connect_timeout` bounds establishing the socket, and
    // `read_timeout` bounds *silence between chunks* — it resets on every byte received, so steady tokens
    // never trip it however long the reply, while a genuinely stalled stream still aborts. Those two
    // bounds now live on the shared `HTTP` client (F-16), so this request just adds NO total `.timeout()`.
    // Non-streaming `complete` keeps its total deadline via its own per-request `.timeout()`: there the
    // whole answer arrives at once, so a total bound fits.
    let response = HTTP
        .post(ENDPOINT)
        .bearer_auth(api_key)
        // Optional attribution headers OpenRouter recognises.
        .header(
            "HTTP-Referer",
            "https://github.com/Admin-Atlas/Personal-Manager",
        )
        .header("X-Title", "PM")
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(request_error(status, &detail));
    }

    // Untrusted model output (rule #6): bound both the assembled reply and any
    // single unterminated SSE line so a malicious/runaway endpoint can't grow
    // memory without limit within the request timeout. (The ICS feed path caps
    // fetched bytes the same way.) Both limits sit far above any real reply.
    const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
    const MAX_SSE_LINE_BYTES: usize = 2 * 1024 * 1024;

    let mut full = String::new();
    let mut served: Option<String> = None;
    let mut usage = Usage::default();
    // Raw bytes, decoded only one complete line at a time: a multi-byte UTF-8
    // char split across two network chunks must not be lossily decoded in halves.
    let mut truncated = false;
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);

        // Server-Sent Events are newline-delimited; process whole lines only.
        for line in drain_lines(&mut buffer) {
            let Some(data) = line.strip_prefix("data:") else {
                continue; // SSE comments / keep-alives
            };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(Completion {
                    text: full,
                    model: served,
                    usage,
                    truncated,
                });
            }
            if data.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                // A mid-stream failure. Erroring routes into `send_message`'s existing error path
                // (ChatEvent::Error), whose dangling-user-turn self-heal already covers the user row
                // persisted before the stream began. `full` is dropped on purpose: a partial reply
                // from a failed call is exactly what must not be kept.
                if let Some(e) = chunk_error(&value) {
                    return Err(e);
                }
                // The chunk carries which model actually served the request — keep
                // the first one we see so the stored message reflects any fallback.
                if served.is_none() {
                    if let Some(m) = value["model"].as_str() {
                        served = Some(m.to_string());
                    }
                }
                // Why the reply stopped. Flag, not error — the text is worth keeping; the caller
                // marks the stored turn honest (see `Completion::truncated`).
                if is_length_stop(&value) {
                    truncated = true;
                }
                // The final chunk (empty choices) carries token usage — we asked for
                // it via stream_options; keep it whenever present.
                let u = parse_usage(&value);
                if u.prompt_tokens.is_some() || u.completion_tokens.is_some() {
                    usage = u;
                }
                if let Some(token) = value["choices"][0]["delta"]["content"].as_str() {
                    full.push_str(token);
                    if full.len() > MAX_REPLY_BYTES {
                        return Err(Error::Other(
                            "the model reply exceeded the size limit".into(),
                        ));
                    }
                    on_token(token);
                }
            }
        }

        // After draining complete lines, only an unfinished line remains; if it has
        // grown past the cap there's no newline coming — bail rather than buffer on.
        if buffer.len() > MAX_SSE_LINE_BYTES {
            return Err(Error::Other(
                "the model stream sent an oversized line".into(),
            ));
        }
    }

    Ok(Completion {
        text: full,
        model: served,
        usage,
        truncated,
    })
}

/// Drain every complete `\n`-terminated line from a raw SSE byte buffer, decoding
/// each whole line as UTF-8 (lossy) and trimming it. Incomplete trailing bytes
/// stay in `buffer` for the next chunk, so a multi-byte char straddling a chunk
/// boundary is decoded once, intact, rather than as two replacement characters.
pub(crate) fn drain_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buffer.drain(..=newline).collect();
        lines.push(String::from_utf8_lossy(&line_bytes).trim().to_string());
    }
    lines
}

/// A single non-streaming chat completion — used for background work (sorting
/// proposals, the Learning-You profile) where we want the whole answer at once,
/// not a token stream. Takes an ordered model list (one model, or several for
/// auto-switch fallback). Returns the assistant message content. Set `cache_prefix`
/// when the first message is a stable prefix reused across many back-to-back calls
/// (the review loop) so providers cache + cheaply reuse it.
pub async fn complete(
    api_key: &str,
    models: &[String],
    messages: &[ChatMessage],
    cache_prefix: bool,
) -> Result<Completion> {
    // Background callers cache from message 0 (their stable system instruction); `false` ⇒ no breakpoint.
    let body = chat_body(models, messages, false, cache_prefix.then_some(0));

    let response = HTTP
        .post(ENDPOINT)
        // A total deadline fits here (unlike `stream_chat`): the whole answer arrives at once.
        .timeout(std::time::Duration::from_secs(120))
        .bearer_auth(api_key)
        .header(
            "HTTP-Referer",
            "https://github.com/Admin-Atlas/Personal-Manager",
        )
        .header("X-Title", "PM")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(request_error(status, &detail));
    }

    let value: serde_json::Value = response.json().await?;
    let text = value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Other("OpenRouter response had no message content".into()))?;
    Ok(Completion {
        text,
        model: value["model"].as_str().map(str::to_string),
        usage: parse_usage(&value),
        // Same signal, same meaning — the non-streaming path gets it in one response rather than a
        // chunk, and callers must not have to care which path produced the Completion.
        truncated: value["choices"][0]["finish_reason"].as_str() == Some("length"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(text: &str, truncated: bool) -> Completion {
        Completion {
            text: text.into(),
            model: None,
            usage: Usage::default(),
            truncated,
        }
    }

    #[test]
    fn a_finished_answer_and_a_failed_one_are_now_different_things() {
        // The pair PM could not tell apart. Both arrive as HTTP 200 with text attached, and every
        // structured parser in the tree degrades to an empty result rather than raising — so the
        // caller wrote both down as "there was nothing to find" and moved a cursor past the turns
        // they came from.
        assert_eq!(
            completion("[]", false).usable_text(),
            Some("[]"),
            "an empty ARRAY is a real answer — most conversations state no preference"
        );
        assert_eq!(
            completion("  - a bullet  ", false).usable_text(),
            Some("- a bullet"),
            "usable text comes back trimmed"
        );

        // Cut off at the token ceiling: the JSON has no closing bracket and the prose has no last
        // clause, but nothing about the response says so except this flag.
        assert_eq!(completion(r#"[{"scope":"glob"#, true).usable_text(), None);
        // Nothing at all.
        assert_eq!(completion("", false).usable_text(), None);
        assert_eq!(completion("   \n  ", false).usable_text(), None);

        // The reason is the part a user can act on, so it has to distinguish the two as well.
        assert!(completion("x", true)
            .unusable_reason()
            .unwrap()
            .contains("cut off"));
        assert!(completion(" ", false)
            .unusable_reason()
            .unwrap()
            .contains("empty"));
        assert_eq!(completion("fine", false).unusable_reason(), None);
    }

    #[test]
    fn a_prompt_cut_at_the_front_is_deliberately_not_caught_here() {
        // The complementary half, stated so nobody widens this into a claim it cannot make. A
        // server running with `--context-shift` discards the front of an over-long prompt, answers
        // `finish_reason: stop`, and reports its `prompt_tokens` AFTER the cut. The reply is a
        // complete, well-formed answer to a question PM did not ask, and there is nothing in the
        // response to detect it by — which is why the sizing check that prevents it has to run
        // BEFORE the request goes out.
        let answered_a_decapitated_prompt = completion(r#"{"assignments":[]}"#, false);
        assert!(
            answered_a_decapitated_prompt.usable_text().is_some(),
            "this check is about the REPLY; input truncation is unobservable from here"
        );
    }

    #[test]
    fn a_mid_stream_error_chunk_is_an_error() {
        // The whole bug: the HTTP status is 200 and the failure arrives INSIDE the stream. Ignoring
        // it let the loop run out of chunks and return the truncated text as a success, which the
        // caller then persisted to messages + the vault + the index as a complete assistant turn.
        let chunk = serde_json::json!({
            "error": {"code": 502, "message": "upstream provider is down"}
        });
        let err = chunk_error(&chunk).expect("an error chunk must not be ignored");
        assert!(err.to_string().contains("upstream provider is down"));

        // A ZDR refusal arriving mid-stream gets the same actionable copy as one at request time —
        // that is the point of routing through `request_error` rather than a bare message.
        let zdr = serde_json::json!({
            "error": {"code": 404, "message": "No endpoints found matching your data policy"}
        });
        assert!(chunk_error(&zdr)
            .unwrap()
            .to_string()
            .contains("auto-switch"));

        // A malformed error object is still an error — never a silent success.
        assert!(chunk_error(&serde_json::json!({"error": {}})).is_some());
    }

    #[test]
    fn an_ordinary_chunk_is_not_an_error() {
        // The common case runs through this on every token; it must never false-positive.
        let chunk = serde_json::json!({
            "model": "x/y",
            "choices": [{"delta": {"content": "hello"}}]
        });
        assert!(chunk_error(&chunk).is_none());
        assert!(!is_length_stop(&chunk));
    }

    #[test]
    fn a_length_stop_is_flagged_but_other_reasons_are_not() {
        let length = serde_json::json!({"choices": [{"finish_reason": "length"}]});
        assert!(is_length_stop(&length), "hit the token ceiling mid-thought");

        // A normal end must not be marked — otherwise every complete reply gets the caveat.
        for reason in ["stop", "tool_calls", "content_filter"] {
            let v = serde_json::json!({"choices": [{"finish_reason": reason}]});
            assert!(!is_length_stop(&v), "{reason} is a normal end");
        }
        // Streaming chunks carry no finish_reason until the last one.
        assert!(!is_length_stop(
            &serde_json::json!({"choices": [{"delta": {}}]})
        ));
    }

    #[test]
    fn drain_lines_decodes_multibyte_split_across_chunks() {
        // "é" is 0xC3 0xA9; arrive with the two bytes in separate chunks.
        let mut buffer: Vec<u8> = Vec::new();
        buffer.extend_from_slice(b"data: caf\xc3");
        // No newline and the char is incomplete: nothing should be emitted yet,
        // and the lone 0xC3 must not be decoded into a replacement character.
        assert!(drain_lines(&mut buffer).is_empty());
        buffer.extend_from_slice(b"\xa9\n");
        assert_eq!(drain_lines(&mut buffer), vec!["data: café".to_string()]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_lines_yields_multiple_lines_and_keeps_partial_tail() {
        let mut buffer = b"data: a\ndata: b\ndata: c".to_vec();
        assert_eq!(
            drain_lines(&mut buffer),
            vec!["data: a".to_string(), "data: b".to_string()],
        );
        // The unterminated "data: c" is held back for the next chunk.
        assert_eq!(buffer, b"data: c");
    }

    #[test]
    fn parse_usage_reads_tokens_and_defaults_to_none() {
        // The final streamed usage chunk has empty choices + a usage object.
        let chunk: serde_json::Value = serde_json::from_str(
            r#"{"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":45}}"#,
        )
        .unwrap();
        let u = parse_usage(&chunk);
        assert_eq!(u.prompt_tokens, Some(123));
        assert_eq!(u.completion_tokens, Some(45));
        // A chunk with no usage object → both None (cost shown as unknown, not zero).
        let plain: serde_json::Value =
            serde_json::from_str(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        let n = parse_usage(&plain);
        assert!(n.prompt_tokens.is_none() && n.completion_tokens.is_none());
    }

    #[test]
    fn chat_body_enforces_zdr_and_keeps_model_routing() {
        // Single model: the "model" field, the ZDR provider object, usage accounting, and the
        // streaming usage opt-in. chat_body is PM's only ZDR enforcement point, so this guards it.
        let one = chat_body(&["a/b".to_string()], &[], true, None);
        assert_eq!(one["provider"]["zdr"], serde_json::json!(true));
        assert_eq!(
            one["provider"]["data_collection"],
            serde_json::json!("deny")
        );
        assert_eq!(one["model"], serde_json::json!("a/b"));
        assert!(one.get("models").is_none());
        assert_eq!(one["usage"]["include"], serde_json::json!(true));
        assert_eq!(
            one["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        // Several models: the ordered "models" fallback list, still ZDR-enforced + usage-accounted,
        // no "model" and (non-streaming) no stream_options.
        let many = chat_body(&["a/b".to_string(), "c/d".to_string()], &[], false, None);
        assert_eq!(many["models"], serde_json::json!(["a/b", "c/d"]));
        assert!(many.get("model").is_none());
        assert_eq!(many["provider"]["zdr"], serde_json::json!(true));
        // Both privacy axes on the fallback form too: they're independent (a provider can
        // decline to train yet still retain), so asserting only zdr would let a regression
        // that dropped data_collection through on the multi-model path.
        assert_eq!(
            many["provider"]["data_collection"],
            serde_json::json!("deny")
        );
        assert_eq!(many["usage"]["include"], serde_json::json!(true));
        assert!(many.get("stream_options").is_none());
    }

    fn detail(id: &str) -> ModelDetail {
        ModelDetail {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn zdr_filter_keeps_compliant_models_and_drops_unservable_ones() {
        // The picker must not offer a model PM cannot serve: chat_body pins zdr:true, so a
        // model with no ZDR endpoint 404s rather than degrading. This is the fable-5 class of
        // bug that the removed recommender shipped (6 endpoints, none of them ZDR).
        let zdr: HashSet<String> = ["ok/servable".to_string()].into_iter().collect();
        let kept = retain_zdr_servable(
            vec![detail("ok/servable"), detail("anthropic/fable-5")],
            &zdr,
        );
        let ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["ok/servable"]);
    }

    #[test]
    fn zdr_filter_keeps_router_pseudo_models() {
        // Routers are absent from the ZDR feed (they aren't concrete endpoints) yet are
        // servable: the per-request pin applies to whatever they resolve to. Filtering them
        // out would delete openrouter/free — a real, in-use model — from the picker.
        let zdr: HashSet<String> = HashSet::new();
        let kept = retain_zdr_servable(
            vec![
                detail("openrouter/free"),
                detail("openrouter/auto"),
                detail("x/no-zdr"),
            ],
            &zdr,
        );
        let ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["openrouter/free", "openrouter/auto"]);
    }

    #[test]
    fn router_match_respects_the_namespace_boundary() {
        assert!(is_router_model("openrouter/free"));
        // A lookalike vendor namespace must not inherit the router carve-out.
        assert!(!is_router_model("openrouter-mirror/free"));
        assert!(!is_router_model("openai/gpt-5.5"));
    }

    #[test]
    fn chat_body_marks_the_cache_breakpoint_at_the_given_index() {
        let msgs = vec![
            ChatMessage {
                role: "system".into(),
                content: "profile".into(),
            },
            ChatMessage {
                role: "system".into(),
                content: "rolling summary".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "the latest turn".into(),
            },
        ];
        // None → every message is a plain string, no cache_control anywhere (the old chat behaviour).
        let plain = chat_body(&["a/b".to_string()], &msgs, false, None);
        for i in 0..msgs.len() {
            assert!(
                plain["messages"][i]["content"].is_string(),
                "message {i} stays a plain string when uncached"
            );
        }
        // Some(1) → the breakpoint sits on message 1 (the last stable system message), so the provider
        // caches the prefix through it; the earlier stable message and the dynamic turn after it stay
        // plain. (Anthropic caches everything up to AND INCLUDING the marked block.)
        let cached = chat_body(&["a/b".to_string()], &msgs, false, Some(1));
        assert!(
            cached["messages"][0]["content"].is_string(),
            "messages before the breakpoint are not themselves marked"
        );
        assert_eq!(
            cached["messages"][1]["content"][0]["cache_control"]["type"],
            serde_json::json!("ephemeral"),
            "the breakpoint is on the last stable message"
        );
        assert_eq!(
            cached["messages"][1]["content"][0]["text"],
            serde_json::json!("rolling summary")
        );
        assert_eq!(
            cached["messages"][2]["content"],
            serde_json::json!("the latest turn"),
            "the dynamic turn after the breakpoint stays uncached"
        );
    }
}
