// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Local-endpoint runtime discipline for the provider seam (#297): the single-inference slot
//! (chat preempts background on a one-GPU machine), the dead-host circuit breaker, the endpoint
//! http-posture classifier, and the per-endpoint context-window cache. The wire client itself
//! ([`crate::openai_compat`]) is pure I/O; the *policy* around it lives here.
//!
//! Everything that can be decided without a socket is a pure function tested below: the cooldown
//! reducer ([`HealthState::observe`]), the failure→outcome mapping, and the endpoint classifier.
//! The concurrency ([`LocalSlot`]) and its preemption are the thin async edge — real cancellation
//! (a dropped reqwest future), verified on the epic's live rig, not in CI.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::openai_compat::{LocalFailKind, WindowInfo};

// =================================================================================================
// TUNABLES — the ONE place to tune local-endpoint behaviour after live testing.
// =================================================================================================
//
// These are REASONED STARTING VALUES, not measured ones: they come from documented ecosystem
// defaults (llama.cpp / Ollama / LM Studio, the OpenAI client libraries, reqwest, and standard
// circuit-breaker / backoff guidance — Envoy outlier-detection, resilience4j, AWS "backoff and
// jitter"), cross-checked in 2026-07. Tuning after real-server testing must be a one-file edit
// here — never a hunt through the gateway. Where a value departs from the epic's baseline, the
// reason is in the doc comment on the constant.
//
// The BACKGROUND retry policy lives here too: a preempted or warming-up background call retries
// in-process, jittered, until a **total-elapsed budget** is spent, before it defers to its scheduler
// — the shape is in preemption_retry_delay() / loading_retry_backoff() and the gateway's
// run_local_complete. The budget is a total-elapsed cap (not an attempt count) because the case this
// exists for — a host that ANSWERED "model loading" — must be able to span a real cold model load
// (30-60s+), which an attempt count with a short backoff cannot honestly express. The two reasons get
// DIFFERENT budgets: a warming model is worth waiting out (LOADING_RETRY_BUDGET ≈ the cold-load window
// the TTFT timeout also covers), a busy GPU is not (PREEMPTION_RETRY_BUDGET is short — defer to the
// idle-gated scheduler, which is the right backstop for "chat is using the GPU"). Foreground chat is
// never retried (interactive: cloud-fallback pre-first-token, or surface the error). The retry stays
// NARROW so it can never become the hot loop against a reloading server the research warns against:
// only a PREEMPTION (not a fault) or a "model loading" 503 (alive, warming) is retried — every hard
// failure (refused / timeout / 5xx) skips the loop and falls back to cloud, or defers to the circuit
// breaker + the next scheduler tick.
//
// COMPOUND WORST CASE for one background job on the local path: LOADING_RETRY_BUDGET (~120s of fast
// "503 loading" rechecks) PLUS — only if a final recheck then accepts the request and hangs — one
// BACKGROUND_TOTAL_TIMEOUT (180s) before that Timeout strikes and exits: ~5 min absolute maximum, but
// ~120s in the realistic cold-load case (the model loads and a request succeeds, or we fall to cloud
// at the budget). It does NOT compound with the streaming TTFT timeout — that is the FOREGROUND
// (stream_chat) path, which has no loading-retry (it falls back to cloud pre-first-token instead). And
// it never LOCKS the slot for that long: chat preempts any in-flight attempt and walks into the free
// lane during the backoff sleeps, which are OUTSIDE `run_background` so the lane is not held (pinned by
// `a_returned_background_call_frees_the_slot_for_foreground` +
// `foreground_preempts_an_in_flight_background_call`).
pub mod tunables {
    use std::time::Duration;

    /// Connect timeout for a loopback endpoint. A loopback connect completes sub-millisecond and a
    /// dead port RSTs instantly, so this only has to cover a server whose accept backlog is briefly
    /// saturated while it loads a model. Baseline; even 1s would be defensible.
    pub const CONNECT_TIMEOUT_LOOPBACK: Duration = Duration::from_secs(2);

    /// Connect timeout for a remote (LAN / Tailscale) endpoint. Slightly conservative vs the OpenAI
    /// client's 5s so slow LAN/DNS still connects; tighten toward 5s for faster failure if wanted.
    pub const CONNECT_TIMEOUT_REMOTE: Duration = Duration::from_secs(10);

    /// How long to wait for the FIRST content token. This absorbs a silent cold model load —
    /// Ollama and LM Studio JIT-load a model on the first request and stream NOTHING until the
    /// first token (5-30s typical, >60s for a large model on a slow disk) — plus prompt prefill.
    /// Generous by design: the OpenAI client's own whole-request default is 600s, so 120s is not
    /// aggressive. A cold load is surfaced to the UI as "loading model…", never a silent hang.
    pub const TIME_TO_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(120);

    /// Silence allowed BETWEEN tokens once streaming has started — the short deadline that catches a
    /// genuinely wedged stream. RAISED from the 30s baseline to 45s: llama.cpp emits an SSE keepalive
    /// ping every 30s (`--sse-ping-interval` default), and a 30s inter-token deadline would race that
    /// ping. 45s guarantees a ping (which arrives as bytes and resets this timer) lands first, while
    /// still catching a dead stream fast. Ollama/LM Studio send no pings, so a real stall is still
    /// caught inside the window.
    pub const INTER_TOKEN_TIMEOUT: Duration = Duration::from_secs(45);

    /// Total wall-clock budget for a NON-streaming background completion (summaries, titles, prefs).
    /// With no token-level signal to lean on, this single deadline must cover a cold load plus the
    /// whole (short) generation. 180s = ~60s worst-case load + generation headroom. A dead host is
    /// still caught fast by the connect timeout; this only bounds an accepted-then-wedged request.
    pub const BACKGROUND_TOTAL_TIMEOUT: Duration = Duration::from_secs(180);

    /// Per-request deadline for the `/v1/models` reachability probe — a wrong URL must fail fast.
    pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

    /// Per-request deadline for the `/slots` and `/v1/models` context-window probes.
    pub const WINDOW_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

    /// Consecutive hard failures that trip the dead-host cooldown. A consecutive-count trip (not an
    /// error-rate window) is the right model for a low-volume single backend. Envoy's infra default
    /// is 5; 3 is deliberately more sensitive for an interactive desktop app — move toward 5 if
    /// transient blips trip it too eagerly.
    pub const COOLDOWN_FAILURE_THRESHOLD: u32 = 3;

    /// Base cooldown once tripped. Escalates linearly by ejection count (60s, 120s, 180s…) capped at
    /// [`COOLDOWN_MAX`], matching Envoy's `base_ejection_time * ejections` and resilience4j's 60s
    /// open-state. Cleared early by any success or a manual re-probe (half-open).
    pub const COOLDOWN_BASE: Duration = Duration::from_secs(60);

    /// Ceiling on the escalating cooldown — a persistently-dead host is retried at most this rarely,
    /// never longer. Matches Envoy's `max_ejection_time`.
    pub const COOLDOWN_MAX: Duration = Duration::from_secs(300);

    /// Minimum gap between backend health probes triggered by `local_llm_status`. The Local AI tab
    /// polls status on a ~30s cadence; this debounce means a burst of UI polls can never spam the
    /// user's server with probes — the backend actually re-probes at most this often.
    pub const HEALTH_PROBE_DEBOUNCE: Duration = Duration::from_secs(30);

    /// Consecutive IDENTICAL streamed tokens that trip the degenerate-stream guard's fast path. 50
    /// in a row is almost certainly a broken small model; legitimate output effectively never does
    /// this, so the false-positive risk is negligible. Complements the period-based detector (which
    /// needs a longer byte run to trip) by killing a single-token loop sooner. Endorsed by the
    /// timeout/resilience research pass as safely conservative.
    pub const LOOP_GUARD_SAME_TOKEN_RUN: usize = 50;
    // NOTE the period-based loop detector's own thresholds (min cycles / min cover) live next to it
    // in `openai_compat` and are documented there. The epic baseline of "3 n-gram repeats" was NOT
    // adopted: 3 repeats false-positive on tables, code, and enumerations (the research pass and the
    // existing `loop_guard_leaves_legitimate_repetition_alone` test both confirm this), so the
    // period detector's 6-cycle / 768-byte requirement is the researched, false-positive-safe form.

    // --- Background retry policy — "background waits and retries" (#297), total-elapsed-bounded. ---

    /// Total in-process wait for a foreground chat to free the single GPU slot before a PREEMPTED
    /// background job defers to its (idle-gated) scheduler for a next-tick retry (cursor unadvanced).
    /// Short on purpose: a busy GPU is exactly what the idle scheduler backstop is for, so there is no
    /// point waiting out a long chat session in-process — a user in a rapid back-and-forth must not
    /// trap the job. (Preemption retries are cheap: they mostly block on the slot's lane until chat
    /// yields, they do not hit the server.)
    pub const PREEMPTION_RETRY_BUDGET: Duration = Duration::from_secs(20);

    /// Per-retry pause after a preemption. Flat, not escalating — a preemption is not a fault, so there
    /// is nothing to back off from; [`super::preemption_retry_delay`] jitters it to `[base/2, 3·base/2]`
    /// so a retry never fires instantly back into a still-active chat.
    pub const PREEMPTION_RETRY_DELAY: Duration = Duration::from_secs(3);

    /// Total in-process wait for a host that keeps answering "model loading" before we give up on the
    /// warm-up (local-then-cloud → cloud; local-only → a clear "still loading" error + a next-tick
    /// retry). Sized to the cold-load window the TTFT timeout also covers (Ollama/LM Studio JIT-loading
    /// a model can take 30-60s+), so a warming model completes locally instead of being abandoned after
    /// a few seconds. A hard failure is NEVER retried, so this only ever spans a genuinely-warming host.
    pub const LOADING_RETRY_BUDGET: Duration = Duration::from_secs(120);

    /// Base for the exponential + full-jitter backoff (AWS "backoff and jitter") between "model
    /// loading" rechecks: recheck N waits a uniform-random `[0, min(CAP, BASE · 2^(N-1))]`
    /// ([`super::loading_retry_backoff`]).
    pub const RETRY_BASE_BACKOFF: Duration = Duration::from_secs(2);

    /// Cap on a single "model loading" backoff step, so rechecks stay frequent late in
    /// [`LOADING_RETRY_BUDGET`] — a loaded model is then noticed within ~this long, not a full doubling.
    pub const RETRY_BACKOFF_CAP: Duration = Duration::from_secs(15);
}

// =================================================================================================
// Background retry timing — the jittered delays the gateway sleeps between in-process retries of a
// preempted or warming-up BACKGROUND local call. Pure but for the RNG; the bounds are unit-tested.
// =================================================================================================

/// A uniformly-random `Duration` in `[0, span]` for backoff jitter, from the app's `getrandom`
/// source (same as everywhere else). On the astronomically-unlikely RNG error it returns the full
/// `span` — jitter is advisory, so a deterministic fall-back is fine and never zero-waits a retry.
fn jitter_up_to(span: Duration) -> Duration {
    let nanos = span.as_nanos();
    if nanos == 0 {
        return Duration::ZERO;
    }
    let mut buf = [0u8; 8];
    let r = match getrandom::fill(&mut buf) {
        Ok(()) => u128::from(u64::from_le_bytes(buf)),
        Err(_) => return span,
    };
    // `nanos` fits u64 for the small spans used here (well under a minute), so the cast is lossless.
    Duration::from_nanos((r % (nanos + 1)) as u64)
}

/// The jittered wait before retrying a PREEMPTED background call: at least half
/// [`tunables::PREEMPTION_RETRY_DELAY`] (so a retry can't fire instantly back into a still-active
/// chat and immediately re-preempt) plus up to a further full delay of jitter — i.e. `[base/2, 3·base/2]`.
pub fn preemption_retry_delay() -> Duration {
    let base = tunables::PREEMPTION_RETRY_DELAY;
    base / 2 + jitter_up_to(base)
}

/// AWS full-jitter exponential backoff for retrying a warming-up ("model loading") host on 1-based
/// `attempt`: a uniform-random wait in `[0, min(RETRY_BACKOFF_CAP, BASE · 2^(attempt-1))]`. The cap
/// keeps rechecks frequent late in [`tunables::LOADING_RETRY_BUDGET`]; the saturating shift/multiply
/// mean a large attempt can never overflow (it just pins to the cap).
pub fn loading_retry_backoff(attempt: u32) -> Duration {
    let factor = 1u32
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    let uncapped = tunables::RETRY_BASE_BACKOFF
        .checked_mul(factor)
        .unwrap_or(tunables::RETRY_BACKOFF_CAP);
    jitter_up_to(uncapped.min(tunables::RETRY_BACKOFF_CAP))
}

// =================================================================================================
// Endpoint http-posture classifier — pure. The threat model is that the USER'S model server may be
// exposed, not that PM is attackable. We refuse to send a token + chats in the clear to a PUBLIC
// host, but tolerate http on loopback and private ranges (a LAN llama-server has no TLS story).
// =================================================================================================

/// Where a resolved endpoint address sits, for the http-vs-https posture decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointClass {
    /// 127.0.0.0/8 or ::1 — the server runs on this machine.
    Loopback,
    /// An RFC1918 / CGNAT / link-local / IPv6-ULA address — a server on the user's own network
    /// (a home LAN box, a Tailscale peer). Reachable only from inside that network.
    PrivateRemote,
    /// A globally-routable address — anyone on the internet may be able to reach it too.
    PublicRemote,
}

/// Classify a resolved IP. IMPORTANT: callers must classify the RESOLVED address, never the
/// hostname string — `localhost` (and any name) can resolve to a non-loopback address, and trusting
/// the string would let a "localhost" endpoint quietly send chats off the machine.
pub fn classify_ip(ip: IpAddr) -> EndpointClass {
    if ip.is_loopback() {
        return EndpointClass::Loopback;
    }
    let private = match ip {
        IpAddr::V4(v4) => {
            v4.is_private()            // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()  // 169.254/16
                // 100.64/10 CGNAT (Tailscale's default range) — treat as private.
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    };
    if private {
        EndpointClass::PrivateRemote
    } else {
        EndpointClass::PublicRemote
    }
}

/// The verdict for an (scheme, class) pair: allowed silently, allowed with a plain warning, or
/// refused. Copy for the warning/refusal is honest that PM cannot secure a server it does not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostureVerdict {
    /// https anywhere, or http to loopback — send it, no note needed beyond the loopback story.
    Ok,
    /// http to a private-network host — allowed, but the traffic (token + chats) is unencrypted on
    /// that network. PM can't secure a server it doesn't run.
    WarnUnencrypted,
    /// http to a public host — refused. A bearer token and chat text in the clear over the internet
    /// is never acceptable; the user must use https.
    RefusePublicCleartext,
}

/// Decide the posture for a scheme (`"http"`/`"https"`) against a resolved endpoint class. Pure.
pub fn posture_for(scheme: &str, class: EndpointClass) -> PostureVerdict {
    if scheme.eq_ignore_ascii_case("https") {
        return PostureVerdict::Ok;
    }
    // http from here down.
    match class {
        EndpointClass::Loopback => PostureVerdict::Ok,
        EndpointClass::PrivateRemote => PostureVerdict::WarnUnencrypted,
        EndpointClass::PublicRemote => PostureVerdict::RefusePublicCleartext,
    }
}

// =================================================================================================
// Dead-host circuit breaker — pure reducer. `observe` folds a call's outcome into the health state;
// `available` decides whether a route may try the local host right now (half-open after cooldown).
// =================================================================================================

/// What a completed local call means for host health. Derived from the typed [`LocalFailKind`] (or
/// a success / a preemption) so the failure→policy mapping is decided ONCE, here, and tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    /// The call succeeded — the host is healthy; clear everything.
    Ok,
    /// A hard failure that counts toward the dead-host cooldown (refused, timeout, malformed or
    /// degenerate stream, 5xx, oversized reply).
    Strike,
    /// The host answered but the call didn't succeed for a reason that means it is ALIVE, not dead:
    /// a 503 "model loading" (warming up) or a 4xx (a config/model-id problem). Never a strike;
    /// clears the strike streak because the host is demonstrably responding.
    Alive,
    /// The call was preempted by a higher-priority (chat) request. Not the host's fault — leaves
    /// health untouched.
    Neutral,
}

impl CallOutcome {
    /// Map a wire failure to its health meaning. The single home for "which failures are the host's
    /// fault"; nothing downstream re-decides this by inspecting a `LocalFailKind` again.
    pub fn for_failure(kind: &LocalFailKind) -> Self {
        match kind {
            LocalFailKind::ModelLoading | LocalFailKind::ClientError(_) => CallOutcome::Alive,
            LocalFailKind::Refused
            | LocalFailKind::Timeout
            | LocalFailKind::MalformedStream
            | LocalFailKind::DegenerateStream
            | LocalFailKind::ServerError(_)
            | LocalFailKind::ReplyTooLarge => CallOutcome::Strike,
        }
    }
}

/// The rolling health of the configured local endpoint. In-memory only (a restart re-probes); the
/// window cache and the last-probe stamp live beside it on [`LocalRuntime`].
#[derive(Clone, Copy, Debug, Default)]
pub struct HealthState {
    /// Consecutive strikes since the last success/alive signal.
    consecutive_strikes: u32,
    /// When the current cooldown ends, if the host is in one.
    cooldown_until: Option<Instant>,
    /// How many times the host has been ejected into cooldown (drives the escalation), reset on a
    /// clean success so a recovered host starts fresh.
    ejections: u32,
}

impl HealthState {
    /// Fold a call's outcome into the state at time `now`. Pure — `now` is passed in so the
    /// escalation and cooldown are unit-tested without a clock.
    pub fn observe(&mut self, outcome: CallOutcome, now: Instant) {
        match outcome {
            CallOutcome::Ok => {
                self.consecutive_strikes = 0;
                self.cooldown_until = None;
                self.ejections = 0;
            }
            CallOutcome::Alive => {
                // The host is responding — it is not dead. Clear the streak but do NOT lift an
                // active cooldown early (a loading host will succeed shortly and clear it then).
                self.consecutive_strikes = 0;
            }
            CallOutcome::Neutral => {}
            CallOutcome::Strike => {
                self.consecutive_strikes += 1;
                if self.consecutive_strikes >= tunables::COOLDOWN_FAILURE_THRESHOLD {
                    self.ejections = self.ejections.saturating_add(1);
                    self.cooldown_until = Some(now + self.cooldown_duration());
                    self.consecutive_strikes = 0;
                }
            }
        }
    }

    /// The escalating cooldown length for the current ejection count: `base * ejections`, capped.
    fn cooldown_duration(&self) -> Duration {
        let scaled = tunables::COOLDOWN_BASE
            .checked_mul(self.ejections.max(1))
            .unwrap_or(tunables::COOLDOWN_MAX);
        scaled.min(tunables::COOLDOWN_MAX)
    }

    /// Whether a route may attempt the local host at `now`. `true` when not in cooldown, and `true`
    /// again the instant a cooldown elapses (half-open: the next attempt is the probe — if it
    /// strikes, `observe` re-ejects with a longer cooldown; if it succeeds, health resets).
    pub fn available(&self, now: Instant) -> bool {
        match self.cooldown_until {
            Some(until) => now >= until,
            None => true,
        }
    }

    /// Remaining cooldown at `now` (zero when available) — for the status surface.
    pub fn cooldown_remaining(&self, now: Instant) -> Duration {
        match self.cooldown_until {
            Some(until) if until > now => until - now,
            _ => Duration::ZERO,
        }
    }

    pub fn in_cooldown(&self, now: Instant) -> bool {
        !self.available(now)
    }
}

// =================================================================================================
// The single-inference slot — one local call at a time (a consumer GPU can't run two), with chat
// preempting an in-flight background call by ABORTING it (dropping the reqwest future), not merely
// discarding its result.
// =================================================================================================

/// What happened to a slot-guarded call.
pub enum SlotOutcome<T> {
    /// The call ran to completion (its own `Result` is inside).
    Ran(T),
    /// A chat request preempted this (background) call; its future was dropped mid-flight.
    Preempted,
}

/// Serialises local inference to one call at a time and lets a foreground (chat) call preempt an
/// in-flight background call. See the module docs for the race analysis; the short version:
///   * `lane` — the one-at-a-time mutex, held for a call's duration.
///   * `chat_waiting` — set while a chat call wants/holds the lane; a background call checks it and
///     bails so chat never waits a whole background generation.
///   * `preempt` — a FRESH `Notify` per background acquire (so a stale permit from a prior preempt
///     can't cancel a new call); chat fires `notify_one` on it, which a background call consumes via
///     `select!` even if it registers after the notify — closing the register race.
#[derive(Default)]
pub struct LocalSlot {
    lane: tokio::sync::Mutex<()>,
    chat_waiting: AtomicBool,
    preempt: Mutex<Arc<Notify>>,
}

/// Resets `chat_waiting` to `false` on drop — including on an unwinding panic — so a chat call that
/// panics mid-flight can never wedge the slot into "chat forever waiting".
struct ChatWaitingGuard<'a>(&'a AtomicBool);
impl Drop for ChatWaitingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl LocalSlot {
    /// Run a FOREGROUND (chat) call with priority: signal any in-flight background call to abort,
    /// take the lane, run to completion (chat is never itself preempted). The future is awaited
    /// as-is; cancellation only flows background← chat, never the reverse.
    pub async fn run_foreground<F>(&self, fut: F) -> F::Output
    where
        F: std::future::Future,
    {
        self.chat_waiting.store(true, Ordering::SeqCst);
        let _waiting = ChatWaitingGuard(&self.chat_waiting);
        // Wake a background call that is already registered on the current notify. `notify_one`
        // (not `notify_waiters`) also stores a permit for a background call that registers a hair
        // later, so the preemption can't be missed in the register-race window.
        if let Ok(n) = self.preempt.lock() {
            n.notify_one();
        }
        let _lane = self.lane.lock().await; // waits for the (now-cancelling) background to drop it
        fut.await
    }

    /// Run a BACKGROUND call that yields the lane to chat. Installs a fresh preemption `Notify`,
    /// bails immediately if chat is already waiting, then races the call against a preemption; on
    /// preemption the call future is DROPPED (a real reqwest abort) and [`SlotOutcome::Preempted`]
    /// is returned for the caller to retry on its next scheduler tick.
    pub async fn run_background<F>(&self, fut: F) -> SlotOutcome<F::Output>
    where
        F: std::future::Future,
    {
        let _lane = self.lane.lock().await;
        let notify = Arc::new(Notify::new());
        if let Ok(mut slot) = self.preempt.lock() {
            *slot = notify.clone();
        }
        // Close the register race: if chat is already waiting, don't even start.
        if self.chat_waiting.load(Ordering::SeqCst) {
            return SlotOutcome::Preempted;
        }
        let notified = notify.notified();
        tokio::pin!(notified);
        tokio::select! {
            biased;
            _ = &mut notified => SlotOutcome::Preempted,
            out = fut => SlotOutcome::Ran(out),
        }
    }
}

// =================================================================================================
// The runtime state parked on AppState: the slot, the circuit-breaker health, a per-(base,model)
// context-window cache, and the last-probe stamp for the health debounce. All in-memory.
// =================================================================================================

/// In-memory local-endpoint runtime. Reset on restart by design: a restart may change the server's
/// loaded model / `n_ctx`, so re-probing once is cheaper and more honest than persisting stale
/// windows, and the health/cooldown should not survive a relaunch either.
#[derive(Default)]
pub struct LocalRuntime {
    pub slot: LocalSlot,
    health: Mutex<HealthState>,
    windows: Mutex<std::collections::HashMap<String, WindowInfo>>,
    last_probe: Mutex<Option<Instant>>,
}

impl LocalRuntime {
    /// Record a call's outcome against host health (poison-tolerant — health is advisory).
    pub fn record(&self, outcome: CallOutcome) {
        if let Ok(mut h) = self.health.lock() {
            h.observe(outcome, Instant::now());
        }
    }

    /// A snapshot of the current health (poison-tolerant).
    pub fn health(&self) -> HealthState {
        self.health.lock().map(|h| *h).unwrap_or_default()
    }

    /// Whether the local host may be attempted right now (not in cooldown).
    pub fn available(&self) -> bool {
        self.health().available(Instant::now())
    }

    /// The cache key for a context window: the endpoint and model together (a restart can change
    /// either, and two models on one endpoint have different windows).
    fn window_key(base_url: &str, model: &str) -> String {
        format!("{base_url}::{model}")
    }

    pub fn cached_window(&self, base_url: &str, model: &str) -> Option<WindowInfo> {
        let key = Self::window_key(base_url, model);
        self.windows.lock().ok()?.get(&key).copied()
    }

    pub fn cache_window(&self, base_url: &str, model: &str, info: WindowInfo) {
        if let Ok(mut w) = self.windows.lock() {
            w.insert(Self::window_key(base_url, model), info);
        }
    }

    /// Whether enough time has passed since the last health probe to run another (the UI-poll
    /// debounce). Records `now` as the last probe when it returns `true`.
    pub fn probe_debounce_elapsed(&self) -> bool {
        let now = Instant::now();
        if let Ok(mut last) = self.last_probe.lock() {
            let due = last.is_none_or(|t| now.duration_since(t) >= tunables::HEALTH_PROBE_DEBOUNCE);
            if due {
                *last = Some(now);
            }
            due
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    // ---- endpoint classification + http posture ----

    #[test]
    fn loopback_addresses_classify_as_loopback() {
        assert_eq!(
            classify_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            EndpointClass::Loopback
        );
        assert_eq!(
            classify_ip(IpAddr::V4(Ipv4Addr::new(127, 3, 2, 1))),
            EndpointClass::Loopback,
            "all of 127/8 is loopback"
        );
        assert_eq!(
            classify_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            EndpointClass::Loopback
        );
    }

    #[test]
    fn private_ranges_classify_as_private_remote() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            IpAddr::V4(Ipv4Addr::new(172, 16, 4, 4)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)), // link-local
            IpAddr::V4(Ipv4Addr::new(100, 100, 3, 4)), // Tailscale CGNAT
        ] {
            assert_eq!(
                classify_ip(ip),
                EndpointClass::PrivateRemote,
                "{ip} is private"
            );
        }
        // fe80:: link-local and fc00:: ULA are private too.
        assert_eq!(
            classify_ip(IpAddr::V6("fe80::1".parse().unwrap())),
            EndpointClass::PrivateRemote
        );
        assert_eq!(
            classify_ip(IpAddr::V6("fd12:3456::1".parse().unwrap())),
            EndpointClass::PrivateRemote
        );
    }

    #[test]
    fn public_addresses_classify_as_public_remote() {
        assert_eq!(
            classify_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))),
            EndpointClass::PublicRemote
        );
        assert_eq!(
            classify_ip(IpAddr::V4(Ipv4Addr::new(100, 200, 3, 4))), // outside 100.64/10
            EndpointClass::PublicRemote
        );
        assert_eq!(
            classify_ip(IpAddr::V6("2606:4700::1111".parse().unwrap())),
            EndpointClass::PublicRemote
        );
    }

    #[test]
    fn http_posture_refuses_only_public_cleartext() {
        // https is always fine.
        for class in [
            EndpointClass::Loopback,
            EndpointClass::PrivateRemote,
            EndpointClass::PublicRemote,
        ] {
            assert_eq!(posture_for("https", class), PostureVerdict::Ok);
            assert_eq!(
                posture_for("HTTPS", class),
                PostureVerdict::Ok,
                "scheme is case-insensitive"
            );
        }
        // http: fine on loopback, warned on a private network, refused to a public host.
        assert_eq!(
            posture_for("http", EndpointClass::Loopback),
            PostureVerdict::Ok
        );
        assert_eq!(
            posture_for("http", EndpointClass::PrivateRemote),
            PostureVerdict::WarnUnencrypted
        );
        assert_eq!(
            posture_for("http", EndpointClass::PublicRemote),
            PostureVerdict::RefusePublicCleartext
        );
    }

    // ---- cooldown reducer ----

    #[test]
    fn strikes_below_threshold_do_not_trip_cooldown() {
        let t0 = Instant::now();
        let mut h = HealthState::default();
        for _ in 0..(tunables::COOLDOWN_FAILURE_THRESHOLD - 1) {
            h.observe(CallOutcome::Strike, t0);
        }
        assert!(h.available(t0), "under the threshold the host stays usable");
        assert_eq!(h.cooldown_remaining(t0), Duration::ZERO);
    }

    #[test]
    fn threshold_strikes_trip_an_escalating_cooldown() {
        let t0 = Instant::now();
        let mut h = HealthState::default();
        for _ in 0..tunables::COOLDOWN_FAILURE_THRESHOLD {
            h.observe(CallOutcome::Strike, t0);
        }
        assert!(h.in_cooldown(t0), "the threshold trips a cooldown");
        assert!(
            h.available(t0 + tunables::COOLDOWN_BASE),
            "first cooldown lasts the base duration"
        );

        // A second ejection escalates to base*2.
        for _ in 0..tunables::COOLDOWN_FAILURE_THRESHOLD {
            h.observe(CallOutcome::Strike, t0 + tunables::COOLDOWN_BASE);
        }
        let expect2 = tunables::COOLDOWN_BASE * 2;
        assert!(
            !h.available(t0 + tunables::COOLDOWN_BASE + expect2 - Duration::from_secs(1)),
            "the second cooldown is longer than the first"
        );
    }

    #[test]
    fn cooldown_is_capped() {
        let t0 = Instant::now();
        let mut h = HealthState::default();
        // Drive many ejections; the cooldown must never exceed the cap.
        for _ in 0..20 {
            for _ in 0..tunables::COOLDOWN_FAILURE_THRESHOLD {
                h.observe(CallOutcome::Strike, t0);
            }
        }
        assert!(
            h.available(t0 + tunables::COOLDOWN_MAX),
            "cooldown is capped at COOLDOWN_MAX no matter how many ejections"
        );
    }

    #[test]
    fn success_clears_everything_and_resets_escalation() {
        let t0 = Instant::now();
        let mut h = HealthState::default();
        for _ in 0..tunables::COOLDOWN_FAILURE_THRESHOLD {
            h.observe(CallOutcome::Strike, t0);
        }
        assert!(h.in_cooldown(t0));
        h.observe(CallOutcome::Ok, t0);
        assert!(h.available(t0), "a success lifts the cooldown immediately");
        // And escalation is reset: the next trip is base again, not base*2.
        for _ in 0..tunables::COOLDOWN_FAILURE_THRESHOLD {
            h.observe(CallOutcome::Strike, t0);
        }
        assert!(h.available(t0 + tunables::COOLDOWN_BASE));
    }

    #[test]
    fn alive_outcomes_do_not_strike_and_break_a_streak() {
        let t0 = Instant::now();
        let mut h = HealthState::default();
        // Two strikes, then an "alive" signal (503 loading / 4xx) resets the streak, so the next
        // strike doesn't reach the threshold.
        h.observe(CallOutcome::Strike, t0);
        h.observe(CallOutcome::Strike, t0);
        h.observe(CallOutcome::Alive, t0);
        h.observe(CallOutcome::Strike, t0);
        assert!(h.available(t0), "an alive signal broke the strike streak");
    }

    #[test]
    fn failure_kinds_map_to_the_right_outcome() {
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::Refused),
            CallOutcome::Strike
        );
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::Timeout),
            CallOutcome::Strike
        );
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::ServerError(500)),
            CallOutcome::Strike
        );
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::ModelLoading),
            CallOutcome::Alive,
            "a warming-up host is alive, never a strike"
        );
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::ClientError(404)),
            CallOutcome::Alive,
            "a 4xx means the host answered — config problem, not a dead host"
        );
    }

    // ---- background retry timing (bounds over many RNG draws) ----

    #[test]
    fn preemption_delay_is_bounded_and_never_instant() {
        let base = tunables::PREEMPTION_RETRY_DELAY;
        for _ in 0..2000 {
            let d = preemption_retry_delay();
            assert!(
                d >= base / 2,
                "a preemption retry must never fire instantly back into an active chat"
            );
            assert!(d <= base / 2 + base, "bounded above at 3·base/2");
        }
    }

    #[test]
    fn loading_backoff_stays_within_the_capped_exponential_ceiling() {
        for attempt in 1..=10u32 {
            let uncapped = tunables::RETRY_BASE_BACKOFF
                .checked_mul(1u32 << (attempt - 1))
                .unwrap_or(tunables::RETRY_BACKOFF_CAP);
            let ceil = uncapped.min(tunables::RETRY_BACKOFF_CAP);
            for _ in 0..2000 {
                let d = loading_retry_backoff(attempt);
                assert!(
                    d <= ceil,
                    "attempt {attempt}: full-jitter backoff must stay within [0, min(cap, base·2^(n-1))]"
                );
            }
        }
        // Late attempts pin to the cap, and an absurd attempt count saturates rather than panicking.
        assert!(loading_retry_backoff(20) <= tunables::RETRY_BACKOFF_CAP);
        assert!(loading_retry_backoff(u32::MAX) <= tunables::RETRY_BACKOFF_CAP);
    }

    #[test]
    fn jitter_up_to_zero_span_is_zero() {
        assert_eq!(jitter_up_to(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn window_cache_round_trips_per_endpoint_and_model() {
        use crate::openai_compat::{WindowInfo, WindowSource};
        let rt = LocalRuntime::default();
        let a = WindowInfo {
            tokens: 8192,
            source: WindowSource::Slots,
        };
        rt.cache_window("http://localhost:11434", "llama3.2", a);
        assert_eq!(
            rt.cached_window("http://localhost:11434", "llama3.2"),
            Some(a)
        );
        // A different model on the same endpoint is a separate entry.
        assert_eq!(rt.cached_window("http://localhost:11434", "qwen2.5"), None);
    }

    // ---- the single-inference slot: the "chat always wins" invariant (async) ----

    /// A returned background call FREES the lane. [`LocalSlot::run_background`] holds the lane only for
    /// its own duration; once it returns, the lane is free. This is why the gateway's retry backoffs —
    /// which sleep OUTSIDE `run_background`, between calls — do NOT hold the slot: a background job that
    /// is merely waiting out its LOADING/PREEMPTION budget cannot block foreground chat. If the lane
    /// leaked past `run_background`, the foreground call below would deadlock and the timeout would fire.
    #[tokio::test]
    async fn a_returned_background_call_frees_the_slot_for_foreground() {
        let slot = LocalSlot::default();
        let out = slot.run_background(async { 7u8 }).await;
        assert!(
            matches!(out, SlotOutcome::Ran(7)),
            "the background call ran"
        );
        // The lane is free now — foreground must acquire it without blocking.
        let fg = tokio::time::timeout(Duration::from_secs(5), slot.run_foreground(async { 9u8 }))
            .await
            .expect("foreground must not block once the background call has returned");
        assert_eq!(fg, 9);
    }

    /// Foreground chat PREEMPTS an in-flight background call. While a background call is actively parked
    /// inside the slot (holding the lane, awaiting its request), a foreground call signals it, takes the
    /// lane, and runs — the background call returns [`SlotOutcome::Preempted`], its future dropped. This
    /// is the "chat always wins" invariant for the case the lane IS held (an active request), the
    /// complement of the freed-lane test above. Together they cover both ways chat wins: it preempts an
    /// active request, and it walks straight into a free lane while a background job is between retries.
    #[tokio::test]
    async fn foreground_preempts_an_in_flight_background_call() {
        use std::sync::Arc;
        let slot = Arc::new(LocalSlot::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let bg = {
            let slot = slot.clone();
            tokio::spawn(async move {
                slot.run_background(async move {
                    // Signal that we are now running inside the slot (lane held), then never finish on
                    // our own — only a preemption can end this call.
                    let _ = started_tx.send(());
                    std::future::pending::<u8>().await
                })
                .await
            })
        };
        started_rx
            .await
            .expect("the background call started inside the slot");

        let fg = tokio::time::timeout(Duration::from_secs(5), slot.run_foreground(async { 42u8 }))
            .await
            .expect("foreground must preempt the in-flight background call, not block behind it");
        assert_eq!(fg, 42);
        assert!(
            matches!(bg.await.unwrap(), SlotOutcome::Preempted),
            "the in-flight background call was preempted, its future dropped"
        );
    }
}
