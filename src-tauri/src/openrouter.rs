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

#[derive(Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
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

/// Fetch the full OpenRouter model catalogue. This endpoint is public (no API key
/// required), but we still go through Rust for it so the webview never has to talk
/// to OpenRouter directly. Sorted newest-first by OpenRouter; we preserve order.
pub async fn list_models() -> Result<Vec<ModelInfo>> {
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
    }
    #[derive(Deserialize)]
    struct RawPricing {
        // OpenRouter sends prices as decimal strings, e.g. "0.000003".
        prompt: Option<String>,
        completion: Option<String>,
    }
    #[derive(Deserialize)]
    struct RawArch {
        #[serde(default)]
        input_modalities: Vec<String>,
    }

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?
        .get(MODELS_ENDPOINT)
        .header("HTTP-Referer", "https://github.com/Admin-Atlas/Personal-Manager")
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
            let (prompt_price, completion_price) = match m.pricing {
                Some(p) => (parse_price(p.prompt), parse_price(p.completion)),
                None => (None, None),
            };
            ModelInfo {
                id: m.id,
                name: m.name,
                description: m.description,
                context_length: m.context_length,
                prompt_price,
                completion_price,
                input_modalities: m.architecture.map(|a| a.input_modalities).unwrap_or_default(),
            }
        })
        .collect();
    Ok(models)
}

/// Parse a price string ("0.000003") into a float, discarding anything that isn't
/// a real non-negative number (OpenRouter uses "-1" / absent for "not priced").
fn parse_price(s: Option<String>) -> Option<f64> {
    s.and_then(|s| s.parse::<f64>().ok()).filter(|v| *v >= 0.0)
}

/// Build the request body, picking single-model vs. fallback-routing form. With
/// one model we send `"model"`; with several we send `"models"` (an ordered
/// fallback list — OpenRouter advances to the next on a rate-limit/quota/provider
/// error, which is how auto-switch works). Callers guarantee a non-empty list.
fn chat_body(models: &[String], messages: &[ChatMessage], stream: bool) -> serde_json::Value {
    let mut body = serde_json::json!({
        "messages": messages,
        "stream": stream,
    });
    if models.len() == 1 {
        body["model"] = serde_json::json!(models[0]);
    } else {
        body["models"] = serde_json::json!(models);
    }
    body
}

/// POST a streaming chat completion. `on_token` is called with each text delta as
/// it arrives. Returns the assembled reply plus the model that actually served it
/// (which can differ from the first requested model when a fallback fires).
pub async fn stream_chat<F>(
    api_key: &str,
    models: &[String],
    messages: &[ChatMessage],
    mut on_token: F,
) -> Result<(String, Option<String>)>
where
    F: FnMut(&str),
{
    let body = chat_body(models, messages, true);

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?
        .post(ENDPOINT)
        .bearer_auth(api_key)
        // Optional attribution headers OpenRouter recognises.
        .header("HTTP-Referer", "https://github.com/Admin-Atlas/Personal-Manager")
        .header("X-Title", "PM")
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(Error::Other(format!("OpenRouter request failed ({status}): {detail}")));
    }

    // Untrusted model output (rule #6): bound both the assembled reply and any
    // single unterminated SSE line so a malicious/runaway endpoint can't grow
    // memory without limit within the request timeout. (The ICS feed path caps
    // fetched bytes the same way.) Both limits sit far above any real reply.
    const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
    const MAX_SSE_LINE_BYTES: usize = 2 * 1024 * 1024;

    let mut full = String::new();
    let mut served: Option<String> = None;
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
                return Ok((full, served));
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
                if let Some(token) = value["choices"][0]["delta"]["content"].as_str() {
                    full.push_str(token);
                    if full.len() > MAX_REPLY_BYTES {
                        return Err(Error::Other("the model reply exceeded the size limit".into()));
                    }
                    on_token(token);
                }
            }
        }

        // After draining complete lines, only an unfinished line remains; if it has
        // grown past the cap there's no newline coming — bail rather than buffer on.
        if buffer.len() > MAX_SSE_LINE_BYTES {
            return Err(Error::Other("the model stream sent an oversized line".into()));
        }
    }

    Ok((full, served))
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
}

/// A single non-streaming chat completion — used for background work (sorting
/// proposals, the Learning-You profile) where we want the whole answer at once,
/// not a token stream. Takes an ordered model list (one model, or several for
/// auto-switch fallback). Returns the assistant message content.
pub async fn complete(api_key: &str, models: &[String], messages: &[ChatMessage]) -> Result<String> {
    let body = chat_body(models, messages, false);

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?
        .post(ENDPOINT)
        .bearer_auth(api_key)
        .header("HTTP-Referer", "https://github.com/Admin-Atlas/Personal-Manager")
        .header("X-Title", "PM")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(Error::Other(format!("OpenRouter request failed ({status}): {detail}")));
    }

    let value: serde_json::Value = response.json().await?;
    value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Other("OpenRouter response had no message content".into()))
}
