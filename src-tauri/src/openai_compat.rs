// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An OpenAI-compatible chat client for a user-configured **local** endpoint — Ollama
//! (`:11434/v1`), LM Studio (`:1234/v1`), llama-server (`:8080/v1`), or anything that speaks
//! `/v1/chat/completions` SSE. This is the LOCAL arm of the provider seam (#297); the cloud arm
//! stays in [`crate::openrouter`]. The request body here is deliberately **minimal** — plain
//! `model` + `messages` + `stream` — carrying NONE of OpenRouter's body fields (no `provider`
//! ZDR pin, no `cache_control`, no `models` fallback array), because a local server understands
//! none of them and rejecting or ignoring them varies by server.
//!
//! Design: everything that can be wrong *without a socket* — SSE framing, failure classification,
//! the degenerate-stream guard, URL normalisation, the `/v1/models` shape check, the request
//! body's shape, and the context-window ladder's preference order — is a pure function, unit
//! tested below. The network-touching entrypoints (`stream_chat`, `complete`, `probe`,
//! `probe_window`) are the thin I/O edge, and their behaviour against a real server is NOT
//! covered here or in CI — the epic's live-rig checklist owns that check and still owes it.
//! "OpenAI-compatible" is a spectrum in practice, so the parser tolerates all three
//! named servers plus buffered/keepalive variants and never crashes on an unknown field.
//!
//! Live as of #297 PR3: the gateway ([`crate::llm_gateway`]) drives these entrypoints for a
//! configured local endpoint. Timeouts and the loop-guard fast path read the central tunables in
//! [`crate::local_slot::tunables`], so tuning after live testing is a one-file edit there.

use futures_util::StreamExt;

use crate::error::{Error, Result};
use crate::local_slot::tunables;
use crate::openrouter::{drain_lines, ChatMessage, Completion, Usage};

/// HTTP clients for local-endpoint calls. Two of them, differing in connect timeout and in whether
/// they honour a system proxy: a loopback server that isn't listening RSTs instantly (2 s is ample),
/// while a remote (LAN / Tailscale) endpoint may take longer to connect. Separate from
/// `openrouter::HTTP` so the cloud path can never be perturbed (strict additivity), and — crucially
/// — NEITHER sets a `read_timeout`: streaming manages a two-phase (first-token vs inter-token)
/// deadline itself, and the non-streaming calls set a per-request total `timeout`. A single flat read
/// timeout cannot tell a legitimate 60 s cold model load from a dead stream.
static LOCAL_HTTP_LOOPBACK: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| build_local_client(tunables::CONNECT_TIMEOUT_LOOPBACK, true));
static LOCAL_HTTP_REMOTE: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| build_local_client(tunables::CONNECT_TIMEOUT_REMOTE, false));

/// `bypass_proxy` is the whole point of the loopback/remote split beyond the timeout.
///
/// reqwest defaults to auto-system-proxy, and neither reqwest nor hyper-util's matcher has any
/// loopback special-casing — `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` are read unconditionally. So
/// on a machine with a proxy variable set, which is the normal state of affairs behind a corporate
/// network or a VPN client, PM's request to the user's OWN Ollama on `127.0.0.1:11434` was handed to
/// the proxy. The proxy has no route to the user's loopback, the connect fails, and the failure is
/// scored as a `Strike` — three of those and the circuit breaker cools a perfectly healthy local
/// model down for 60 s escalating to 300 s, taking chat with it.
///
/// Only the loopback client bypasses. A remote endpoint is a real network destination and may
/// legitimately need the proxy to reach it, so that client is left exactly as it was. reqwest does
/// not offer a middle ground: `ClientBuilder::proxy()` sets `auto_sys_proxy = false`, so adding a
/// NO_PROXY-style rule on top of the system proxies would mean re-implementing the env parsing.
fn build_local_client(connect_timeout: std::time::Duration, bypass_proxy: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder().connect_timeout(connect_timeout);
    let builder = if bypass_proxy {
        builder.no_proxy()
    } else {
        builder
    };
    builder
        .build()
        .expect("reqwest client with a static connect timeout should build")
}

/// Pick the client (and thus the connect timeout) by whether the endpoint host is a loopback
/// literal / `localhost`. A cheap SYNTACTIC check only, and **not** a security control: a name can
/// resolve anywhere, so this decides a timeout and nothing else.
///
/// The security posture decision (resolve the address, refuse public cleartext) is a separate,
/// stricter check made by the CALLER, not here — `local_ai::endpoint_refused_now`, applied by the
/// `local_ai::configured_endpoint` prologue that every endpoint command shares and by the two
/// `llm_gateway` local arms plus its window-probe task. It cannot live in this function: it is
/// async, and a refusal here would have to become a `LocalFailKind`, which the circuit breaker
/// would score as a strike — wrong, because a rebound host is not a dead host.
///
/// (This comment previously asserted a pre-call check that six of the seven paths through this
/// module did not in fact perform. Keep it true: a false invariant comment is how that gap stayed
/// invisible.)
fn client_for(base_url: &str) -> &'static reqwest::Client {
    if host_is_loopback_literal(base_url) {
        &LOCAL_HTTP_LOOPBACK
    } else {
        &LOCAL_HTTP_REMOTE
    }
}

/// Whether the URL's host is `localhost` or a loopback IP literal — a cheap string check (no DNS).
fn host_is_loopback_literal(base_url: &str) -> bool {
    let after_scheme = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // Host = authority minus a trailing :port. IPv6 literals are bracketed, e.g. `[::1]:11434`.
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .map(|v4| v4.is_loopback())
            .unwrap_or(false)
}

// Untrusted model output (rule #6): bound both the assembled reply and any single unterminated SSE
// line so a malicious/runaway local endpoint can't grow memory without limit. Same caps as the
// cloud arm; both sit far above any real reply.
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
const MAX_SSE_LINE_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// Typed failures — classified structurally AT THIS LAYER so no caller ever string-matches later.
// ---------------------------------------------------------------------------------------------

/// Why a local-endpoint call failed. The gateway (#297 PR3) maps these to fallback + dead-host
/// cooldown policy — *which* kinds count as a "strike" and which mean the host is alive lives with
/// that policy, not here. This enum only names the failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalFailKind {
    /// Connection refused / DNS failure / host unreachable — nothing is listening.
    Refused,
    /// Connect timed out, or the stream went silent past the inter-chunk read deadline.
    Timeout,
    /// The SSE stream broke down: an oversized line, an undecodable frame, a mid-stream `error`
    /// object, or a stream that stopped without any clean end signal.
    MalformedStream,
    /// The endpoint answered with a success status, and PM could not read the body: unparseable
    /// JSON, or a shape that is not the one this route returns.
    ///
    /// Split out of [`Self::MalformedStream`] because nothing streamed. It was raised by the
    /// `/v1/models` probe and by the non-streaming completion path, which meant a user whose
    /// endpoint answered 200 with an unexpected body was told "the local model stream ended
    /// unexpectedly" about a call with no stream in it — and the one diagnosable part, the body
    /// itself, was dropped. The host ANSWERED, so the policy treats this as alive: a server that
    /// replies is not a dead host, and ejecting it would hide the problem behind a cooldown.
    UnrecognisedResponse,
    /// The degenerate-stream guard tripped on an obvious token loop.
    DegenerateStream,
    /// A 503 whose body looks like "model is loading" — the host is ALIVE, just warming up. The
    /// policy treats this as alive (no cooldown strike), unlike a plain 5xx.
    ModelLoading,
    /// Any other 5xx from the server.
    ServerError(u16),
    /// A 4xx (bad model id, auth) — the host answered, so it is alive; this is a config problem,
    /// not a dead host.
    ClientError(u16),
    /// The reply or a single line exceeded the untrusted-output byte caps.
    ReplyTooLarge,
    /// PM's own prompt is larger than the window the server is serving, so it was NOT sent.
    ///
    /// The only failure kind raised before the request leaves PM, and the only one that is PM's
    /// fault rather than the host's. It exists because the alternative is unobservable: a server
    /// running with `--context-shift` accepts an oversized prompt, silently discards the front of it
    /// (the system message — the output contract and the untrusted-data guard), answers 200, and
    /// reports a `prompt_tokens` measured after the cut. Every downstream check passes and the
    /// result is wrong. Refusing by name is the only way the user ever learns the window is the
    /// problem. Never a strike: the host is healthy, and ejecting it would rest a working server for
    /// a prompt PM chose to build.
    PromptTooLarge,
}

/// A local failure with its human-readable detail (server body, or a short description). The
/// gateway converts this to a `crate::error::Error` only at the point it surfaces to the UI.
#[derive(Clone, Debug)]
pub struct LocalFailure {
    pub kind: LocalFailKind,
    pub detail: String,
}

impl LocalFailure {
    fn new(kind: LocalFailKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

pub type LocalResult<T> = std::result::Result<T, LocalFailure>;

/// Classify a non-success HTTP status + body into a typed failure. Pure so the shape-matching —
/// the part that silently decides fallback vs cooldown — is testable without a server.
pub fn classify_http(status: u16, body: &str) -> LocalFailKind {
    if status == 503 && body_looks_like_loading(body) {
        return LocalFailKind::ModelLoading;
    }
    if (500..600).contains(&status) {
        return LocalFailKind::ServerError(status);
    }
    if (400..500).contains(&status) {
        return LocalFailKind::ClientError(status);
    }
    // Defensive: only reached for a non-success status, so anything else is a server problem.
    LocalFailKind::ServerError(status)
}

/// Whether a 503 body reads like a model still loading (Ollama/llama-server warm-up) rather than a
/// hard server error. Tolerant substring match — servers word this differently.
fn body_looks_like_loading(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    b.contains("loading") || b.contains("is being loaded") || b.contains("warming up")
}

/// Render a transport error for `LocalFailure.detail` with any attached request URL redacted
/// to scheme + host. This module is the one place in the backend that formats a `reqwest::Error`
/// **outside** `?`, so it must redact the same way: `normalize_base_url` strips only a trailing
/// `/v1`, so an endpoint pasted as `https://user:APIKEY@host` keeps its userinfo, and reqwest's
/// Display appends the whole URL. Routed through the central `From<reqwest::Error>` so this
/// bypass can never drift from it, then Displaying the inner error alone to keep the historical
/// detail wording (no `network error:` prefix in a local-failure detail).
fn redacted_transport_detail(e: reqwest::Error) -> String {
    match crate::error::Error::from(e) {
        crate::error::Error::Http(redacted) => redacted.to_string(),
        // Unreachable — the conversion always yields `Http`. Defensive, not a fallback.
        other => other.to_string(),
    }
}

/// Map a reqwest send/stream error to a typed failure. Connect refusal → dead host; a timeout →
/// `Timeout`; anything else transport-level is treated as a dead host (fallback-eligible).
fn classify_send_error(e: &reqwest::Error) -> LocalFailKind {
    if e.is_timeout() {
        LocalFailKind::Timeout
    } else {
        // is_connect() and the residual transport errors (reset, unreachable) all mean "the local
        // server isn't answering" — the dead-host arm.
        LocalFailKind::Refused
    }
}

// ---------------------------------------------------------------------------------------------
// SSE assembler — a pure, incremental parser. Feed raw bytes; get decodable events back.
// ---------------------------------------------------------------------------------------------

/// One decoded event from an OpenAI-compatible chat stream.
#[derive(Clone, Debug)]
pub enum SseEvent {
    /// A content delta (or, for a buffered pseudo-stream, the whole message content).
    Token(String),
    /// The model that actually served this response (first one seen wins).
    Model(String),
    /// A `finish_reason` — `"stop"`, `"length"`, etc. `"length"` means the token ceiling was hit.
    Finish(String),
    /// Token usage from the final chunk (present only when the server honours `include_usage`).
    Usage(Usage),
    /// A mid-stream `error` object's message, verbatim — the gateway classifies it.
    Error(String),
    /// The terminal `data: [DONE]` marker.
    Done,
}

/// Incremental SSE parser. Buffers raw bytes and decodes only complete `\n`-terminated lines, so a
/// multi-byte UTF-8 char split across two network chunks is never decoded in halves. Stateless
/// beyond the byte buffer and a `[DONE]` latch — no sockets, so every wire quirk is fixture-testable.
#[derive(Default)]
pub struct SseAssembler {
    buffer: Vec<u8>,
    saw_done: bool,
}

impl SseAssembler {
    /// Push newly-arrived bytes and return every event that can now be decoded. Incomplete trailing
    /// bytes stay buffered for the next call.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        for line in drain_lines(&mut self.buffer) {
            // Non-`data:` lines are SSE comments / keep-alives (e.g. `: ping`) — skip them.
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                self.saw_done = true;
                events.push(SseEvent::Done);
                continue;
            }
            if data.is_empty() {
                continue;
            }
            // A frame we can't parse as JSON is skipped, never fatal — an unknown server may emit a
            // stray keep-alive payload; the end-of-stream check catches a genuine breakdown.
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(err) = value.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("the model stream reported an error")
                    .to_string();
                events.push(SseEvent::Error(msg));
                continue;
            }
            if let Some(m) = value["model"].as_str() {
                events.push(SseEvent::Model(m.to_string()));
            }
            if let Some(reason) = value["choices"][0]["finish_reason"].as_str() {
                events.push(SseEvent::Finish(reason.to_string()));
            }
            let usage = parse_usage(&value);
            if usage.prompt_tokens.is_some() || usage.completion_tokens.is_some() {
                events.push(SseEvent::Usage(usage));
            }
            // Streaming servers put the delta under `delta.content`; a couple of "OpenAI-compatible"
            // servers stream a single buffered message under `message.content` — tolerate both.
            if let Some(tok) = value["choices"][0]["delta"]["content"].as_str() {
                events.push(SseEvent::Token(tok.to_string()));
            } else if let Some(tok) = value["choices"][0]["message"]["content"].as_str() {
                events.push(SseEvent::Token(tok.to_string()));
            }
        }
        events
    }

    /// How many bytes are buffered but not yet a complete line — the guard for an oversized line.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn saw_done(&self) -> bool {
        self.saw_done
    }
}

/// Extract token usage from a response/chunk (absent fields → None). Same shape as the cloud side;
/// a local server that reports no usage degrades the context meter honestly rather than lying zero.
fn parse_usage(value: &serde_json::Value) -> Usage {
    Usage {
        prompt_tokens: value["usage"]["prompt_tokens"].as_i64(),
        completion_tokens: value["usage"]["completion_tokens"].as_i64(),
        // A local server almost never reports a cost; kept for shape-parity with the cloud arm.
        cost: value["usage"]["cost"].as_f64(),
    }
}

/// Whether a stream that has stopped producing bytes ended cleanly or was cut off. An
/// OpenAI-compatible stream signals its end with `[DONE]`, a `finish_reason`, or a final usage
/// chunk; a stream that just stops with none of those was severed mid-flight (`MalformedStream`).
pub fn stream_ended_cleanly(saw_done: bool, saw_finish: bool, saw_usage: bool) -> bool {
    saw_done || saw_finish || saw_usage
}

// ---------------------------------------------------------------------------------------------
// Degenerate-stream guard — pure. Cheap insurance against a small quantised model looping.
// ---------------------------------------------------------------------------------------------

const GUARD_TAIL_BYTES: usize = 2048;
const GUARD_SCAN_WINDOW: usize = 1024;
const GUARD_MAX_PERIOD: usize = 256;
const GUARD_MIN_CYCLES: usize = 6;
const GUARD_MIN_COVER: usize = 768;
const GUARD_SCAN_EVERY: usize = 64;

/// Watches a rolling tail of recent output and trips when a short period repeats unmistakably
/// ("the the the…" or a looping phrase). Operates on bytes (a repeating multi-byte char has a
/// byte-period too), so there are no char-boundary hazards, and only re-scans every
/// `GUARD_SCAN_EVERY` appended bytes so the cost is amortised. Local-arm only: the strict-additivity
/// rule forbids adding abort behaviour to the cloud path, and token loops are a small-model pathology.
#[derive(Default)]
pub struct LoopGuard {
    tail: Vec<u8>,
    since_scan: usize,
    /// The previous streamed token and its consecutive-repeat count — the cheap fast path that kills
    /// a single-token loop (`LOOP_GUARD_SAME_TOKEN_RUN` identical deltas in a row) well before the
    /// byte-period detector below accumulates its cover.
    last_token: String,
    same_run: usize,
}

impl LoopGuard {
    /// Observe a new content chunk. Returns `true` once an obvious loop is detected.
    pub fn observe(&mut self, chunk: &str) -> bool {
        // Fast path: N identical consecutive tokens is almost certainly degenerate, and legitimate
        // output effectively never repeats one exact token 50 times in a row.
        if chunk == self.last_token {
            self.same_run += 1;
        } else {
            self.last_token.clear();
            self.last_token.push_str(chunk);
            self.same_run = 1;
        }
        if self.same_run >= tunables::LOOP_GUARD_SAME_TOKEN_RUN {
            return true;
        }

        self.tail.extend_from_slice(chunk.as_bytes());
        if self.tail.len() > GUARD_TAIL_BYTES {
            let cut = self.tail.len() - GUARD_TAIL_BYTES;
            self.tail.drain(..cut);
        }
        self.since_scan += chunk.len();
        if self.since_scan < GUARD_SCAN_EVERY {
            return false;
        }
        self.since_scan = 0;
        tail_is_looping(&self.tail)
    }
}

/// Pure loop detector over a byte tail: the smallest period `p` (1..=256) whose repetition covers
/// the last `GUARD_MIN_CYCLES` cycles AND at least `GUARD_MIN_COVER` bytes of the scan window trips it.
fn tail_is_looping(tail: &[u8]) -> bool {
    let n = tail.len();
    if n < GUARD_MIN_COVER {
        return false;
    }
    let window = &tail[n.saturating_sub(GUARD_SCAN_WINDOW)..];
    let w = window.len();
    for p in 1..=GUARD_MAX_PERIOD.min(w) {
        let period = &window[w - p..];
        let mut cycles = 0usize;
        let mut end = w;
        while end >= p && &window[end - p..end] == period {
            cycles += 1;
            end -= p;
        }
        if cycles >= GUARD_MIN_CYCLES && cycles * p >= GUARD_MIN_COVER {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// URL normalisation, /v1/models shape check, request body — all pure.
// ---------------------------------------------------------------------------------------------

/// Canonicalise a user-entered endpoint URL: trim, require an http(s) scheme, and strip a trailing
/// `/` and a trailing `/v1` so the stored base is bare (`http://localhost:11434`). The client
/// appends `/v1/...` itself, so a user who pastes `http://localhost:11434/v1/` and one who pastes
/// `http://localhost:11434` end up identical. The http-vs-https *posture* (loopback vs remote) is a
/// policy decision enforced by the caller, not here.
pub fn normalize_base_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(Error::Other(
            "the endpoint URL must start with http:// or https://".into(),
        ));
    }
    let t = trimmed.trim_end_matches('/');
    let t = t.strip_suffix("/v1").unwrap_or(t);
    Ok(t.trim_end_matches('/').to_string())
}

/// Whether a JSON body is a plausible OpenAI `/v1/models` response — the check that distinguishes a
/// real LLM server from any other web server that happens to answer on the port.
///
/// Two branches, trusting different evidence. An explicit `object == "list"` marker is itself the
/// proof that this is the OpenAI-compat route, so `data` only has to be list-SHAPED: an array or
/// `null` both mean "a list, possibly with nothing in it". Ollama builds that response from a Go
/// nil slice when its store is empty, and Go marshals a nil slice as `null` — so a freshly
/// installed one, before anything has been pulled into it, answers `{"object":"list","data":null}`
/// (measured against 0.33.0, 26-08-2026). LM Studio spells the same state `[]`, which was always
/// accepted; rejecting the other spelling made a working server undetectable at exactly the moment
/// a first-time user has no models yet.
///
/// Without the marker there is no such proof, so the second branch still demands a NON-EMPTY `data`
/// array of entries carrying string `id`s as its only evidence. That one must stay strict: a null
/// `data` under no marker is the ubiquitous `{"code":…,"msg":…,"data":null}` REST envelope, and
/// services in that idiom answer an unknown path with 200 — on 8080, next to llama-server.
pub fn is_models_list(value: &serde_json::Value) -> bool {
    if value.get("object").and_then(|o| o.as_str()) == Some("list") {
        return matches!(value.get("data"), Some(d) if d.is_array() || d.is_null());
    }
    value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            !arr.is_empty()
                && arr
                    .iter()
                    .all(|m| m.get("id").and_then(|i| i.as_str()).is_some())
        })
        .unwrap_or(false)
}

/// The model ids from a `/v1/models` body, in order (empty if the shape is unexpected).
pub fn models_from_list(value: &serde_json::Value) -> Vec<String> {
    value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Build the minimal OpenAI-compatible chat body. Deliberately carries no cloud-only fields — this
/// is the whole point of the separate local arm. Pure, so a test can prove the body stays clean.
pub fn chat_body(model: &str, messages: &[ChatMessage], stream: bool) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();
    let mut body = serde_json::json!({
        "model": model,
        "messages": msgs,
        "stream": stream,
    });
    if stream {
        // Ask for a final usage chunk so the context meter has real numbers when the server obliges.
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    body
}

// ---------------------------------------------------------------------------------------------
// Context-window ladder — the *selection* is pure; the probes that populate it are I/O.
// ---------------------------------------------------------------------------------------------

/// Where a discovered context window came from — surfaced so the UI can say "assumed" rather than
/// presenting a guess as measured. [`Self::is_proven`] is the line that matters.
///
/// The distinction the ladder got wrong: a model's TRAINED capacity and the window a server actually
/// LOADED are different physical quantities. `num_ctx` is a server setting; no model card can know
/// it. Ollama on a 7-8 GiB card silently loads 4096 for a model trained at 32768, and reading the
/// model's number as the server's number overstated it 8x — which is not a small error, because the
/// meter's numerator is capped by the truncation it is meant to warn about while the denominator is
/// fiction, so the alert can never fire (#792).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSource {
    /// llama-server `/slots` — the proven window of the actually-loaded model.
    Slots,
    /// Ollama `/api/ps` `context_length` — the proven window of the actually-loaded model. Ollama
    /// does not proxy llama-server's `/slots`, so before this rung existed there was no way to ask
    /// the runner PM recommends first what it had really loaded.
    LoadedModel,
    /// A `/v1/models` entry's metadata (`n_ctx_train` / `max_context_length`). The server's own
    /// claim, but about the MODEL rather than this load — an upper bound, not a measurement.
    ModelsMeta,
    /// The conservative fallback — nothing else was discoverable.
    Default,
}

impl WindowSource {
    /// Whether the server told us what it actually loaded, as opposed to PM inferring it. Only a
    /// proven window may be presented as a measurement.
    pub fn is_proven(self) -> bool {
        matches!(self, WindowSource::Slots | WindowSource::LoadedModel)
    }

    /// A stable identifier for the IPC boundary, so the UI never string-matches a Debug format.
    pub fn as_str(self) -> &'static str {
        match self {
            WindowSource::Slots => "slots",
            WindowSource::LoadedModel => "loaded_model",
            WindowSource::ModelsMeta => "models_meta",
            WindowSource::Default => "default",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub tokens: u32,
    pub source: WindowSource,
}

/// The window PM assumes when nothing is discoverable. A FLOOR, not a claim about any server's
/// default — assuming anything larger would make the context meter lie in the one direction that
/// costs a silently decapitated prompt.
///
/// It is deliberately not described as "Ollama's default" any more, because that was wrong.
/// Ollama 0.33's own help reads `OLLAMA_CONTEXT_LENGTH ... (default: 4k/32k/256k based on VRAM)`,
/// so 4096 is only its LOW tier — what a small card gets, and what this machine would have got
/// before the variable was set. A server on a 24 GiB card defaults to 32768 and one on 48 GiB to
/// 262144. The floor is still right; the reasoning behind it was not.
pub const DEFAULT_CONTEXT: u32 = 4096;

/// The ladder's preference order, as a pure choice over what each rung found: the two PROVEN rungs
/// first (`/slots`, then Ollama's `/api/ps`), then the server's own claim about the model, else the
/// conservative default.
///
/// The catalog rung is gone. It supplied the model's TRAINED capacity, which is a property of the
/// weights and not of this load, and it outranked [`DEFAULT_CONTEXT`] — so on Ollama, where neither
/// live rung answers, PM read 32768 off a model card while the server served 4096 (#792). The
/// comment on `DEFAULT_CONTEXT` argued for exactly this and was unreachable for the one server it
/// was written to protect against. A conservative floor makes PM compress a little early; a
/// confident guess makes it send prompts that are silently cut in half, and never say so.
pub fn pick_window(
    slots: Option<u32>,
    loaded_model: Option<u32>,
    models_meta: Option<u32>,
) -> WindowInfo {
    if let Some(tokens) = slots {
        return WindowInfo {
            tokens,
            source: WindowSource::Slots,
        };
    }
    if let Some(tokens) = loaded_model {
        return WindowInfo {
            tokens,
            source: WindowSource::LoadedModel,
        };
    }
    if let Some(tokens) = models_meta {
        return WindowInfo {
            tokens,
            source: WindowSource::ModelsMeta,
        };
    }
    WindowInfo {
        tokens: DEFAULT_CONTEXT,
        source: WindowSource::Default,
    }
}

// ---------------------------------------------------------------------------------------------
// The I/O edge — network entrypoints. Wired up by the gateway seam (#297 PR3).
// ---------------------------------------------------------------------------------------------

/// Probe an endpoint by GETting `{base}/v1/models` and shape-checking the response. Returns the
/// model ids on success. Bounded by a short per-request deadline so a wrong URL fails fast. Returns
/// the failure *shape* (never a bare error) so the Workbench UI can render "reachable / not" rather
/// than treating an unreachable server as a user-facing exception.
pub async fn probe(base_url: &str, token: Option<&str>) -> LocalResult<Vec<String>> {
    let url = format!("{base_url}/v1/models");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req
        .send()
        .await
        .map_err(|e| LocalFailure::new(classify_send_error(&e), redacted_transport_detail(e)))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(LocalFailure::new(
            classify_http(status.as_u16(), &body),
            crate::error::truncate_detail(&body),
        ));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| LocalFailure::new(LocalFailKind::UnrecognisedResponse, e.to_string()))?;
    if !is_models_list(&value) {
        return Err(LocalFailure::new(
            LocalFailKind::UnrecognisedResponse,
            "the endpoint answered but did not look like an OpenAI /v1/models list",
        ));
    }
    Ok(models_from_list(&value))
}

/// One progress tick from an Ollama `/api/pull` stream.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PullProgress {
    /// Ollama's status line for this tick ("pulling manifest", "downloading", "verifying sha256",
    /// "writing manifest", "success").
    pub status: String,
    /// Bytes fetched / total for the layer currently downloading, when Ollama reports them.
    pub completed_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// True on the terminal "success" line.
    pub done: bool,
}

/// Ask an Ollama server to download `model` into itself, streaming progress. Ollama is the only local
/// runner PM knows with a native pull API (LM Studio / llama-server have none — the tab shows a
/// copy-paste command for those). PM downloads NOTHING itself and proxies nothing: this asks the
/// user's own server to fetch the weights from wherever it is configured to. `base_url` is the
/// normalised endpoint (no `/v1`); Ollama's native route is `{base_url}/api/pull`.
pub async fn pull_ollama_model<F>(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    mut on_progress: F,
) -> LocalResult<()>
where
    F: FnMut(PullProgress),
{
    // Both keys on purpose: current Ollama reads `model`, older builds read `name`.
    let body = serde_json::json!({ "model": model, "name": model, "stream": true });
    let url = format!("{base_url}/api/pull");
    let mut req = client_for(base_url).post(&url).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req
        .send()
        .await
        .map_err(|e| LocalFailure::new(classify_send_error(&e), redacted_transport_detail(e)))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(LocalFailure::new(
            classify_http(status.as_u16(), &body),
            crate::error::truncate_detail(&body),
        ));
    }

    // Ollama streams newline-delimited JSON objects. Buffer bytes and parse each complete line; a
    // stalled (open-but-idle) stream is bounded by a generous per-chunk deadline so a dead download
    // can't hang the tab forever. The deadline is STATUS-AWARE: the verify/write phases emit one
    // line per layer and then hash in silence — several minutes for the catalogue's largest rows —
    // so only those named phases get `PULL_VERIFY_STALL_TIMEOUT`; a silent download is still a
    // stall at the ordinary bound. (Sized against the catalogue's largest tagged artifact, 44 GiB,
    // on a spinning disk — the miss that used to call an essentially-complete pull "stalled".)
    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut last_status = String::new();
    loop {
        let stall = if last_status.starts_with("verifying") || last_status.starts_with("writing") {
            tunables::PULL_VERIFY_STALL_TIMEOUT
        } else {
            tunables::PULL_STALL_TIMEOUT
        };
        let next = match tokio::time::timeout(stall, stream.next()).await {
            Ok(next) => next,
            Err(_elapsed) => {
                return Err(LocalFailure::new(
                    LocalFailKind::Timeout,
                    "the download stalled — no progress from the server",
                ));
            }
        };
        let Some(chunk) = next else {
            break; // the byte stream ended
        };
        let bytes = chunk.map_err(|e| LocalFailure::new(classify_send_error(&e), e.to_string()))?;
        buf.extend_from_slice(&bytes);
        // The endpoint is untrusted: a broken/hostile server dribbling newline-free bytes would reset
        // the stall timer each chunk and grow `buf` without bound. Cap it like the SSE line assembler.
        if buf.len() > MAX_SSE_LINE_BYTES {
            return Err(LocalFailure::new(
                LocalFailKind::MalformedStream,
                "the download stream sent an oversized line",
            ));
        }
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let text = String::from_utf8_lossy(&line);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
                continue; // a partial or non-JSON keep-alive line — wait for more bytes
            };
            // Ollama reports a pull failure as an {"error": "..."} line, not an HTTP error.
            if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                return Err(LocalFailure::new(
                    LocalFailKind::MalformedStream,
                    crate::error::truncate_detail(err),
                ));
            }
            let status = v
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string();
            last_status = status.clone();
            let done = status == "success";
            on_progress(PullProgress {
                status,
                completed_bytes: v.get("completed").and_then(serde_json::Value::as_u64),
                total_bytes: v.get("total").and_then(serde_json::Value::as_u64),
                done,
            });
            if done {
                return Ok(()); // terminal line — don't wait a stall-timeout for the server to hang up
            }
        }
    }
    Err(LocalFailure::new(
        LocalFailKind::MalformedStream,
        "the download ended without a success line",
    ))
}

/// Stream a chat completion from a local endpoint. `on_token` is called with each content delta.
/// Classifies every failure structurally (`LocalFailKind`) and aborts an obvious token loop.
pub async fn stream_chat<F>(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    messages: &[ChatMessage],
    mut on_token: F,
) -> LocalResult<Completion>
where
    F: FnMut(&str),
{
    let body = chat_body(model, messages, true);
    let url = format!("{base_url}/v1/chat/completions");
    let mut req = client_for(base_url)
        .post(&url)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req
        .send()
        .await
        .map_err(|e| LocalFailure::new(classify_send_error(&e), redacted_transport_detail(e)))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(LocalFailure::new(
            classify_http(status.as_u16(), &body),
            crate::error::truncate_detail(&body),
        ));
    }

    let mut full = String::new();
    let mut served: Option<String> = None;
    let mut usage = Usage::default();
    let mut truncated = false;
    let mut saw_finish = false;
    let mut saw_usage = false;
    let mut assembler = SseAssembler::default();
    let mut guard = LoopGuard::default();
    let mut stream = response.bytes_stream();
    let mut got_first_token = false;

    loop {
        // Two-phase timeout: a generous deadline for the FIRST token — absorbing a silent cold model
        // load (Ollama / LM Studio JIT-load and stream nothing until the first token) — then a short
        // inter-token deadline once streaming has started. Any received bytes, including an SSE
        // keepalive ping, arrive as a chunk and reset the timer; the inter-token window is set above
        // llama-server's 30 s ping cadence so a ping always resets it before it can fire.
        let deadline = if got_first_token {
            tunables::INTER_TOKEN_TIMEOUT
        } else {
            tunables::TIME_TO_FIRST_TOKEN_TIMEOUT
        };
        let next = match tokio::time::timeout(deadline, stream.next()).await {
            Ok(next) => next,
            Err(_elapsed) => {
                let detail = if got_first_token {
                    "the model stream stalled between tokens"
                } else {
                    "the model produced no first token before the deadline"
                };
                return Err(LocalFailure::new(LocalFailKind::Timeout, detail));
            }
        };
        let Some(chunk) = next else {
            break; // the byte stream ended
        };
        let bytes = chunk.map_err(|e| LocalFailure::new(classify_send_error(&e), e.to_string()))?;
        for event in assembler.feed(&bytes) {
            match event {
                SseEvent::Token(tok) => {
                    got_first_token = true;
                    full.push_str(&tok);
                    if full.len() > MAX_REPLY_BYTES {
                        return Err(LocalFailure::new(
                            LocalFailKind::ReplyTooLarge,
                            "the model reply exceeded the size limit",
                        ));
                    }
                    if guard.observe(&tok) {
                        return Err(LocalFailure::new(
                            LocalFailKind::DegenerateStream,
                            "the model stream fell into an obvious token loop",
                        ));
                    }
                    on_token(&tok);
                }
                SseEvent::Model(m) => {
                    if served.is_none() {
                        served = Some(m);
                    }
                }
                SseEvent::Finish(reason) => {
                    saw_finish = true;
                    if reason == "length" {
                        truncated = true;
                    }
                }
                SseEvent::Usage(u) => {
                    saw_usage = true;
                    usage = u;
                }
                SseEvent::Error(msg) => {
                    return Err(LocalFailure::new(LocalFailKind::MalformedStream, msg));
                }
                SseEvent::Done => {
                    return Ok(Completion {
                        text: full,
                        model: served,
                        usage,
                        truncated,
                    });
                }
            }
        }
        if assembler.buffered_len() > MAX_SSE_LINE_BYTES {
            return Err(LocalFailure::new(
                LocalFailKind::MalformedStream,
                "the model stream sent an oversized line",
            ));
        }
    }

    // The byte stream ended without a `[DONE]`. Accept it only if the model gave some clean end
    // signal (a finish_reason or a usage chunk); otherwise the connection was cut mid-flight.
    if stream_ended_cleanly(assembler.saw_done(), saw_finish, saw_usage) {
        Ok(Completion {
            text: full,
            model: served,
            usage,
            truncated,
        })
    } else {
        Err(LocalFailure::new(
            LocalFailKind::MalformedStream,
            "the model stream ended without a completion marker",
        ))
    }
}

/// A single non-streaming chat completion — background work wants the whole answer at once.
pub async fn complete(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    messages: &[ChatMessage],
) -> LocalResult<Completion> {
    let body = chat_body(model, messages, false);
    let url = format!("{base_url}/v1/chat/completions");
    let mut req = client_for(base_url)
        .post(&url)
        .timeout(tunables::BACKGROUND_TOTAL_TIMEOUT)
        .json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req
        .send()
        .await
        .map_err(|e| LocalFailure::new(classify_send_error(&e), redacted_transport_detail(e)))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(LocalFailure::new(
            classify_http(status.as_u16(), &body),
            crate::error::truncate_detail(&body),
        ));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| LocalFailure::new(LocalFailKind::UnrecognisedResponse, e.to_string()))?;
    let text = value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalFailure::new(
                LocalFailKind::UnrecognisedResponse,
                "the local response had no message content",
            )
        })?;
    Ok(Completion {
        text,
        model: value["model"].as_str().map(str::to_string),
        usage: parse_usage(&value),
        truncated: value["choices"][0]["finish_reason"].as_str() == Some("length"),
    })
}

/// Discover the model's context window via the ladder, populating `pick_window` from live probes.
/// `/slots` (llama-server) is tried first for the proven window; `/v1/models` metadata second; the
/// catalog hook is filled in by #296 (PR4). A conservative default is never an error — it is the
/// bottom rung by design.
pub async fn probe_window(base_url: &str, model: &str, token: Option<&str>) -> WindowInfo {
    let slots = probe_slots_ctx(base_url, token).await;
    // Each rung costs a request, so ask only while the answer is still unknown.
    let loaded = if slots.is_none() {
        probe_loaded_ctx(base_url, model, token).await
    } else {
        None
    };
    let models_meta = if slots.is_none() && loaded.is_none() {
        probe_models_ctx(base_url, model, token).await
    } else {
        None
    };
    pick_window(slots, loaded, models_meta)
}

/// The two PROVEN rungs only — `/slots`, then Ollama's `/api/ps` — or `None` when neither answered.
///
/// [`probe_window`] can never return `None`: it falls through to [`DEFAULT_CONTEXT`] by design,
/// because a caller sizing a prompt always needs a number. A caller that is merely LOOKING needs the
/// opposite guarantee — it must be able to learn nothing and write nothing, rather than record PM's
/// own floor into a cache that other paths read as evidence about the user's server.
///
/// Neither rung cares who loaded the model, which is what makes a passive caller worth having: the
/// answer is there whenever something is resident, whoever put it there.
pub async fn probe_proven_window(
    base_url: &str,
    model: &str,
    token: Option<&str>,
) -> Option<WindowInfo> {
    if let Some(tokens) = probe_slots_ctx(base_url, token).await {
        return Some(WindowInfo {
            tokens,
            source: WindowSource::Slots,
        });
    }
    probe_loaded_ctx(base_url, model, token)
        .await
        .map(|tokens| WindowInfo {
            tokens,
            source: WindowSource::LoadedModel,
        })
}

/// Ollama `/api/ps` reports the RESIDENT models and, for each, the `context_length` it was actually
/// loaded with — the same number `ollama ps` prints and the one llama-server was launched with.
///
/// This is the truth the ladder was missing. Ollama does not proxy `/slots`, and its `/v1/models`
/// entries carry only `id`/`object`/`created`/`owned_by`, so both live rungs came back empty and PM
/// fell through to a guess. Returns `None` when nothing is resident, which is correct rather than a
/// failure: there is no served window until something is loaded, and the ladder should say so by
/// falling to its floor.
///
/// Matched on the model id so a machine serving several models cannot answer for the wrong one.
async fn probe_loaded_ctx(base_url: &str, model: &str, token: Option<&str>) -> Option<u32> {
    let url = format!("{base_url}/api/ps");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    loaded_ctx_from_ps(&value, model)
}

/// The `context_length` `/api/ps` reports for `model`, if it is resident. Pure, so the matching —
/// including the "several models loaded, answer for the right one" case — is testable without a
/// server.
pub fn loaded_ctx_from_ps(value: &serde_json::Value, model: &str) -> Option<u32> {
    value
        .get("models")?
        .as_array()?
        .iter()
        .find(|m| {
            // Ollama echoes the name as pulled. `model` is what PM sends on the wire, so an exact
            // match is the honest test; anything looser risks answering for a different load.
            m.get("name").and_then(|n| n.as_str()) == Some(model)
                || m.get("model").and_then(|n| n.as_str()) == Some(model)
        })?
        .get("context_length")?
        .as_u64()
        .and_then(|c| u32::try_from(c).ok())
        .filter(|c| *c > 0)
}

/// One model in an Ollama server's own store, from `/api/tags` — whether or not it is loaded.
///
/// The point of this rung is the **byte size**. PM otherwise scores a model the endpoint serves with
/// `fit::fit`, which picks the best quantization that fits the machine's budget — correct advice for
/// something you have not downloaded yet, and fiction for something you already have. Measured on a
/// real setup: PM believed a served Qwen2.5-7B was Q8_0 at 10.04 GB while the user's actual file was
/// Q5_K_M at 5.44 GB, and a served gemma-3-4b was 9.21 GB against a real 3.34 GB.
///
/// Worse, that fiction moves the wrong way: a bigger budget lets `fit` pick a higher quant, so
/// FREEING memory made PM's estimate of an already-downloaded model grow. This route is how PM stops
/// guessing at a file that is sitting right there.
#[derive(Debug, Clone, PartialEq)]
pub struct OllamaTag {
    /// The tag exactly as pulled. Byte-for-byte the id `/v1/models` reports and the one `/api/ps`
    /// echoes, so the listings key against each other with no normalisation.
    pub name: String,
    /// The whole model's bytes on disk as the server measures them — weights AND any projector, so
    /// a caller must put it in the weight term with the projector at zero rather than adding both.
    pub size_bytes: u64,
    /// `details.quantization_level`, or `None`. Ollama spells "I could not read one" as the literal
    /// string `"unknown"` (measured: it does that for `hf.co/ggml-org/gemma-3-4b-it-GGUF` while
    /// reporting `Q5_K_M` for a bartowski build pulled the same day), and that sentinel is folded
    /// into `None` here so a caller cannot mistake it for a quantization it merely lacks a size for.
    /// Untrusted content — the server read it out of a file — so bound it before display.
    pub quant: Option<String>,
}

/// Ollama's own inventory: every model in its store, loaded or not, with the real byte size of each.
///
/// Best-effort exactly like `/slots` and `/api/ps`: a non-Ollama 404s and the caller falls through to
/// the catalogue estimate. A 404 here is not evidence about the server's health and must never be
/// scored as one.
///
/// `None` means "did not answer"; `Some(vec![])` means "an Ollama with nothing pulled". A caller that
/// flattens the two will report an empty store for a server that is not an Ollama at all.
pub async fn ollama_tags(base_url: &str, token: Option<&str>) -> Option<Vec<OllamaTag>> {
    let url = format!("{base_url}/api/tags");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    tags_from_json(&value)
}

/// Parse an `/api/tags` body. Pure, so the three shapes that matter are testable without a server.
///
/// The check has to EARN `Some(vec![])`, since that value asserts "this is an Ollama and it is
/// empty": `models` must be an array, and every entry in it must be an object carrying a string
/// `name`. Deliberately cannot reuse [`is_models_list`], which is written for `/v1/models` — whose
/// empty spelling is `{"object":"list","data":null}` — and would reject `{"models":[]}` outright.
pub fn tags_from_json(value: &serde_json::Value) -> Option<Vec<OllamaTag>> {
    let entries = value.get("models")?.as_array()?;
    if !entries
        .iter()
        .all(|m| m.get("name").and_then(|n| n.as_str()).is_some())
    {
        return None;
    }
    Some(
        entries
            .iter()
            .filter_map(|m| {
                Some(OllamaTag {
                    name: m.get("name")?.as_str()?.to_string(),
                    size_bytes: m.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                    quant: m
                        .get("details")
                        .and_then(|d| d.get("quantization_level"))
                        .and_then(|q| q.as_str())
                        .map(str::trim)
                        .filter(|q| !q.is_empty() && !q.eq_ignore_ascii_case("unknown"))
                        .map(str::to_string),
                })
            })
            .collect(),
    )
}

/// llama-server `/slots` returns per-slot `n_ctx` for the loaded model. 404/501/timeout → None (not
/// a llama-server, or slots disabled) — the ladder falls through, by design.
async fn probe_slots_ctx(base_url: &str, token: Option<&str>) -> Option<u32> {
    let url = format!("{base_url}/slots");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    // `/slots` is an array of slot objects; take the first slot's n_ctx.
    value
        .as_array()
        .and_then(|slots| slots.first())
        .and_then(|slot| slot.get("n_ctx"))
        .and_then(|c| c.as_u64())
        .and_then(|c| u32::try_from(c).ok())
}

/// A `/v1/models` entry may carry the training/max context under a couple of server-specific keys.
async fn probe_models_ctx(base_url: &str, model: &str, token: Option<&str>) -> Option<u32> {
    let url = format!("{base_url}/v1/models");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let response = req.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    let entry = value
        .get("data")?
        .as_array()?
        .iter()
        .find(|m| m.get("id").and_then(|i| i.as_str()) == Some(model))?;
    // llama-server exposes `meta.n_ctx_train`; LM Studio exposes `max_context_length`.
    let meta_ctx = entry
        .get("meta")
        .and_then(|meta| meta.get("n_ctx_train"))
        .and_then(|c| c.as_u64());
    let lm_ctx = entry.get("max_context_length").and_then(|c| c.as_u64());
    meta_ctx
        .or(lm_ctx)
        .and_then(|c| u32::try_from(c).ok())
        .filter(|&c| c > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    /// Collect every token an assembler decodes from a whole transcript fed in one shot.
    fn collect(events: &[SseEvent]) -> (String, Option<String>, Option<Usage>, bool, bool) {
        let mut text = String::new();
        let mut model = None;
        let mut usage = None;
        let mut finished = false;
        let mut done = false;
        for e in events {
            match e {
                SseEvent::Token(t) => text.push_str(t),
                SseEvent::Model(m) => model = Some(m.clone()),
                SseEvent::Usage(u) => usage = Some(*u),
                SseEvent::Finish(_) => finished = true,
                SseEvent::Done => done = true,
                SseEvent::Error(_) => {}
            }
        }
        (text, model, usage, finished, done)
    }

    // Representative transcripts modelled on each server's documented OpenAI-compatible output.
    // Synthetic — the live-rig checklist still owes a diff against one real capture per server; the
    // parser must handle all three plus the buffered/keepalive/truncated variants below.

    const OLLAMA: &[u8] = b"data: {\"model\":\"llama3.2\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\ndata: {\"model\":\"llama3.2\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\ndata: {\"model\":\"llama3.2\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

    const LM_STUDIO: &[u8] = b"data: {\"model\":\"qwen2.5-7b-instruct\",\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\ndata: {\"model\":\"qwen2.5-7b-instruct\",\"choices\":[{\"delta\":{\"content\":\" there\"},\"finish_reason\":null}]}\n\ndata: {\"model\":\"qwen2.5-7b-instruct\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";

    const LLAMA_SERVER: &[u8] = b": keep-alive\n\ndata: {\"model\":\"local\",\"choices\":[{\"delta\":{\"content\":\"one\"}}]}\n\ndata: {\"model\":\"local\",\"choices\":[{\"delta\":{\"content\":\" two\"}}]}\n\ndata: {\"model\":\"local\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";

    #[test]
    fn parses_an_ollama_transcript() {
        let mut a = SseAssembler::default();
        let (text, model, usage, finished, done) = collect(&a.feed(OLLAMA));
        assert_eq!(text, "Hello world");
        assert_eq!(model.as_deref(), Some("llama3.2"));
        assert!(usage.is_none(), "Ollama's default stream reports no usage");
        assert!(finished && done);
    }

    #[test]
    fn parses_an_lm_studio_transcript_with_usage() {
        let mut a = SseAssembler::default();
        let (text, model, usage, finished, done) = collect(&a.feed(LM_STUDIO));
        assert_eq!(text, "Hi there");
        assert_eq!(model.as_deref(), Some("qwen2.5-7b-instruct"));
        let u = usage.expect("LM Studio reported usage");
        assert_eq!(u.prompt_tokens, Some(11));
        assert_eq!(u.completion_tokens, Some(2));
        assert!(finished && done);
    }

    #[test]
    fn parses_a_llama_server_transcript_ignoring_keepalive() {
        let mut a = SseAssembler::default();
        let (text, model, usage, _finished, done) = collect(&a.feed(LLAMA_SERVER));
        assert_eq!(
            text, "one two",
            "the `: keep-alive` comment must be skipped"
        );
        assert_eq!(model.as_deref(), Some("local"));
        assert_eq!(usage.unwrap().prompt_tokens, Some(5));
        assert!(done);
    }

    #[test]
    fn a_whole_stream_in_one_chunk_parses_identically_to_byte_by_byte() {
        // Buffered framing (one big chunk) vs the meanest possible chunking (one byte at a time)
        // must yield the same tokens — the incremental buffer is the thing under test.
        let mut whole = SseAssembler::default();
        let (whole_text, ..) = collect(&whole.feed(OLLAMA));

        let mut drip = SseAssembler::default();
        let mut drip_text = String::new();
        for b in OLLAMA {
            for e in drip.feed(&[*b]) {
                if let SseEvent::Token(t) = e {
                    drip_text.push_str(&t);
                }
            }
        }
        assert_eq!(whole_text, drip_text);
        assert_eq!(drip_text, "Hello world");
    }

    #[test]
    fn a_stream_that_ends_without_done_or_finish_is_not_clean() {
        // Only content deltas arrive, then the socket closes — no [DONE], no finish_reason, no usage.
        let cut =
            b"data: {\"model\":\"m\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
        let mut a = SseAssembler::default();
        let (text, _m, _u, finished, done) = collect(&a.feed(cut));
        assert_eq!(text, "partial");
        assert!(!done && !finished);
        assert!(
            !stream_ended_cleanly(a.saw_done(), finished, false),
            "a severed stream must be classed malformed"
        );
    }

    #[test]
    fn a_mid_stream_error_object_is_surfaced() {
        let err = b"data: {\"error\":{\"message\":\"upstream provider is down\",\"code\":502}}\n\n";
        let mut a = SseAssembler::default();
        let events = a.feed(err);
        assert!(
            matches!(&events[0], SseEvent::Error(m) if m.contains("upstream provider is down"))
        );
    }

    #[test]
    fn classify_http_maps_status_and_a_loading_body() {
        assert_eq!(
            classify_http(503, "model is loading, please wait"),
            LocalFailKind::ModelLoading,
            "a warming-up server is alive, not a strike"
        );
        assert_eq!(
            classify_http(503, "gateway exploded"),
            LocalFailKind::ServerError(503),
            "a 503 without a loading body is a real server error"
        );
        assert_eq!(classify_http(500, ""), LocalFailKind::ServerError(500));
        assert_eq!(classify_http(404, ""), LocalFailKind::ClientError(404));
        assert_eq!(
            classify_http(401, "bad token"),
            LocalFailKind::ClientError(401)
        );
    }

    #[test]
    fn loop_guard_trips_on_a_single_token_cycle() {
        let mut g = LoopGuard::default();
        let mut tripped = false;
        // "a" repeated far past the cover threshold.
        for _ in 0..2000 {
            if g.observe("a") {
                tripped = true;
                break;
            }
        }
        assert!(tripped, "a degenerate single-char loop must trip");
    }

    #[test]
    fn loop_guard_trips_on_a_repeating_phrase() {
        let mut g = LoopGuard::default();
        let mut tripped = false;
        for _ in 0..400 {
            if g.observe("the cat ") {
                tripped = true;
                break;
            }
        }
        assert!(tripped, "a repeating phrase is still a loop");
    }

    #[test]
    fn loop_guard_leaves_legitimate_repetition_alone() {
        // A markdown table / indented code block repeats *structure* but not *content* — every row
        // carries different values, so there is no short exact period across the window. This is the
        // real distinction: a broken model emits the SAME bytes over and over (which must trip), a
        // healthy one emits a varying stream through a repeated template (which must not).
        let mut g = LoopGuard::default();
        let mut tripped = false;
        for i in 0..300 {
            let row = format!("| row {i} | value {} | note-{i:x} |\n", i * 7);
            if g.observe(&row) {
                tripped = true;
                break;
            }
        }
        assert!(
            !tripped,
            "structurally-repeated but varying rows are not a token loop"
        );
    }

    #[test]
    fn loop_guard_fast_path_trips_on_a_short_repeated_token() {
        // 50 identical multi-char tokens = 300 bytes, below the period detector's 768-byte cover —
        // the same-token-run fast path must still trip, and exactly at the threshold.
        let mut g = LoopGuard::default();
        let mut tripped_at = None;
        for i in 0..(tunables::LOOP_GUARD_SAME_TOKEN_RUN + 5) {
            if g.observe(" hello") {
                tripped_at = Some(i + 1);
                break;
            }
        }
        assert_eq!(tripped_at, Some(tunables::LOOP_GUARD_SAME_TOKEN_RUN));
    }

    #[test]
    fn loop_guard_tail_stays_bounded() {
        let mut g = LoopGuard::default();
        for _ in 0..10_000 {
            g.observe("abcdefghij");
        }
        assert!(
            g.tail.len() <= GUARD_TAIL_BYTES,
            "the rolling tail must not grow without bound"
        );
    }

    #[test]
    fn normalize_base_url_strips_trailing_slash_and_v1() {
        assert_eq!(
            normalize_base_url("http://localhost:11434/v1/").unwrap(),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434").unwrap(),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("  https://box.local:8080/v1  ").unwrap(),
            "https://box.local:8080"
        );
        assert!(
            normalize_base_url("localhost:11434").is_err(),
            "a scheme is required"
        );
        assert!(normalize_base_url("ftp://x").is_err());
    }

    /// The one leak class the central `From<reqwest::Error>` does not cover on its own: these
    /// entrypoints format the raw error into `LocalFailure.detail` instead of going through `?`.
    /// `normalize_base_url` strips only a trailing `/v1`, so userinfo survives into the endpoint
    /// string and would otherwise be Displayed straight into the UI. Loopback port 1 refuses
    /// instantly, so this needs no DNS and no server.
    #[test]
    fn a_userinfo_base_url_never_reaches_a_local_failure_detail() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(probe("https://user:APIKEY@127.0.0.1:1", None))
            .expect_err("nothing listens on loopback port 1");
        assert!(
            !err.detail.contains("APIKEY"),
            "userinfo leaked into the detail: {}",
            err.detail
        );
        assert!(
            err.detail.contains("127.0.0.1"),
            "the host is the diagnosable part and should survive: {}",
            err.detail
        );
    }

    #[test]
    fn a_tags_listing_carries_the_real_file_size_and_folds_ollamas_unknown_sentinel() {
        // Byte-exact from a live server (Ollama 0.33.0, 30-08-2026). The two entries differ in
        // exactly the way that matters: the same server reports a real quantization for one repo and
        // the literal string "unknown" for another pulled the same day.
        let body: serde_json::Value = serde_json::from_str(
            r#"{"models":[
                {"name":"hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M","size":3341010115,
                 "details":{"parameter_size":"3.88B","quantization_level":"unknown"}},
                {"name":"hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M","size":5444833987,
                 "details":{"parameter_size":"7.62B","quantization_level":"Q5_K_M"}}
            ]}"#,
        )
        .unwrap();
        let tags = tags_from_json(&body).expect("a tags listing");
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].size_bytes, 3_341_010_115);
        // "unknown" is an ABSENCE, not a label. Passed through it became "PM doesn't have a size for
        // the UNKNOWN quantization yet" — asserting knowledge the server had just disclaimed.
        assert_eq!(tags[0].quant, None);
        assert_eq!(tags[1].quant.as_deref(), Some("Q5_K_M"));
    }

    #[test]
    fn an_ollama_with_nothing_pulled_is_not_the_same_as_no_ollama_at_all() {
        // `Some(vec![])` says "this IS an Ollama and it is empty" — a first-time installer's exact
        // state. `None` says nothing answered. A caller that flattens the two reports an empty store
        // for a server that is not an Ollama.
        let empty: serde_json::Value = serde_json::from_str(r#"{"models":[]}"#).unwrap();
        assert_eq!(tags_from_json(&empty), Some(Vec::new()));

        // Not a tags listing: no `models`, or a `models` whose entries are not named models.
        for body in [
            r#"{"object":"list","data":[]}"#,
            r#"{"models":[1,2,3]}"#,
            r#"{}"#,
        ] {
            let v: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(tags_from_json(&v), None, "{body}");
        }

        // And this shape must stay distinct from the `/v1/models` one, whose empty spelling is
        // `{"object":"list","data":null}` — `is_models_list` would reject `{"models":[]}` outright.
        assert!(!is_models_list(&empty));
    }

    #[test]
    fn probe_shape_accepts_the_three_servers_and_rejects_a_web_page() {
        let openai = serde_json::json!({"object":"list","data":[{"id":"llama3.2"},{"id":"qwen"}]});
        assert!(is_models_list(&openai));
        assert_eq!(models_from_list(&openai), vec!["llama3.2", "qwen"]);

        // A runner with nothing downloaded into it yet is still a runner. Byte-exact from a live
        // server: Ollama 0.33.0, zero models pulled, GET /v1/models, 26-08-2026. Its handler never
        // appends to a Go `var data []Model`, and Go marshals a nil slice as `null` — so this is
        // the shape EVERY first-time Ollama user's server returns, before they have pulled
        // anything. Rejecting it made a running server report as "No local server found".
        let ollama_empty = serde_json::json!({"object":"list","data":null});
        assert!(is_models_list(&ollama_empty));
        assert!(models_from_list(&ollama_empty).is_empty());
        // LM Studio's spelling of that same state, which was always accepted.
        assert!(is_models_list(
            &serde_json::json!({"object":"list","data":[]})
        ));

        // A minimal OpenAI-compat shim that omits the object=="list" marker. Not attributed to any
        // named runner: all three measured ones send the marker, and a fixture that claims a
        // producer nobody has seen is a test passing for free. The fall-through branch is real and
        // wants coverage; who emits it is the part we don't know.
        let bare = serde_json::json!({"data":[{"id":"phi3","object":"model"}]});
        assert!(is_models_list(&bare));

        // A random web server / HTML-as-JSON must be rejected.
        assert!(!is_models_list(&serde_json::json!({"message":"Not Found"})));
        assert!(!is_models_list(&serde_json::json!({"data":[]})));
        assert!(!is_models_list(
            &serde_json::json!({"data":[{"name":"no-id"}]})
        ));
        assert!(!is_models_list(&serde_json::json!("<html>hi</html>")));
        // The trap in accepting a null `data`: it is list-shaped ONLY under the object=="list"
        // marker. The ubiquitous Go/Java REST envelope has a null `data` and no marker, and a
        // service in that idiom will happily 200 an unknown path — on 8080, llama-server's port.
        assert!(!is_models_list(
            &serde_json::json!({"code":404,"msg":"not found","data":null})
        ));
        // A missing `data` key stays rejected too: no measured server omits it, and the marker
        // alone is thinner evidence than we need on a contested port.
        assert!(!is_models_list(&serde_json::json!({"object":"list"})));
    }

    #[test]
    fn window_ladder_prefers_what_the_server_actually_loaded() {
        // Both proven rungs outrank the server's claim about the model.
        assert_eq!(
            pick_window(Some(8192), Some(4096), Some(32768)),
            WindowInfo {
                tokens: 8192,
                source: WindowSource::Slots
            }
        );
        assert_eq!(
            pick_window(None, Some(4096), Some(32768)),
            WindowInfo {
                tokens: 4096,
                source: WindowSource::LoadedModel
            },
            "a served window of 4096 must beat a trained capacity of 32768 — they are different \
             quantities, and only one of them describes this load"
        );
        assert_eq!(
            pick_window(None, None, Some(32768)).source,
            WindowSource::ModelsMeta
        );

        // Nothing discoverable falls to the honest floor, NOT to a model card's number. This is the
        // rung the catalogue used to sit above, and doing so overstated Ollama 8x (#792).
        let fallback = pick_window(None, None, None);
        assert_eq!(fallback.tokens, DEFAULT_CONTEXT);
        assert_eq!(fallback.source, WindowSource::Default);
        assert!(!fallback.source.is_proven());
        assert!(!WindowSource::ModelsMeta.is_proven());
        assert!(WindowSource::Slots.is_proven());
        assert!(WindowSource::LoadedModel.is_proven());
    }

    #[test]
    fn api_ps_answers_for_the_right_load_or_not_at_all() {
        // Byte-shaped after a live Ollama 0.33.0 answering with one model resident, 27-08-2026.
        let ps = serde_json::json!({"models":[
            {"name":"qwen2.5:7b-instruct-q4_K_M","model":"qwen2.5:7b-instruct-q4_K_M",
             "context_length":4096,"size_vram":4748056984u64},
            {"name":"llama3.2:1b","model":"llama3.2:1b","context_length":32768}
        ]});
        assert_eq!(
            loaded_ctx_from_ps(&ps, "qwen2.5:7b-instruct-q4_K_M"),
            Some(4096)
        );
        // A machine serving several models must never answer for the wrong one.
        assert_eq!(loaded_ctx_from_ps(&ps, "llama3.2:1b"), Some(32768));
        assert_eq!(loaded_ctx_from_ps(&ps, "mistral:7b"), None);

        // Nothing resident is not a failure — there is no served window yet, and the ladder should
        // fall to its floor rather than invent one. This is what an idle Ollama returns.
        assert_eq!(
            loaded_ctx_from_ps(&serde_json::json!({"models":[]}), "qwen2.5:7b"),
            None
        );
        // A non-Ollama server's 200 must not be mined for a number.
        assert_eq!(
            loaded_ctx_from_ps(
                &serde_json::json!({"object":"list","data":[]}),
                "qwen2.5:7b"
            ),
            None
        );
        // A zero is a missing value, not a window.
        assert_eq!(
            loaded_ctx_from_ps(
                &serde_json::json!({"models":[{"name":"m","context_length":0}]}),
                "m"
            ),
            None
        );
    }

    #[test]
    fn request_body_carries_no_cloud_only_fields() {
        let body = chat_body("llama3.2", &[msg("user", "hi")], true);
        // The model is a plain string (never a `models` fallback array), messages are plain strings,
        // and NONE of OpenRouter's body fields appear.
        assert_eq!(body["model"], "llama3.2");
        assert!(body.get("models").is_none(), "no fallback-model array");
        assert!(body.get("provider").is_none(), "no ZDR provider pin");
        let serialized = serde_json::to_string(&body).unwrap();
        assert!(
            !serialized.contains("cache_control"),
            "no prompt-cache breakpoints"
        );
        assert!(!serialized.contains("zdr"));
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(body["stream_options"]["include_usage"], true);

        // The non-streaming body omits stream_options too.
        let buffered = chat_body("m", &[msg("user", "x")], false);
        assert_eq!(buffered["stream"], false);
        assert!(buffered.get("stream_options").is_none());
    }
}
