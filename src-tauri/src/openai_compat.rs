// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An OpenAI-compatible chat client for a user-configured **local** endpoint — Ollama
//! (`:11434/v1`), LM Studio (`:1234/v1`), llama-server (`:8080/v1`), or anything that speaks
//! `/v1/chat/completions` SSE. This is the LOCAL arm of the provider seam (#297); the cloud arm
//! stays in [`crate::openrouter`]. The request body here is deliberately **minimal** — plain
//! `model` + `messages` + `stream` — carrying NONE of OpenRouter's body fields (no `provider`
//! ZDR pin, no `cache_control`, no `models` fallback array), because a local server understands
//! none of them and rejecting or ignoring them varies by server. The one addition is Ollama's own
//! `reasoning_effort: "none"`, sent only to a server that has said it is an Ollama recent enough to
//! take it (see [`Thinking`]), because a model that thinks before it answers otherwise spends most of every call
//! PM makes on thinking nobody reads. The chat Thinking button is the one exception: it leaves the
//! field out so the user can watch the thinking, and only [`stream_chat`] can carry it.
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
use serde::Serialize;
use std::time::{Duration, Instant};

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
pub(crate) fn host_is_loopback_literal(base_url: &str) -> bool {
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
    /// The model thought and never answered. The thinking ran past its time, size or loop bound, or
    /// the stream ended with thinking and no answer. The host answered and was generating, so this is
    /// never a strike.
    UnfinishedThought,
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
    /// A thinking delta. Never a `Token`: it is never part of the answer, and only a stream that
    /// asked to show it forwards it at all.
    Reasoning(String),
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
            // Thinking, before the content in the same frame — the order the model produced them.
            // Ollama (and LM Studio for gpt-oss) stream it as `delta.reasoning`, llama-server (and LM
            // Studio's split setting) as `delta.reasoning_content`, and a buffered server puts either
            // under `message`. The FIRST non-empty one is the thinking, and two are never joined, so
            // a thought sent under both names is never shown twice. An empty one is not thinking,
            // the same rule as the content filter below.
            let choice = &value["choices"][0];
            let reasoning = [
                &choice["delta"]["reasoning"],
                &choice["delta"]["reasoning_content"],
                &choice["message"]["reasoning"],
                &choice["message"]["reasoning_content"],
            ]
            .into_iter()
            .find_map(|v| v.as_str().filter(|r| !r.is_empty()));
            if let Some(r) = reasoning {
                events.push(SseEvent::Reasoning(r.to_string()));
            }
            // Streaming servers put the delta under `delta.content`; a couple of "OpenAI-compatible"
            // servers stream a single buffered message under `message.content` — tolerate both.
            //
            // An EMPTY content is not a token. Ollama sends `"content": ""` on every chunk a
            // thinking model spends thinking (beside `reasoning`, read above as thinking), and on
            // its first chunk. Passed on, fifty of those in a row read as a one-token loop and the
            // guard killed the reply mid-thought; they also counted as the first token, which
            // switched off the fallback to the cloud for a reply that had not started.
            let tok = value["choices"][0]["delta"]["content"]
                .as_str()
                .or_else(|| value["choices"][0]["message"]["content"].as_str());
            if let Some(tok) = tok.filter(|t| !t.is_empty()) {
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
// Thinking — what a stream hands its caller, and the bounds on thinking that is shown. All pure.
// ---------------------------------------------------------------------------------------------

/// What a local chat stream hands its caller: a piece of the answer, or a piece of the thinking that
/// came before it. Thinking arrives only from a stream that asked to show it.
#[derive(Clone, Copy)]
pub enum StreamDelta<'a> {
    Answer(&'a str),
    Thinking(&'a str),
}

/// Untrusted output (rule #6): the most shown thinking PM will forward before stopping the call.
/// About 64k tokens; the measured gemma 4 thought is 1,353-1,580 tokens, so this is a memory and
/// webview bound, and the time limit binds first in practice.
const MAX_THOUGHT_BYTES: usize = 256 * 1024;

/// Why shown thinking was stopped, or why a thinking reply has no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThoughtStop {
    /// Thinking ran past [`tunables::THINKING_TIME_LIMIT`] without the answer starting.
    TooLong,
    /// Thinking ran past [`MAX_THOUGHT_BYTES`].
    TooLarge,
    /// Thinking's own loop guard tripped.
    Looping,
    /// The stream ended cleanly with thinking and no answer.
    NoAnswer,
    /// The stream hit the token ceiling (`finish_reason: "length"`) with thinking and no answer.
    OutOfRoom,
}

impl ThoughtStop {
    /// The failure detail: the reason, then one hint. `button_helps` = the user turned the chat
    /// Thinking button on AND PM can switch thinking off for this model on this server
    /// ([`Thinking::Switchable`]), so the way out is turning the button off again. Anywhere else the
    /// server is thinking on its own — on LM Studio, llama-server, an older Ollama or a model that
    /// refused the switch the button only shows or hides it, and turning it off would only hide the
    /// same thought from every bound PM puts on shown thinking — so the way out is on the server.
    pub fn detail(self, button_helps: bool) -> String {
        let reason = match self {
            ThoughtStop::TooLong => format!(
                "it thought for {} minutes without starting its answer, so PM stopped it",
                tunables::THINKING_TIME_LIMIT.as_secs() / 60
            ),
            ThoughtStop::TooLarge => "its thinking ran past PM's size limit".to_string(),
            ThoughtStop::Looping => "its thinking got stuck repeating itself".to_string(),
            ThoughtStop::NoAnswer => "it finished thinking without writing an answer".to_string(),
            ThoughtStop::OutOfRoom => "it ran out of room while it was still thinking".to_string(),
        };
        let hint = if button_helps {
            "turn off Thinking for a straight answer"
        } else {
            "your model server lets this model think; set it to skip thinking, or give the model a \
             longer context"
        };
        format!("{reason} — {hint}")
    }
}

/// Bounds on SHOWN thinking. Pure: `now` is a parameter, so the time limit tests need no sleep.
///
/// It keeps its own [`LoopGuard`], so the answer's guard still sees exactly the answer and nothing
/// a thought left in its tail. A stream with no total deadline and no Stop button needs all three
/// bounds: without them a model that keeps thinking holds the foreground slot until the server gives
/// up. Hidden thinking never reaches this — it is liveness only.
#[derive(Default)]
pub(crate) struct ThoughtBudget {
    bytes: usize,
    started: Option<Instant>,
    guard: LoopGuard,
}

impl ThoughtBudget {
    /// Count one thinking chunk. `Some(stop)` once a bound is crossed.
    /// The size check runs first, then time (from the first chunk), then the loop guard.
    pub(crate) fn observe(&mut self, chunk: &str, now: Instant) -> Option<ThoughtStop> {
        self.bytes += chunk.len();
        if self.bytes > MAX_THOUGHT_BYTES {
            return Some(ThoughtStop::TooLarge);
        }
        let started = *self.started.get_or_insert(now);
        if now.saturating_duration_since(started) > tunables::THINKING_TIME_LIMIT {
            return Some(ThoughtStop::TooLong);
        }
        self.guard.observe(chunk).then_some(ThoughtStop::Looping)
    }
}

/// The silence allowed before the next chunk: the long first-token wait until the model is
/// producing ANYTHING (thinking or answer), then the inter-token window.
fn chunk_deadline(generating: bool) -> Duration {
    if generating {
        tunables::INTER_TOKEN_TIMEOUT
    } else {
        tunables::TIME_TO_FIRST_TOKEN_TIMEOUT
    }
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
///
/// `thinking_off` adds [`THINKING_OFF`] as `reasoning_effort`, and the caller sets it only for an
/// endpoint [`Thinking::Switchable`] says takes it.
pub fn chat_body(
    model: &str,
    messages: &[ChatMessage],
    stream: bool,
    thinking_off: bool,
) -> serde_json::Value {
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
    if thinking_off {
        body["reasoning_effort"] = serde_json::json!(THINKING_OFF);
    }
    body
}

// --- switching thinking off -------------------------------------------------------------------------

/// The one `reasoning_effort` PM sends, and only to an Ollama of [`THINKING_OFF_SINCE`] or later.
///
/// A model that thinks before it answers does it on every call unless told not to, and PM reads only
/// the answer. Measured on Ollama 0.33.0 with gemma 4 12b Q3_K_M (an RTX 5060 Laptop GPU), filing one
/// invoice title into one of four projects: **1353–1580 completion tokens and 48–61 s** with nothing
/// sent, against **20 tokens and 0.9 s** with `"none"` — the same answer either way. At that cost a
/// long background job runs into `BACKGROUND_TOTAL_TIMEOUT`, and three of those cool the endpoint
/// down for chat too. Ollama thinks for that model even though its `/api/show` lists no "thinking"
/// capability, so PM cannot ask the model first; it asks the server instead.
///
/// `"none"` is the only safe value, measured on the same server: Qwen2.5 7B and gemma 3 4b, which
/// never think, answer normally with it; `"low"` is refused with a 400 "does not support thinking"
/// for Qwen2.5 and for gemma 4 itself. Ollama ignores `chat_template_kwargs` and a top-level
/// `think: false` on `/v1` (gemma 4 still thought for 655-789 tokens), so neither is an alternative.
///
/// Chat leaves it out when the user has turned on the chat Thinking button (`show_thinking`). That
/// is the request PM sent before #852, so no server can refuse it. Background work never can.
pub const THINKING_OFF: &str = "none";

/// The first Ollama that takes [`THINKING_OFF`] from every model. Read from Ollama's source by tag:
/// 0.11.5-0.12.3 pass `"none"` through as a think level and refuse it with a 400 ("invalid think
/// value"), 0.11.4 and earlier have no such field, and 0.12.4 maps it to `think: false` but then
/// demands the thinking capability for it, so a model that never thinks is refused there. 0.12.5 is
/// the first that only asks for the capability when thinking is turned ON.
pub const THINKING_OFF_SINCE: (u64, u64, u64) = (0, 12, 5);

/// Whether an endpoint takes [`THINKING_OFF`]. Learned once per endpoint, from `/api/version`, and
/// remembered in [`THINKING`] until the endpoint or its token is saved again ([`forget_thinking`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    /// An Ollama of [`THINKING_OFF_SINCE`] or later: PM switches thinking off.
    Switchable,
    /// Anything else: the body goes exactly as it did before.
    ///
    /// For llama-server, LM Studio and the rest this is the old behaviour, not a fix. Each has its
    /// own switch (llama-server, for one, takes `chat_template_kwargs` for a template that reads
    /// `enable_thinking`, or `--reasoning-budget 0` at launch), none of them has been measured here,
    /// and a field sent to a server that has not said what it is risks a strict one refusing every
    /// call PM makes. There, the chat Thinking button only shows or hides what the server already
    /// does.
    Leave,
}

/// What PM has learned per endpoint, and which models on it refused the switch anyway. Only a
/// definite answer is recorded (see [`thinking_from_version`]), so an endpoint that could not answer
/// is asked again on the next call.
#[derive(Default)]
struct KnownThinking {
    endpoints: std::collections::HashMap<String, Thinking>,
    refused: std::collections::HashSet<(String, String)>,
}

static THINKING: std::sync::LazyLock<std::sync::Mutex<KnownThinking>> =
    std::sync::LazyLock::new(Default::default);

/// Forget everything learned about switching thinking off. Called whenever the endpoint or its token
/// is saved: a new address may be a different server, and a new token can turn a 401 into an answer.
pub fn forget_thinking() {
    if let Ok(mut known) = THINKING.lock() {
        *known = KnownThinking::default();
    }
}

/// An Ollama version string as `(major, minor, patch)`: `0.33.0`, or `0.12.5-rc1` with its
/// pre-release tag dropped. `None` for anything else, including a development build's `0.0.0`
/// stand-in, which says nothing about what the server takes.
pub fn ollama_version(raw: &str) -> Option<(u64, u64, u64)> {
    let core = raw.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let version = (next()?, next()?, next()?);
    (version != (0, 0, 0)).then_some(version)
}

/// What one `/api/version` answer says, or `None` when it says nothing either way.
///
/// - A 404 or 405 is a server without the route: llama-server, vLLM. Recorded as [`Thinking::Leave`].
/// - A 2xx whose JSON carries a version of [`THINKING_OFF_SINCE`] or later is an Ollama that takes
///   the switch. Any other 2xx JSON is a server that answered and is not one: LM Studio is reported
///   (not measured here) to answer a route it doesn't have with a 200 and an `error` object, and
///   LocalAI, KoboldCpp and llama-swap answer this route with versions or shapes that are not a
///   recent Ollama's. Both recorded as `Leave`.
/// - Anything else (401, 403, 407, 408, 429, a 5xx, or a 2xx whose body is not JSON) is a proxy or a
///   server that could not answer right now, and is not recorded.
pub fn thinking_from_version(status: u16, body: Option<&serde_json::Value>) -> Option<Thinking> {
    match status {
        404 | 405 => Some(Thinking::Leave),
        200..=299 => {
            let body = body?;
            let recent = body
                .get("version")
                .and_then(|v| v.as_str())
                .and_then(ollama_version)
                .is_some_and(|v| v >= THINKING_OFF_SINCE);
            Some(if recent {
                Thinking::Switchable
            } else {
                Thinking::Leave
            })
        }
        _ => None,
    }
}

/// Whether to switch thinking off for this model on this endpoint, asking the endpoint the first
/// time.
async fn thinking_for(base_url: &str, model: &str, token: Option<&str>) -> Thinking {
    let key = (base_url.to_string(), model.to_string());
    if let Some(known) = THINKING.lock().ok().and_then(|k| {
        if k.refused.contains(&key) {
            return Some(Thinking::Leave);
        }
        k.endpoints.get(base_url).copied()
    }) {
        return known;
    }
    let url = format!("{base_url}/api/version");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    // Unreachable or timed out: nothing learned, and the call that follows will most likely fail
    // the same way. Leave, for this call only.
    let Ok(response) = req.send().await else {
        return Thinking::Leave;
    };
    let status = response.status().as_u16();
    let body = if response.status().is_success() {
        response.json::<serde_json::Value>().await.ok()
    } else {
        None
    };
    let Some(thinking) = thinking_from_version(status, body.as_ref()) else {
        return Thinking::Leave;
    };
    if let Ok(mut known) = THINKING.lock() {
        known.endpoints.insert(base_url.to_string(), thinking);
    }
    thinking
}

/// Whether PM can switch thinking off for this model on this endpoint — asked by the gateway when a
/// chat that wants to think has no room to, so it can send the turn with thinking off instead.
pub(crate) async fn takes_thinking_off(base_url: &str, model: &str, token: Option<&str>) -> bool {
    thinking_for(base_url, model, token).await == Thinking::Switchable
}

/// Whether a refusal is one the thinking switch could have caused, so PM resends without it.
fn may_refuse_the_switch(status: u16) -> bool {
    status == 400 || status == 422
}

/// POST one chat request and return the response once the server has accepted it.
///
/// Switches thinking off where [`thinking_for`] says the server takes it. If the server refuses the
/// request with a 400 or 422, PM resends it once without the switch, and only when THAT succeeds —
/// proof it was the switch the server objected to — does it stop sending the switch for this model
/// on this endpoint. A refusal that has nothing to do with the switch fails the same way twice and
/// is reported as it always was, with nothing remembered.
///
/// `want_thinking` is the chat Thinking button, and only [`stream_chat`] can pass `true`.
async fn post_chat(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    messages: &[ChatMessage],
    stream: bool,
    deadline: Option<Duration>,
    want_thinking: bool,
) -> LocalResult<reqwest::Response> {
    // Thinking asked for: send the body PM sent before #852 — no field, no probe. Short-circuited so
    // the on path never touches THINKING (it can't resend, so it can't record a refusal either).
    let mut thinking_off =
        !want_thinking && thinking_for(base_url, model, token).await == Thinking::Switchable;
    let mut resent = false;
    let url = format!("{base_url}/v1/chat/completions");
    loop {
        let body = chat_body(model, messages, stream, thinking_off);
        let mut req = client_for(base_url).post(&url);
        if stream {
            req = req.header(reqwest::header::ACCEPT, "text/event-stream");
        }
        if let Some(d) = deadline {
            req = req.timeout(d);
        }
        let mut req = req.json(&body);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let response = req.send().await.map_err(|e| {
            LocalFailure::new(classify_send_error(&e), redacted_transport_detail(e))
        })?;
        let status = response.status();
        if status.is_success() {
            if resent {
                if let Ok(mut known) = THINKING.lock() {
                    known
                        .refused
                        .insert((base_url.to_string(), model.to_string()));
                }
            }
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        if thinking_off && may_refuse_the_switch(status.as_u16()) {
            thinking_off = false;
            resent = true;
            continue;
        }
        return Err(LocalFailure::new(
            classify_http(status.as_u16(), &body),
            crate::error::truncate_detail(&body),
        ));
    }
}

// --- releasing the GPU (#786 item 8) ------------------------------------------------------------

/// Build the body that asks Ollama to drop a model from memory, and nothing else.
///
/// `keep_alive: 0` on an EMPTY message list is the unload gesture: it takes the unload arm, loads
/// nothing and generates nothing. Measured against 0.33.0 — 200 with `done_reason: "unload"`, in
/// under a millisecond.
///
/// **Zero is the only value PM may ever send here**, and that is not fussiness. Measured on the same
/// server: one request carrying a POSITIVE `keep_alive` reprograms that runner for the rest of its
/// life. A later request that omits the field inherits the expiry and slides it forward — and so does
/// one arriving on `/v1/chat/completions`, which has no such field at all. So a single PM request
/// carrying `keep_alive: "5m"` would silently demote a user's own `OLLAMA_KEEP_ALIVE=-1`, for every
/// client of that server, permanently. `0` leaves nothing behind: the model unloads, and the next
/// load takes the server's own default again.
///
/// This is why PM's "release after a quiet period" policy is its OWN timer plus this call, rather
/// than the obvious implementation of putting a TTL on each request.
pub fn unload_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [],
        "keep_alive": 0,
    })
}

/// How long to wait for the runner to actually go away after a successful unload.
///
/// The 200 is NOT the teardown. Measured: the response came back in 0.82 ms while the runner stayed
/// resident for a further ~850 ms, and a load issued inside that window returned a FALSE
/// `done_reason: "load"` in 0.9 ms by re-attaching to the runner that was still there. So a caller
/// that trusts the 200 will report memory freed that is still held.
const UNLOAD_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const UNLOAD_SETTLE_POLL: Duration = Duration::from_millis(100);

/// What happened when PM asked a server to drop a model.
///
/// Three states rather than a bool, because the two a bool collapses are the ones that matter.
/// "I could not check" is not "it is gone", and "this server has no unload route" is not "there was
/// nothing to free" — the first would have PM tell someone it freed memory it never confirmed, and
/// the second would have PM ask a server that can never answer, every twenty seconds, forever.
///
/// There is deliberately no "was not loaded" arm: the caller establishes residency from `/api/ps`
/// before asking, so a model that is not there is never asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnloadOutcome {
    /// Confirmed gone.
    Freed,
    /// This server has no unload route at all (llama-server, LM Studio, a `/v1`-only proxy). A
    /// permanent property of the endpoint, so a caller should latch it and stop asking.
    NoRoute,
    /// The request was accepted but PM could not confirm the memory came back. Deliberately NOT
    /// counted as success: the caller must not tell anyone it freed something it never saw freed.
    Unconfirmed,
}

/// Ask the server to drop `model` from memory.
///
/// With `confirm`, waits until `/api/ps` stops reporting it — because the 200 is not the teardown.
/// Measured: the response came back in 0.82 ms while the runner stayed resident a further ~850 ms,
/// and a load issued inside that window returned a FALSE `done_reason: "load"` by re-attaching to
/// the runner that was still there.
///
/// Without `confirm`, sends and returns. That is for the exit path only, where the process is about
/// to end and confirmation has no consumer — spending a shutdown budget proving something nobody
/// will read is how the second model never gets its request sent at all.
///
/// Never an error the caller must handle: this is housekeeping, and it must not be evidence about
/// the endpoint's health in either direction.
pub async fn unload_model(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    confirm: bool,
) -> UnloadOutcome {
    let url = format!("{base_url}/api/chat");
    let mut req = client_for(base_url)
        .post(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT)
        .json(&unload_body(model));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let Ok(response) = req.send().await else {
        return UnloadOutcome::Unconfirmed;
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Either the route does not exist or the model id is unknown. Both mean "asking this again
        // will not help", and the caller latches on it.
        return UnloadOutcome::NoRoute;
    }
    if !response.status().is_success() {
        return UnloadOutcome::Unconfirmed;
    }
    if !confirm {
        return UnloadOutcome::Freed;
    }
    let deadline = Instant::now() + UNLOAD_SETTLE_TIMEOUT;
    loop {
        match model_is_resident(base_url, model, token).await {
            // Answered, and it is gone.
            Some(false) => return UnloadOutcome::Freed,
            // Could not ask. NOT "it is gone" — that inversion is the whole reason this returns four
            // states, since a `/v1`-only proxy answers the POST and fails `/api/ps`.
            None => return UnloadOutcome::Unconfirmed,
            Some(true) => {}
        }
        if Instant::now() >= deadline {
            return UnloadOutcome::Unconfirmed;
        }
        tokio::time::sleep(UNLOAD_SETTLE_POLL).await;
    }
}

/// What `/api/ps` answered — three states, because two of them are routinely confused.
///
/// [`Self::Resident`] with an empty list is a real answer: the server is up and holding nothing.
/// [`Self::Unknown`] is the absence of an answer. A caller that flattens the two will tell someone
/// their card is free when PM simply could not ask.
#[derive(Debug, Clone, PartialEq)]
pub enum PsAnswer {
    /// The server answered with what it is holding. An empty list means "nothing", not "no idea".
    Resident(Vec<ResidentModel>),
    /// The server answered, and it has no such route.
    ///
    /// `/api/ps` is Ollama's own API, and so is the `/api/chat` unload. A server that 404s the first
    /// does not have the second either, which is the fact the release path needs: llama-server and
    /// LM Studio hold a model for their whole process life, and asking them to let go is a request
    /// that can only ever fail. Kept distinct from [`Self::Unknown`] so it can be LATCHED — a
    /// missing route is permanent for this endpoint, an unreachable host is not.
    NoRoute,
    /// PM could not ask: unreachable, a timeout, or a body it could not read. It knows nothing.
    Unknown,
}

impl PsAnswer {
    /// The resident list, or `None` for both of the not-an-answer states.
    pub fn models(self) -> Option<Vec<ResidentModel>> {
        match self {
            Self::Resident(models) => Some(models),
            Self::NoRoute | Self::Unknown => None,
        }
    }
}

/// What the server currently has loaded, per its own `/api/ps`.
pub async fn ollama_ps(base_url: &str, token: Option<&str>) -> PsAnswer {
    let url = format!("{base_url}/api/ps");
    let mut req = client_for(base_url)
        .get(&url)
        .timeout(tunables::WINDOW_PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let Ok(response) = req.send().await else {
        return PsAnswer::Unknown;
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return PsAnswer::NoRoute;
    }
    if !response.status().is_success() {
        return PsAnswer::Unknown;
    }
    let Ok(value) = response.json::<serde_json::Value>().await else {
        return PsAnswer::Unknown;
    };
    // A 200 whose body is not the shape `/api/ps` returns is not evidence of anything — including
    // that the route is missing, since something answered. Unknown, not NoRoute.
    if value.get("models").and_then(|m| m.as_array()).is_none() {
        return PsAnswer::Unknown;
    }
    PsAnswer::Resident(resident_from_ps(&value))
}

/// Whether `model` is currently resident, per the server's own `/api/ps`.
///
/// `None` means PM could not ask — unreachable, no such route, or a body it could not parse. That is
/// a third state and it must stay one: reading "cannot check" as "not resident" is how a confirm
/// loop turns into a rubber stamp.
pub async fn model_is_resident(base_url: &str, model: &str, token: Option<&str>) -> Option<bool> {
    Some(model_in(&ollama_ps(base_url, token).await.models()?, model))
}

/// Whether a resident list names `model`. Ollama echoes the id as it was pulled, so the comparison
/// is case-insensitive; it is otherwise exact, because a looser match would answer for a different
/// load on a machine serving several.
pub fn model_in(resident: &[ResidentModel], model: &str) -> bool {
    resident.iter().any(|m| m.model.eq_ignore_ascii_case(model))
}

/// One resident model, as `/api/ps` reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResidentModel {
    pub model: String,
    /// Total bytes the server placed for it, in GiB.
    pub size_gb: f64,
    /// The share of that it reports as being on the GPU, in GiB.
    ///
    /// A FLOOR, not a measurement. It counts the weights the server placed and excludes the CUDA
    /// context and compute buffers: measured on a laptop card, a model reporting 2.70 GiB here was
    /// actually using 3.95 GiB of the card — a 1.25 GiB gap. Never present it as the GPU footprint,
    /// and never let it prove that something fits.
    pub size_vram_gb: f64,
    /// The context window this load was actually given, when the server reports one. A zero is a
    /// missing value rather than a window of nothing.
    pub context_length: Option<u32>,
}

/// The resident models in an `/api/ps` body. Pure, so the shapes are testable without a server.
pub fn resident_from_ps(value: &serde_json::Value) -> Vec<ResidentModel> {
    value
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let name = m
                        .get("model")
                        .or_else(|| m.get("name"))
                        .and_then(|n| n.as_str())?;
                    // GiB, like every other `*_gb` the Local AI tab shows and compares: these sit
                    // beside the card's `vram_gb` and are rendered with `formatGib`. They were 1e9
                    // bytes, which read a load 7.4% larger than the same load anywhere else.
                    let gib = |key: &str| {
                        m.get(key).and_then(|s| s.as_u64()).unwrap_or(0) as f64 / 1_073_741_824.0
                    };
                    Some(ResidentModel {
                        model: name.to_string(),
                        size_gb: gib("size"),
                        size_vram_gb: gib("size_vram"),
                        context_length: m
                            .get("context_length")
                            .and_then(|c| c.as_u64())
                            .filter(|c| *c > 0)
                            .map(|c| c as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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

/// Stream a chat completion from a local endpoint. `on_delta` gets each answer delta, plus each
/// thinking delta only when `show_thinking`; thinking is never part of `Completion.text`. Classifies
/// every failure structurally (`LocalFailKind`), aborts an obvious token loop, and bounds thinking
/// that is shown ([`ThoughtBudget`]).
///
/// `show_thinking` is the chat Thinking button, and this is the one path that can carry it: on, the
/// request goes without `reasoning_effort` (the body PM sent before #852); off, it is the #852
/// request. In both modes a reply that thinks and never answers is
/// [`LocalFailKind::UnfinishedThought`], never a blank turn.
pub async fn stream_chat<F>(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    messages: &[ChatMessage],
    show_thinking: bool,
    mut on_delta: F,
) -> LocalResult<Completion>
where
    F: FnMut(StreamDelta<'_>),
{
    let response = post_chat(base_url, model, token, messages, true, None, show_thinking).await?;

    let mut full = String::new();
    let mut served: Option<String> = None;
    let mut usage = Usage::default();
    let mut truncated = false;
    let mut saw_finish = false;
    let mut saw_usage = false;
    let mut assembler = SseAssembler::default();
    let mut guard = LoopGuard::default();
    let mut stream = response.bytes_stream();
    // The model is producing something — an answer token OR thinking. A thinking model is generating
    // from its first thought, so a stall after it is a stall between tokens, not a cold load.
    let mut generating = false;
    let mut saw_reasoning = false;
    let mut budget = ThoughtBudget::default();

    'read: loop {
        // Two-phase timeout: a generous deadline until the model produces ANYTHING — absorbing a
        // silent cold model load (Ollama / LM Studio JIT-load and stream nothing until the first
        // token) — then a short inter-token deadline once it is generating. Any received bytes,
        // including an SSE keepalive ping, arrive as a chunk and reset the timer; the inter-token
        // window is set above llama-server's 30 s ping cadence so a ping always resets it before it
        // can fire.
        let deadline = chunk_deadline(generating);
        let next = match tokio::time::timeout(deadline, stream.next()).await {
            Ok(next) => next,
            Err(_elapsed) => {
                let detail = if generating {
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
                SseEvent::Reasoning(r) => {
                    generating = true;
                    saw_reasoning = true;
                    // Hidden thinking is liveness only: no guard, no cap, nothing forwarded,
                    // nothing kept.
                    if !show_thinking {
                        continue;
                    }
                    if let Some(stop) = budget.observe(&r, Instant::now()) {
                        // Hang up first, so the server stops thinking while PM words the way out.
                        drop(stream);
                        return Err(unfinished_thought(stop, true, base_url, model, token).await);
                    }
                    // Never pushed to `full`: the thinking is not the answer, so it can never be
                    // stored, indexed or replayed as one.
                    on_delta(StreamDelta::Thinking(&r));
                }
                SseEvent::Token(tok) => {
                    generating = true;
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
                    on_delta(StreamDelta::Answer(&tok));
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
                SseEvent::Done => break 'read,
            }
        }
        if assembler.buffered_len() > MAX_SSE_LINE_BYTES {
            return Err(LocalFailure::new(
                LocalFailKind::MalformedStream,
                "the model stream sent an oversized line",
            ));
        }
    }

    // The stream is over: by its `[DONE]`, or because the byte stream ended. Without a `[DONE]`,
    // accept it only if the model gave some other clean end signal (a finish_reason or a usage
    // chunk); otherwise the connection was cut mid-flight.
    if !stream_ended_cleanly(assembler.saw_done(), saw_finish, saw_usage) {
        return Err(LocalFailure::new(
            LocalFailKind::MalformedStream,
            "the model stream ended without a completion marker",
        ));
    }
    if let Some(stop) = unanswered(&full, truncated, saw_reasoning) {
        return Err(unfinished_thought(stop, show_thinking, base_url, model, token).await);
    }
    Ok(Completion {
        text: full,
        model: served,
        usage,
        truncated,
    })
}

/// A thinking reply with no answer is a failure, not a turn — in both modes, because on a server
/// that thinks regardless it would otherwise be stored as a blank (or cut-off-marker-only) turn.
/// `None` when the reply has an answer, or never thought.
fn unanswered(full: &str, truncated: bool, saw_reasoning: bool) -> Option<ThoughtStop> {
    (saw_reasoning && full.trim().is_empty()).then_some(if truncated {
        ThoughtStop::OutOfRoom
    } else {
        ThoughtStop::NoAnswer
    })
}

/// The [`LocalFailKind::UnfinishedThought`] for a stopped or unanswered thought, with the way out
/// that really is one ([`ThoughtStop::detail`]): the Thinking button only where it switches
/// thinking off. Asked only once the stream has failed, so a shown thought that ends in an answer
/// never asks `/api/version` (the on path in [`post_chat`] doesn't either), and the answer is
/// usually already known from the background work that switches thinking off on this endpoint.
async fn unfinished_thought(
    stop: ThoughtStop,
    shown: bool,
    base_url: &str,
    model: &str,
    token: Option<&str>,
) -> LocalFailure {
    let button_helps = shown && takes_thinking_off(base_url, model, token).await;
    LocalFailure::new(LocalFailKind::UnfinishedThought, stop.detail(button_helps))
}

/// A single non-streaming chat completion — background work wants the whole answer at once.
pub async fn complete(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    messages: &[ChatMessage],
) -> LocalResult<Completion> {
    complete_within(
        base_url,
        model,
        token,
        messages,
        tunables::BACKGROUND_TOTAL_TIMEOUT,
    )
    .await
}

/// The same call with the deadline named by the caller.
///
/// Background work gets [`tunables::BACKGROUND_TOTAL_TIMEOUT`], which is a budget: three of them in
/// a row are three strikes and a cooled-down endpoint, so it has to be short enough to fail fast. A
/// test the user is watching is the opposite — it exists to find out whether the thing works at all,
/// a cold load is the slow case it most needs to survive, and it records no health outcome either
/// way. Two different questions, so two different deadlines, both named in `tunables`.
///
/// Never thinks: there is deliberately no parameter for it.
pub async fn complete_within(
    base_url: &str,
    model: &str,
    token: Option<&str>,
    messages: &[ChatMessage],
    deadline: std::time::Duration,
) -> LocalResult<Completion> {
    let response = post_chat(
        base_url,
        model,
        token,
        messages,
        false,
        Some(deadline),
        false,
    )
    .await?;
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

/// One pass of the two PROVEN rungs — `/slots`, then Ollama's `/api/ps` — kept whole instead of
/// reduced to a number.
///
/// [`probe_window`] can never return nothing: it falls through to [`DEFAULT_CONTEXT`] by design,
/// because a caller sizing a prompt always needs a number. A caller that is merely LOOKING needs the
/// opposite guarantee — it must be able to learn nothing and write nothing, rather than record PM's
/// own floor into a cache that other paths read as evidence about the user's server. Neither rung
/// cares who loaded the model, which is what makes a passive caller worth having: the answer is
/// there whenever something is resident, whoever put it there.
///
/// It is kept whole because the ladder asks the ENDPOINT, not the model — `/slots` describes the one
/// thing llama-server is running, and a single `/api/ps` lists everything Ollama holds. Asking once
/// and questioning the ANSWER per model is what lets a two-role setup learn both windows and both
/// residencies in the two requests a one-role setup already costs.
pub struct LiveProbe {
    /// llama-server's `/slots`, which answers about its single load and names no model.
    slots_ctx: Option<u32>,
    /// Ollama's `/api/ps`, which answers per model. [`PsAnswer::Unknown`] when `/slots` answered
    /// first and this was never asked.
    ps: PsAnswer,
}

/// Ask both proven rungs, short-circuiting exactly as the ladder always has: `/slots` first, and
/// `/api/ps` only if it was silent.
pub async fn probe_live(base_url: &str, token: Option<&str>) -> LiveProbe {
    if let Some(tokens) = probe_slots_ctx(base_url, token).await {
        return LiveProbe {
            slots_ctx: Some(tokens),
            ps: PsAnswer::Unknown,
        };
    }
    LiveProbe {
        slots_ctx: None,
        ps: ollama_ps(base_url, token).await,
    }
}

impl LiveProbe {
    /// The served window for one model, from whichever rung answered.
    pub fn window_for(&self, model: &str) -> Option<WindowInfo> {
        if let Some(tokens) = self.slots_ctx {
            return Some(WindowInfo {
                tokens,
                source: WindowSource::Slots,
            });
        }
        match &self.ps {
            PsAnswer::Resident(models) => models
                .iter()
                .find(|m| m.model.eq_ignore_ascii_case(model))
                .and_then(|m| m.context_length)
                .map(|tokens| WindowInfo {
                    tokens,
                    source: WindowSource::LoadedModel,
                }),
            PsAnswer::NoRoute | PsAnswer::Unknown => None,
        }
    }

    /// What the endpoint says it is holding, or `None` when it cannot answer that at all.
    ///
    /// Only `/api/ps` answers it, and it answers for the whole ENDPOINT — so a caller with two role
    /// models asks this once and then questions the list, rather than getting an `Option` per model
    /// that is always the same variant. `/slots` proves llama-server is holding SOMETHING, for the
    /// whole life of its process, but never says what: a user who typed a model id that server never
    /// heard of would be told their model was resident. So the runners without an `/api/ps` report
    /// "PM cannot tell", which is true, and which the surfaces already render for an unreachable
    /// endpoint anyway.
    pub fn residency(&self) -> Option<&[ResidentModel]> {
        match &self.ps {
            PsAnswer::Resident(models) => Some(models),
            PsAnswer::NoRoute | PsAnswer::Unknown => None,
        }
    }

    /// Whether the endpoint answered that it has no `/api/ps` — and so no unload gesture either.
    pub fn no_ollama_api(&self) -> bool {
        self.ps == PsAnswer::NoRoute
    }
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
///
/// Built on the same parse and the same matcher as the residency question, so the two cannot
/// disagree about which row is `model`. They used to: this asked case-SENSITIVELY while the release
/// path asked case-insensitively, which is a footer able to say "loaded" and "no window" about one
/// load. Case-insensitive is not the looser test its predecessor's comment feared — Ollama
/// lower-cases a tag when it pulls it, so two rows differing only in case cannot both exist.
pub fn loaded_ctx_from_ps(value: &serde_json::Value, model: &str) -> Option<u32> {
    value.get("models")?.as_array()?;
    resident_from_ps(value)
        .into_iter()
        .find(|m| m.model.eq_ignore_ascii_case(model))?
        .context_length
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
    /// `details.parameter_size` in billions (`"7.62B"` → 7.62, `"494.03M"` → 0.494), or `None` when
    /// absent or not a size. What lets a bare library tag like `qwen2.5:latest`, whose name carries
    /// no size at all, be matched back to a catalogue entry (`local_catalog::match_served`).
    pub parameter_size_b: Option<f64>,
}

/// Parse Ollama's `details.parameter_size` (`"7.62B"`, `"494.03M"`) into billions. A trailing `B`/`b`
/// is billions and `M`/`m` millions; anything else — `"unknown"`, a bare number, a negative or
/// non-finite one — is `None`, because a guessed size would match the wrong catalogue entry.
pub fn parse_parameter_size(raw: &str) -> Option<f64> {
    let s = raw.trim();
    let (number, scale) = match s.strip_suffix(['B', 'b']) {
        Some(n) => (n, 1.0),
        None => (s.strip_suffix(['M', 'm'])?, 1e-3),
    };
    let value: f64 = number.trim().parse().ok()?;
    (value.is_finite() && value > 0.0).then_some(value * scale)
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
                    parameter_size_b: m
                        .get("details")
                        .and_then(|d| d.get("parameter_size"))
                        .and_then(|p| p.as_str())
                        .and_then(parse_parameter_size),
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
                SseEvent::Error(_) | SseEvent::Reasoning(_) => {}
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
    fn the_unload_body_carries_a_zero_keep_alive_and_absolutely_nothing_else() {
        // Shape-pinned like `chat_body`, and for a sharper reason. Measured on a live server: ONE
        // request carrying a POSITIVE `keep_alive` reprograms that runner for the rest of its life —
        // a later request omitting the field inherits the expiry and slides it forward, and so does
        // one arriving on `/v1/chat/completions`, which has no such field at all. So a single stray
        // positive value here would silently demote a user's own `OLLAMA_KEEP_ALIVE=-1` for every
        // client of their server, permanently. Zero leaves nothing behind.
        let body = unload_body("gemma3:4b");
        assert_eq!(body["model"], "gemma3:4b");
        assert_eq!(body["keep_alive"], 0);
        // Empty messages is what makes this the UNLOAD arm rather than a generation.
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(0));
        assert_eq!(
            body.as_object().map(|o| o.len()),
            Some(3),
            "nothing else may ride along in this body"
        );
        // And the value must be the integer zero, not a duration string that happens to mean zero.
        assert!(body["keep_alive"].is_number());
    }

    #[test]
    fn residency_is_read_off_api_ps_without_trusting_its_vram_figure() {
        // Byte-exact from a live server with one model loaded (30-08-2026).
        let body: serde_json::Value = serde_json::from_str(
            r#"{"models":[{"name":"hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M",
                "model":"hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M",
                "size":2896083024,"size_vram":2896083024,"context_length":32768}]}"#,
        )
        .unwrap();
        let resident = resident_from_ps(&body);
        assert_eq!(resident.len(), 1);
        assert_eq!(
            resident[0].model,
            "hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M"
        );
        // 2_896_083_024 bytes in GiB — the 2.70 the comment below quotes, not 2.90 in decimal GB.
        assert!((resident[0].size_gb - 2.697).abs() < 0.001);
        // `size_vram` is carried, but it is a FLOOR: on this measurement the card was actually
        // holding 3.95 GiB against the 2.70 GiB reported, because the CUDA context and compute
        // buffers sit outside it. Nothing may use this to prove something fits.
        assert!((resident[0].size_vram_gb - 2.697).abs() < 0.001);

        // Nothing loaded, and a body that is not a `/api/ps` answer at all, both read as "nothing".
        let empty: serde_json::Value = serde_json::from_str(r#"{"models":[]}"#).unwrap();
        assert!(resident_from_ps(&empty).is_empty());
        let alien: serde_json::Value = serde_json::from_str(r#"{"object":"list"}"#).unwrap();
        assert!(resident_from_ps(&alien).is_empty());
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
        // The parameter count rides along — the one fact a bare library tag's name does not carry.
        assert_eq!(tags[0].parameter_size_b, Some(3.88));
        assert_eq!(tags[1].parameter_size_b, Some(7.62));
    }

    #[test]
    fn a_parameter_size_is_read_in_billions_or_not_at_all() {
        assert_eq!(parse_parameter_size("7.62B"), Some(7.62));
        assert_eq!(parse_parameter_size(" 8.0b "), Some(8.0));
        let small = parse_parameter_size("494.03M").unwrap();
        assert!((small - 0.49403).abs() < 1e-9, "{small}");
        // Ollama's own "I don't know", and the shapes a guess would hide behind.
        for raw in ["unknown", "", "7.62", "B", "-7B", "NaNB", "infB", "7.6 GB"] {
            assert_eq!(parse_parameter_size(raw), None, "{raw:?}");
        }

        // Missing from the listing entirely: no size, never a zero.
        let body: serde_json::Value = serde_json::from_str(
            r#"{"models":[{"name":"llama3.2:latest","size":2019393189,"details":{}},
                          {"name":"qwen2.5:latest","size":4683087332}]}"#,
        )
        .unwrap();
        let tags = tags_from_json(&body).expect("a tags listing");
        assert!(tags.iter().all(|t| t.parameter_size_b.is_none()));
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

    /// The three answers `/api/ps` can give, and the two that are routinely confused.
    #[test]
    fn ps_answer_keeps_could_not_ask_apart_from_holding_nothing() {
        assert_eq!(PsAnswer::Resident(vec![]).models(), Some(vec![]));
        assert_eq!(PsAnswer::NoRoute.models(), None);
        assert_eq!(PsAnswer::Unknown.models(), None);

        // A server answering that it holds nothing is a REAL answer about residency.
        let empty = LiveProbe {
            slots_ctx: None,
            ps: PsAnswer::Resident(vec![]),
        };
        assert_eq!(
            empty.residency().map(|m| model_in(m, "gemma3:4b")),
            Some(false)
        );
        // Neither of the other two is. Reading either as "not loaded" is the inversion that would
        // tell an LM Studio user their model is unloaded for as long as they own it.
        for ps in [PsAnswer::NoRoute, PsAnswer::Unknown] {
            let probe = LiveProbe {
                slots_ctx: None,
                ps,
            };
            assert!(probe.residency().is_none());
        }
    }

    /// The latch fix: only a 404 says "this endpoint has no Ollama API", and only that may be
    /// remembered forever. An unreachable host must never be latched — it comes back.
    #[test]
    fn only_a_missing_route_is_a_permanent_fact_about_the_endpoint() {
        let no_route = LiveProbe {
            slots_ctx: None,
            ps: PsAnswer::NoRoute,
        };
        assert!(no_route.no_ollama_api());
        for ps in [PsAnswer::Unknown, PsAnswer::Resident(vec![])] {
            let probe = LiveProbe {
                slots_ctx: None,
                ps,
            };
            assert!(!probe.no_ollama_api());
        }
    }

    /// `/slots` proves llama-server is holding SOMETHING and names nothing, so it answers the window
    /// question and must decline the residency one.
    #[test]
    fn slots_answers_the_window_and_says_nothing_about_which_model() {
        let probe = LiveProbe {
            slots_ctx: Some(8192),
            ps: PsAnswer::Unknown,
        };
        let window = probe.window_for("anything-at-all").expect("a window");
        assert_eq!(window.tokens, 8192);
        assert_eq!(window.source, WindowSource::Slots);
        // A user who typed a model id this server never heard of would otherwise be told it is
        // resident, on the strength of a route that never looked at the id.
        assert!(probe.residency().is_none());
    }

    /// One `/api/ps` body answers both questions, and the two answers agree about which row is the
    /// model — including on case, which they used to disagree about.
    #[test]
    fn one_ps_read_answers_residency_and_window_about_the_same_row() {
        let body = serde_json::json!({"models":[
            {"model":"Qwen2.5:7B-Instruct-Q4_K_M","size":5000000000u64,
             "size_vram":4748056984u64,"context_length":4096},
            {"model":"gemma3:4b","size":3000000000u64,"size_vram":0,"context_length":0}
        ]});
        let probe = LiveProbe {
            slots_ctx: None,
            ps: PsAnswer::Resident(resident_from_ps(&body)),
        };
        // Ollama lower-cases a tag when it pulls it, so two rows differing only in case cannot both
        // exist — and the two questions must not answer about different rows.
        let here = |m: &str| probe.residency().map(|list| model_in(list, m));
        assert_eq!(here("qwen2.5:7b-instruct-q4_K_M"), Some(true));
        assert_eq!(
            probe
                .window_for("qwen2.5:7b-instruct-q4_K_M")
                .map(|w| (w.tokens, w.source)),
            Some((4096, WindowSource::LoadedModel))
        );
        // A zero context_length is a missing value, not a window of nothing — and it must not make
        // the model look absent.
        assert_eq!(here("gemma3:4b"), Some(true));
        assert_eq!(probe.window_for("gemma3:4b"), None);
        assert_eq!(here("mistral:7b"), Some(false));
    }

    #[test]
    fn request_body_carries_no_cloud_only_fields() {
        let body = chat_body("llama3.2", &[msg("user", "hi")], true, false);
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
        let buffered = chat_body("m", &[msg("user", "x")], false, false);
        assert_eq!(buffered["stream"], false);
        assert!(buffered.get("stream_options").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert!(buffered.get("reasoning_effort").is_none());
    }

    #[test]
    fn the_thinking_switch_is_one_field_and_only_ever_none() {
        for stream in [true, false] {
            let off = chat_body("gemma4", &[msg("user", "hi")], stream, true);
            let plain = chat_body("gemma4", &[msg("user", "hi")], stream, false);
            assert_eq!(off["reasoning_effort"], "none");
            // Exactly one field more than the body PM always sent, and nothing else changed.
            let mut off = off.as_object().unwrap().clone();
            off.remove("reasoning_effort");
            assert_eq!(serde_json::Value::Object(off), plain);
        }
        // "low" is refused for a model that never thinks, so "none" is the only value PM may send.
        assert_eq!(THINKING_OFF, "none");
    }

    #[test]
    fn only_a_recent_ollamas_version_answer_takes_the_switch() {
        let answer = |v: serde_json::Value| thinking_from_version(200, Some(&v));
        assert_eq!(
            answer(serde_json::json!({"version": "0.33.0"})),
            Some(Thinking::Switchable)
        );
        assert_eq!(
            answer(serde_json::json!({"version": "0.12.5"})),
            Some(Thinking::Switchable)
        );
        assert_eq!(
            answer(serde_json::json!({"version": "0.12.5-rc1"})),
            Some(Thinking::Switchable)
        );
        // 0.12.4 refuses "none" for a model that never thinks, and 0.11.5-0.12.3 refuse it outright.
        for old in ["0.12.4", "0.12.3", "0.12.0", "0.11.5", "0.11.4", "0.1.32"] {
            assert_eq!(
                answer(serde_json::json!({ "version": old })),
                Some(Thinking::Leave),
                "{old}"
            );
        }
        // A development build, LM Studio's 200 for a route it doesn't have, and servers that answer
        // the route with something that is not a recent Ollama's version: all answered, none takes it.
        for other in [
            serde_json::json!({"version": "0.0.0"}),
            serde_json::json!({"error": "Unexpected endpoint or method. (GET /api/version)"}),
            serde_json::json!({"version": "0.9.0"}),
            serde_json::json!({"version": "v2.26.0"}),
            serde_json::json!({"version": "v150"}),
            serde_json::json!({"version": ""}),
            serde_json::json!({"version": 3}),
            serde_json::json!([]),
        ] {
            assert_eq!(answer(other.clone()), Some(Thinking::Leave), "{other}");
        }
        // No such route is an answer too.
        assert_eq!(thinking_from_version(404, None), Some(Thinking::Leave));
        assert_eq!(thinking_from_version(405, None), Some(Thinking::Leave));
        // A proxy that wants a token, a server that is restarting or rate-limiting, or a 2xx whose
        // body never arrived say nothing about what the server is, so nothing is recorded.
        for status in [401, 403, 407, 408, 429, 500, 502, 503] {
            assert_eq!(thinking_from_version(status, None), None, "{status}");
        }
        assert_eq!(thinking_from_version(200, None), None);
    }

    #[test]
    fn only_a_plain_version_number_is_read_as_one() {
        assert_eq!(ollama_version("0.33.0"), Some((0, 33, 0)));
        assert_eq!(ollama_version(" 0.12.5-rc1 "), Some((0, 12, 5)));
        assert_eq!(ollama_version("0.12.5+dirty"), Some((0, 12, 5)));
        assert_eq!(ollama_version("0.0.0"), None);
        assert_eq!(ollama_version("v0.33.0"), None);
        assert_eq!(ollama_version("0.33"), None);
        assert_eq!(ollama_version("0.33.x"), None);
        assert_eq!(ollama_version(""), None);
    }

    #[test]
    fn only_a_refusal_the_switch_could_cause_is_resent_without_it() {
        assert!(may_refuse_the_switch(400));
        assert!(may_refuse_the_switch(422));
        for status in [401, 403, 404, 408, 429, 500, 503] {
            assert!(!may_refuse_the_switch(status), "{status}");
        }
    }

    /// Ollama 0.33.0's shape for a model that is thinking: content "" beside the reasoning, eighty
    /// identical `"hm"` thoughts, then a one-word answer.
    fn thinking_loop_wire() -> String {
        let mut wire = String::from(
            "data: {\"model\":\"gemma4\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        );
        for _ in 0..80 {
            wire.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\"hm\"}}]}\n\n");
        }
        wire.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Taxes\"}}]}\n\n");
        wire.push_str(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        );
        wire.push_str("data: [DONE]\n\n");
        wire
    }

    #[test]
    fn a_thinking_models_empty_chunks_are_not_tokens() {
        let events = SseAssembler::default().feed(thinking_loop_wire().as_bytes());
        let tokens: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                SseEvent::Token(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(tokens, ["Taxes"]);
        // And so the loop guard never sees a run of them.
        let mut guard = LoopGuard::default();
        assert!(!tokens.iter().any(|t| guard.observe(t)));
        // The thinking is read — as thinking, every one of the eighty.
        let thoughts = events
            .iter()
            .filter(|e| matches!(e, SseEvent::Reasoning(r) if r == "hm"))
            .count();
        assert_eq!(thoughts, 80);
    }

    /// A one-thread HTTP server for the thinking switch: it answers `/api/version` with `version`
    /// (or `version_status`), and a chat request with a 400 when it carries the switch and
    /// `refuses_switch` says so, else with a one-word completion. Every request is logged as
    /// `"<path> <model> switch|plain"`. Blocking std I/O on purpose: the crate's tokio has no `net`.
    struct MockServer {
        base_url: String,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    fn mock_server(
        version_status: u16,
        version: &'static str,
        refuses_switch: fn(&str) -> bool,
        refuses_everything: bool,
    ) -> MockServer {
        serve(
            version_status,
            version,
            refuses_switch,
            refuses_everything,
            None,
        )
    }

    /// The same server, and a `"stream": true` chat request gets `sse` back as `text/event-stream`
    /// — a scripted transcript, sent whatever the request carried, exactly as a server that thinks
    /// regardless would. A non-streaming request still gets the one-word completion.
    fn mock_stream_server(
        version_status: u16,
        version: &'static str,
        refuses_everything: bool,
        sse: &'static str,
    ) -> MockServer {
        serve(
            version_status,
            version,
            |_| false,
            refuses_everything,
            Some(sse),
        )
    }

    /// The accept loop both servers share.
    fn serve(
        version_status: u16,
        version: &'static str,
        refuses_switch: fn(&str) -> bool,
        refuses_everything: bool,
        sse: Option<&'static str>,
    ) -> MockServer {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                let json_type = "application/json";
                let (status, content_type, reply) = if path == "/api/version" {
                    (
                        version_status,
                        json_type,
                        format!(r#"{{"version":"{version}"}}"#),
                    )
                } else {
                    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                    let model = json["model"].as_str().unwrap_or("").to_string();
                    let switch = json.get("reasoning_effort").is_some();
                    let streaming = json["stream"].as_bool() == Some(true);
                    seen.lock().unwrap().push(format!(
                        "{path} {model} {}",
                        if switch { "switch" } else { "plain" }
                    ));
                    if refuses_everything || (switch && refuses_switch(&model)) {
                        (400, json_type, r#"{"error":{"message":"no"}}"#.to_string())
                    } else if let (Some(sse), true) = (sse, streaming) {
                        (200, "text/event-stream", sse.to_string())
                    } else {
                        (
                            200,
                            json_type,
                            r#"{"model":"m","choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#
                                .to_string(),
                        )
                    }
                };
                if path == "/api/version" {
                    seen.lock().unwrap().push(path);
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        MockServer { base_url, log }
    }

    impl MockServer {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    async fn ask(server: &MockServer, model: &str) -> LocalResult<Completion> {
        complete_within(
            &server.base_url,
            model,
            None,
            &[msg("user", "hi")],
            Duration::from_secs(5),
        )
        .await
    }

    #[tokio::test]
    async fn a_recent_ollama_is_asked_once_and_gets_the_switch_on_every_call() {
        let server = mock_server(200, "0.33.0", |_| false, false);
        ask(&server, "gemma4").await.unwrap();
        ask(&server, "gemma4").await.unwrap();
        assert_eq!(
            server.take(),
            [
                "/api/version",
                "/v1/chat/completions gemma4 switch",
                "/v1/chat/completions gemma4 switch"
            ]
        );
    }

    #[tokio::test]
    async fn a_model_that_refuses_the_switch_loses_it_and_no_other_model_does() {
        let server = mock_server(200, "0.12.5", |m| m == "picky", false);
        ask(&server, "picky").await.unwrap();
        ask(&server, "picky").await.unwrap();
        ask(&server, "other").await.unwrap();
        assert_eq!(
            server.take(),
            [
                "/api/version",
                "/v1/chat/completions picky switch",
                "/v1/chat/completions picky plain",
                "/v1/chat/completions picky plain",
                "/v1/chat/completions other switch"
            ]
        );
    }

    #[tokio::test]
    async fn a_refusal_the_switch_did_not_cause_is_reported_and_forgets_nothing() {
        let server = mock_server(200, "0.33.0", |_| false, true);
        let Err(err) = ask(&server, "m").await else {
            panic!("a refused request must fail");
        };
        assert_eq!(
            err.kind,
            classify_http(400, r#"{"error":{"message":"no"}}"#)
        );
        assert!(ask(&server, "m").await.is_err());
        // Resent once without the switch each time, and the switch still goes first.
        assert_eq!(
            server.take(),
            [
                "/api/version",
                "/v1/chat/completions m switch",
                "/v1/chat/completions m plain",
                "/v1/chat/completions m switch",
                "/v1/chat/completions m plain"
            ]
        );
    }

    #[tokio::test]
    async fn an_old_ollama_or_another_server_never_sees_the_switch() {
        for (status, version) in [(200, "0.12.3"), (200, "0.12.4"), (404, "")] {
            let server = mock_server(status, version, |_| true, false);
            ask(&server, "m").await.unwrap();
            ask(&server, "m").await.unwrap();
            assert_eq!(
                server.take(),
                [
                    "/api/version",
                    "/v1/chat/completions m plain",
                    "/v1/chat/completions m plain"
                ],
                "{status} {version}"
            );
        }
    }

    #[tokio::test]
    async fn a_version_probe_that_could_not_answer_is_asked_again() {
        let server = mock_server(401, "0.33.0", |_| false, false);
        ask(&server, "m").await.unwrap();
        ask(&server, "m").await.unwrap();
        assert_eq!(
            server.take(),
            [
                "/api/version",
                "/v1/chat/completions m plain",
                "/api/version",
                "/v1/chat/completions m plain"
            ]
        );
    }

    // --- showing thinking ---------------------------------------------------------------------

    /// Every event one frame decodes to, as `kind:text`, for the thinking and answer kinds only.
    fn thinking_and_tokens(frame: &str) -> Vec<String> {
        SseAssembler::default()
            .feed(format!("data: {frame}\n\n").as_bytes())
            .into_iter()
            .filter_map(|e| match e {
                SseEvent::Reasoning(r) => Some(format!("reasoning:{r}")),
                SseEvent::Token(t) => Some(format!("token:{t}")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sse_reads_thinking_from_every_servers_field_and_never_as_a_token() {
        // Ollama: `delta.reasoning` beside an empty content.
        assert_eq!(
            thinking_and_tokens(
                r#"{"choices":[{"index":0,"delta":{"content":"","reasoning":"Let me check"}}]}"#
            ),
            ["reasoning:Let me check"]
        );
        // llama-server (and LM Studio's split setting): `delta.reasoning_content`.
        assert_eq!(
            thinking_and_tokens(r#"{"choices":[{"delta":{"reasoning_content":"the invoice"}}]}"#),
            ["reasoning:the invoice"]
        );
        // A buffered server puts it under `message`, under either name.
        assert_eq!(
            thinking_and_tokens(r#"{"choices":[{"message":{"reasoning":"all at once"}}]}"#),
            ["reasoning:all at once"]
        );
        assert_eq!(
            thinking_and_tokens(r#"{"choices":[{"message":{"reasoning_content":"buffered"}}]}"#),
            ["reasoning:buffered"]
        );
        // An empty thought is not thinking, any more than an empty content is a token.
        assert!(
            thinking_and_tokens(r#"{"choices":[{"delta":{"content":"","reasoning":""}}]}"#)
                .is_empty()
        );
        // Both in one frame: the thinking first, then the answer — the order the model wrote them.
        assert_eq!(
            thinking_and_tokens(
                r#"{"choices":[{"delta":{"reasoning":"so it is tax","content":"Taxes"}}]}"#
            ),
            ["reasoning:so it is tax", "token:Taxes"]
        );
        // Two thinking fields in one frame are one thought, never joined: the first wins.
        assert_eq!(
            thinking_and_tokens(
                r#"{"choices":[{"delta":{"reasoning":"first","reasoning_content":"second"}}]}"#
            ),
            ["reasoning:first"]
        );
        // An empty first field does not hide a real second one.
        assert_eq!(
            thinking_and_tokens(
                r#"{"choices":[{"delta":{"reasoning":"","reasoning_content":"second"}}]}"#
            ),
            ["reasoning:second"]
        );
    }

    /// Which of the two ways out a stream picks, against which server, is pinned end to end in
    /// `the_way_out_of_a_shown_thought_is_the_button_only_where_it_switches_thinking_off`.
    #[test]
    fn a_thought_stop_names_its_reason_and_the_way_out() {
        let button = "turn off Thinking for a straight answer";
        let server =
            "your model server lets this model think; set it to skip thinking, or give the \
                      model a longer context";
        for (stop, reason) in [
            (
                ThoughtStop::TooLong,
                "it thought for 5 minutes without starting its answer, so PM stopped it",
            ),
            (
                ThoughtStop::TooLarge,
                "its thinking ran past PM's size limit",
            ),
            (
                ThoughtStop::Looping,
                "its thinking got stuck repeating itself",
            ),
            (
                ThoughtStop::NoAnswer,
                "it finished thinking without writing an answer",
            ),
            (
                ThoughtStop::OutOfRoom,
                "it ran out of room while it was still thinking",
            ),
        ] {
            assert_eq!(
                stop.detail(true),
                format!("{reason} — {button}"),
                "{stop:?}"
            );
            assert_eq!(
                stop.detail(false),
                format!("{reason} — {server}"),
                "{stop:?}"
            );
        }
    }

    /// `n` bytes of pseudo-random lowercase words — thinking that never repeats itself, so only the
    /// bound under test can stop it.
    fn varied(n: usize, mut seed: u64) -> String {
        let mut out = String::with_capacity(n);
        while out.len() < n {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let c = (seed >> 33) % 27;
            out.push(if c == 26 {
                ' '
            } else {
                (b'a' + c as u8) as char
            });
        }
        out
    }

    #[test]
    fn thinking_that_is_shown_is_bounded_by_size_time_and_its_own_loop_guard() {
        let t0 = Instant::now();

        // Size: up to the cap is fine, one byte past it is not.
        let mut b = ThoughtBudget::default();
        assert_eq!(b.observe(&varied(MAX_THOUGHT_BYTES, 1), t0), None);
        assert_eq!(b.observe("x", t0), Some(ThoughtStop::TooLarge));

        // Loop: the same chunk over and over.
        let mut b = ThoughtBudget::default();
        let stops: Vec<_> = (0..60).filter_map(|_| b.observe("hm", t0)).collect();
        assert_eq!(stops.first(), Some(&ThoughtStop::Looping));

        // Time, counted from the FIRST thinking chunk.
        let mut b = ThoughtBudget::default();
        assert_eq!(b.observe("Let me see.", t0), None);
        assert_eq!(
            b.observe(" Still going.", t0 + tunables::THINKING_TIME_LIMIT),
            None
        );
        assert_eq!(
            b.observe(" And more.", t0 + Duration::from_secs(301)),
            Some(ThoughtStop::TooLong)
        );

        // A real thought: 6 KB of varied text over 200 s trips nothing.
        let mut b = ThoughtBudget::default();
        let thought = varied(6 * 1024, 7);
        let chunks: Vec<&str> = thought
            .as_bytes()
            .chunks(24)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        let step = Duration::from_secs(200) / chunks.len() as u32;
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(b.observe(chunk, t0 + step * i as u32), None, "chunk {i}");
        }
    }

    #[test]
    fn the_stall_window_shortens_once_the_model_is_generating_anything() {
        assert_eq!(chunk_deadline(false), Duration::from_secs(120));
        assert_eq!(chunk_deadline(true), Duration::from_secs(45));
    }

    const SENTINEL: &str = "THOUGHT-SENTINEL-7f3";

    /// gemma 4 on Ollama 0.33.0, thinking then answering. The middle thought carries [`SENTINEL`].
    const THINKING_SSE: &str = concat!(
        "data: {\"model\":\"gemma4\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\"The invoice \"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\"names THOUGHT-SENTINEL-7f3, \"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\"so it is tax.\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Taxes\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    /// Thinking, then a clean stop with no answer at all.
    const NO_ANSWER_SSE: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\"Hmm, which project\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    /// Thinking, then the token ceiling, still with no answer.
    const OUT_OF_ROOM_SSE: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\"Hmm, which project\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    /// One chat stream against `server`, collecting what it hands up.
    async fn stream(
        server: &MockServer,
        model: &str,
        show_thinking: bool,
    ) -> (LocalResult<Completion>, Vec<String>, Vec<String>) {
        let (mut answers, mut thoughts) = (Vec::new(), Vec::new());
        let result = stream_chat(
            &server.base_url,
            model,
            None,
            &[msg("user", "file this invoice")],
            show_thinking,
            |delta| match delta {
                StreamDelta::Answer(t) => answers.push(t.to_string()),
                StreamDelta::Thinking(t) => thoughts.push(t.to_string()),
            },
        )
        .await;
        (result, answers, thoughts)
    }

    #[tokio::test]
    async fn shown_thinking_sends_the_plain_body_and_never_asks_the_version() {
        let server = mock_stream_server(200, "0.33.0", false, THINKING_SSE);
        let (result, answers, thoughts) = stream(&server, "gemma4", true).await;
        // The body PM sent before #852: no switch, and no `/api/version` question either.
        assert_eq!(server.take(), ["/v1/chat/completions gemma4 plain"]);
        assert_eq!(result.unwrap().text, "Taxes");
        assert_eq!(answers, ["Taxes"]);
        assert_eq!(
            thoughts,
            [
                "The invoice ",
                "names THOUGHT-SENTINEL-7f3, ",
                "so it is tax."
            ]
        );
    }

    #[tokio::test]
    async fn hidden_thinking_is_the_852_request() {
        let server = mock_stream_server(200, "0.33.0", false, THINKING_SSE);
        let (result, answers, thoughts) = stream(&server, "gemma4", false).await;
        assert_eq!(
            server.take(),
            ["/api/version", "/v1/chat/completions gemma4 switch"]
        );
        assert_eq!(result.unwrap().text, "Taxes");
        assert_eq!(answers, ["Taxes"]);
        // The transcript thinks regardless, and none of it is forwarded.
        assert!(thoughts.is_empty(), "{thoughts:?}");
    }

    #[tokio::test]
    async fn a_shown_chat_changes_nothing_background_sends() {
        let server = mock_stream_server(200, "0.33.0", false, THINKING_SSE);
        stream(&server, "gemma4", true).await.0.unwrap();
        ask(&server, "gemma4").await.unwrap();
        assert_eq!(
            server.take(),
            [
                "/v1/chat/completions gemma4 plain",
                "/api/version",
                "/v1/chat/completions gemma4 switch"
            ]
        );
    }

    #[tokio::test]
    async fn a_refused_thinking_request_is_not_resent_and_remembers_nothing() {
        let server = mock_stream_server(200, "0.33.0", true, THINKING_SSE);
        let (result, answers, thoughts) = stream(&server, "m", true).await;
        let Err(err) = result else {
            panic!("a refused request must fail");
        };
        assert_eq!(err.kind, LocalFailKind::ClientError(400));
        assert!(answers.is_empty() && thoughts.is_empty());
        // One request: it carried no switch, so there was nothing to resend without.
        assert_eq!(server.take(), ["/v1/chat/completions m plain"]);
        // And nothing was learned from it: background work still asks, and still switches first.
        assert!(ask(&server, "m").await.is_err());
        assert_eq!(
            server.take(),
            [
                "/api/version",
                "/v1/chat/completions m switch",
                "/v1/chat/completions m plain"
            ]
        );
    }

    /// The thought is never the answer: not in an answer delta, not in `Completion.text`, so nothing
    /// downstream — `messages`, the vault, the index, a summary — can ever be handed it.
    #[tokio::test]
    async fn the_thought_never_reaches_the_answer() {
        assert!(THINKING_SSE.contains(SENTINEL), "the transcript carries it");
        let server = mock_stream_server(200, "0.33.0", false, THINKING_SSE);

        let (result, answers, thoughts) = stream(&server, "gemma4", true).await;
        assert!(thoughts.concat().contains(SENTINEL));
        assert!(answers.iter().all(|a| !a.contains(SENTINEL)), "{answers:?}");
        assert!(!result.unwrap().text.contains(SENTINEL));

        let (result, answers, thoughts) = stream(&server, "gemma4", false).await;
        assert!(thoughts.is_empty(), "{thoughts:?}");
        assert!(answers.iter().all(|a| !a.contains(SENTINEL)), "{answers:?}");
        assert!(!result.unwrap().text.contains(SENTINEL));
    }

    #[tokio::test]
    async fn a_thought_with_no_answer_is_an_unfinished_thought() {
        for show in [true, false] {
            for (sse, stop) in [
                (NO_ANSWER_SSE, ThoughtStop::NoAnswer),
                (OUT_OF_ROOM_SSE, ThoughtStop::OutOfRoom),
            ] {
                let server = mock_stream_server(200, "0.33.0", false, sse);
                let (result, answers, _) = stream(&server, "gemma4", show).await;
                let Err(err) = result else {
                    panic!("a reply with no answer is not a turn (show={show}, {stop:?})");
                };
                assert_eq!(err.kind, LocalFailKind::UnfinishedThought);
                assert_eq!(err.detail, stop.detail(show), "show={show}");
                assert!(answers.is_empty());
            }
        }
        // The wording the stop arm promises, end to end.
        let server = mock_stream_server(200, "0.33.0", false, NO_ANSWER_SSE);
        let Err(LocalFailure { detail, .. }) = stream(&server, "gemma4", true).await.0 else {
            panic!("a reply with no answer is not a turn");
        };
        assert!(
            detail.starts_with("it finished thinking without writing an answer — "),
            "{detail}"
        );
    }

    #[tokio::test]
    async fn shown_thinking_that_loops_is_stopped_hidden_is_not() {
        let wire: &'static str = Box::leak(thinking_loop_wire().into_boxed_str());
        let server = mock_stream_server(200, "0.33.0", false, wire);

        let (result, answers, thoughts) = stream(&server, "gemma4", true).await;
        let Err(err) = result else {
            panic!("eighty identical thoughts in a row is a loop when shown");
        };
        assert_eq!(err.kind, LocalFailKind::UnfinishedThought);
        assert_eq!(err.detail, ThoughtStop::Looping.detail(true));
        assert!(answers.is_empty());
        assert!(thoughts.len() < 80, "stopped before the end");

        // Hidden, the same thinking is liveness only — nothing guards what nothing shows.
        let (result, answers, thoughts) = stream(&server, "gemma4", false).await;
        assert_eq!(result.unwrap().text, "Taxes");
        assert_eq!(answers, ["Taxes"]);
        assert!(thoughts.is_empty());
    }

    /// "Turn off Thinking" is only advice where turning it off sends a different request. On a server
    /// PM can't switch it sends the same one, the model thinks just the same, and the thought is
    /// then hidden — past every bound PM puts on shown thinking — so the way out is on the server.
    #[tokio::test]
    async fn the_way_out_of_a_shown_thought_is_the_button_only_where_it_switches_thinking_off() {
        let looping: &'static str = Box::leak(thinking_loop_wire().into_boxed_str());
        let shown_stop = |server: &MockServer, model: &'static str| {
            let base_url = server.base_url.clone();
            async move {
                let result = stream_chat(
                    &base_url,
                    model,
                    None,
                    &[msg("user", "file this invoice")],
                    true,
                    |_| {},
                )
                .await;
                let Err(err) = result else {
                    panic!("{model}: a shown thought with no answer is not a turn");
                };
                assert_eq!(err.kind, LocalFailKind::UnfinishedThought);
                err.detail
            }
        };

        // A recent Ollama: the button switches thinking off, so it is the way out. The version is
        // asked only once the stream has failed — a shown thought that answers never asks it.
        let ollama = mock_stream_server(200, "0.33.0", false, looping);
        assert_eq!(
            shown_stop(&ollama, "gemma4").await,
            ThoughtStop::Looping.detail(true)
        );
        assert_eq!(
            ollama.take(),
            ["/v1/chat/completions gemma4 plain", "/api/version"]
        );

        // llama-server (no `/api/version` route) and an Ollama too old to take the switch: the
        // server decides, so the server is the way out — for a stopped thought and an unanswered one.
        for (status, version) in [(404, ""), (200, "0.11.4")] {
            for (sse, stop) in [
                (looping, ThoughtStop::Looping),
                (NO_ANSWER_SSE, ThoughtStop::NoAnswer),
            ] {
                let server = mock_stream_server(status, version, false, sse);
                let detail = shown_stop(&server, "gemma4").await;
                assert_eq!(detail, stop.detail(false), "{status} {version} {stop:?}");
                assert!(!detail.contains("turn off Thinking"), "{detail}");
            }
        }

        // A recent Ollama, but a model that refused the switch: for that model the button can't
        // switch anything off, while the next model on the same server still can.
        let server = serve(200, "0.33.0", |m| m == "qwen3", false, Some(looping));
        ask(&server, "qwen3").await.unwrap();
        assert_eq!(
            server.take(),
            [
                "/api/version",
                "/v1/chat/completions qwen3 switch",
                "/v1/chat/completions qwen3 plain"
            ]
        );
        assert_eq!(
            shown_stop(&server, "qwen3").await,
            ThoughtStop::Looping.detail(false)
        );
        assert_eq!(
            shown_stop(&server, "gemma4").await,
            ThoughtStop::Looping.detail(true)
        );
    }
}
