// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streaming chat against OpenRouter (spec §6 — one key, any model, swappable).
//! The API key is read from the keychain on the Rust side and never reaches the
//! webview.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MODELS_ENDPOINT: &str = "https://openrouter.ai/api/v1/models";

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

/// The fuller per-model record the model **recommender** (spec §6) reasons over and
/// that the daily price refresh caches. On top of pricing it carries the prompt-cache
/// read rate (PM reuses stable prompt prefixes, so cache reads dominate effective cost),
/// the model's `supported_parameters` (the structured-output / tool-calling reliability
/// check), and the Artificial-Analysis `intelligence_index` from the catalogue's
/// `benchmarks` block — a *general-capability* signal, NOT a faithfulness metric (see
/// [`crate::recommend`]). Every field is optional because the catalogue is sparse: most
/// models carry no benchmarks, and some report no price.
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

/// Fetch the full OpenRouter model catalogue as the richer [`ModelDetail`] the
/// recommender + price cache need. This endpoint is public (no API key required), but we
/// still go through Rust so the webview never talks to OpenRouter directly. Sorted
/// newest-first by OpenRouter; we preserve order.
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
/// only — the API has no faithfulness metric (see [`crate::recommend`]).
fn parse_intelligence_index(benchmarks: Option<&serde_json::Value>) -> Option<f64> {
    benchmarks
        .and_then(|b| b.get("artificial_analysis"))
        .and_then(|aa| aa.get("intelligence_index"))
        .and_then(|v| v.as_f64())
}

/// Fetch the catalogue trimmed to the [`ModelInfo`] the Settings model picker shows.
pub async fn list_models() -> Result<Vec<ModelInfo>> {
    Ok(fetch_catalogue()
        .await?
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
    // PRIVACY (spec §6) — enforce Zero-Data-Retention on EVERY request, not just on
    // recommended models. `zdr: true` keeps the request on endpoints that don't retain
    // prompts; `data_collection: "deny"` blocks providers that train on / store data.
    // OpenRouter combines these with the account-level policy using OR semantics — a
    // per-request flag can only *add* enforcement, never weaken it — so this is safe
    // regardless of the user's account config and is the real privacy boundary: the
    // public catalogue exposes no per-model data-policy field (verified against /models
    // and /models/:id/endpoints, neither carries one). If no compliant endpoint exists
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
                });
            }
            if data.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                // The chunk carries which model actually served the request — keep
                // the first one we see so the stored message reflects any fallback.
                if served.is_none() {
                    if let Some(m) = value["model"].as_str() {
                        served = Some(m.to_string());
                    }
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
    })
}

/// Drain every complete `\n`-terminated line from a raw SSE byte buffer, decoding
/// each whole line as UTF-8 (lossy) and trimming it. Incomplete trailing bytes
/// stay in `buffer` for the next chunk, so a multi-byte char straddling a chunk
/// boundary is decoded once, intact, rather than as two replacement characters.
fn drain_lines(buffer: &mut Vec<u8>) -> Vec<String> {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(many["usage"]["include"], serde_json::json!(true));
        assert!(many.get("stream_options").is_none());
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
