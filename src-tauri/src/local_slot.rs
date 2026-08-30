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
//! (a dropped reqwest future) can only be exercised against a running server, which neither these
//! tests nor CI do. The epic's live-rig checklist owns that check and still owes it.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

    /// Silence allowed between progress ticks during an Ollama model pull. A pull streams frequent
    /// byte/manifest updates, so a long quiet gap means a wedged download (or a dropped connection the
    /// stream didn't surface as an error). Generous — a slow disk write, or a "verifying sha256" pause
    /// on a multi-GB model, can be quiet for a while — but bounded so the tab never hangs on a dead pull.
    pub const PULL_STALL_TIMEOUT: Duration = Duration::from_secs(120);

    /// Total wall-clock budget for a NON-streaming background completion (summaries, titles, prefs).
    /// With no token-level signal to lean on, this single deadline must cover a cold load plus the
    /// whole (short) generation. 180s = ~60s worst-case load + generation headroom. A dead host is
    /// still caught fast by the connect timeout; this only bounds an accepted-then-wedged request.
    pub const BACKGROUND_TOTAL_TIMEOUT: Duration = Duration::from_secs(180);

    /// Per-request deadline for the `/v1/models` reachability probe — a wrong URL must fail fast.
    pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

    /// Per-request deadline for the `/slots` and `/v1/models` context-window probes.
    pub const WINDOW_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

    /// The quiet allowance while Ollama VERIFIES a finished download ("verifying sha256 digest" /
    /// "writing manifest"). Hashing is one silent status line per layer and then nothing until it
    /// is done — a 44 GiB catalogue row on a spinning disk is several minutes of silence — and the
    /// general stall timeout below called that a failed download on a pull that was essentially
    /// complete. Only these named phases get the long leash; a silent DOWNLOAD is still a stall.
    pub const PULL_VERIFY_STALL_TIMEOUT: Duration = Duration::from_secs(900);

    /// How old a cached context window may grow before the next opportunity re-probes it. The cache
    /// used to be write-once per process, which made it blind in both directions: a user who
    /// followed PM's own refusal message — raise `OLLAMA_CONTEXT_LENGTH`, restart the server — kept
    /// being refused against the dead server's window until they also restarted PM, and a server
    /// restarted SMALLER left an oversized ceiling silently head-cutting every prompt. Sixty
    /// seconds bounds both wrongs at one minute; the probe itself is two loopback GETs against a
    /// server that just answered, so the steady-state cost is negligible.
    pub const WINDOW_REPROBE_INTERVAL: Duration = Duration::from_secs(60);

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
            // `UnrecognisedResponse` sits here for the same reason as `ClientError`: the host
            // ANSWERED. A 200 settles liveness more firmly than a 404 does, and striking it would
            // eject a working server for what is a config or compatibility problem — hiding the
            // body PM could not read behind a cooldown instead of surfacing it.
            // `PromptTooLarge` never reached the wire at all, so it says nothing about the host —
            // it is `Neutral` rather than `Alive` for exactly that reason: an unsent request must
            // neither strike a healthy server nor clear the strikes of a failing one.
            LocalFailKind::PromptTooLarge => CallOutcome::Neutral,
            LocalFailKind::ModelLoading
            | LocalFailKind::ClientError(_)
            | LocalFailKind::UnrecognisedResponse => CallOutcome::Alive,
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
    /// Calls occupying OR waiting for the lane. Counted from function entry rather than from lane
    /// acquisition on purpose: a call queued behind another is still a reason to keep the model
    /// resident, and releasing the memory it is about to need would be the worst possible timing.
    in_flight: AtomicUsize,
    /// When the last call finished. `None` until one has. Stamped on the way OUT of a call, so a
    /// quiet period measures from the end of the last work rather than the start of it.
    last_active: Mutex<Option<Instant>>,
    /// Background jobs PM has spawned that have not reached the lane yet — see [`ReleaseHold`].
    /// An `Arc` so a hold can outlive the borrow that minted it and travel into a spawned task.
    holds: Arc<AtomicUsize>,
}

/// Resets `chat_waiting` to `false` on drop — including on an unwinding panic — so a chat call that
/// panics mid-flight can never wedge the slot into "chat forever waiting".
struct ChatWaitingGuard<'a>(&'a AtomicBool);
impl Drop for ChatWaitingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Counts one call as occupying the slot, and stamps the finish time when it lets go.
///
/// A guard rather than a matched increment/decrement pair, for exactly the reason `ChatWaitingGuard`
/// above is one — and here the need is sharper. `run_background` has THREE exits: the early
/// `chat_waiting` bail before the `select!`, the preemption arm inside it, and normal completion.
/// A decrement written after the select would be skipped by the first two, and a leaked count means
/// PM decides it is permanently busy and silently never releases the card again — a bug whose only
/// symptom is the absence of a thing happening.
struct SlotBusyGuard<'a>(&'a LocalSlot);
impl Drop for SlotBusyGuard<'_> {
    fn drop(&mut self) {
        // Stamp BEFORE decrementing, so a reader that observes zero in-flight also observes a fresh
        // finish time rather than the previous call's.
        if let Ok(mut t) = self.0.last_active.lock() {
            *t = Some(Instant::now());
        }
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A claim on the model held by work PM has SPAWNED but which has not reached the slot yet.
///
/// The gap this closes is real and narrow: `send_message` spawns its follow-up jobs after the reply
/// has returned, so between the two the slot is genuinely quiet — and a release taken in that window
/// makes every one of those jobs cold-load. This is not a forecast that could be wrong; PM created
/// the work it is holding the model for. Released on drop, so a job that panics or returns early
/// cannot leave the hold behind.
pub struct ReleaseHold(Arc<AtomicUsize>);

impl Drop for ReleaseHold {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let _busy = SlotBusyGuard(self);
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
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let _busy = SlotBusyGuard(self);
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

    /// Run housekeeping that must not overlap an inference call, and must not be interrupted once
    /// begun.
    ///
    /// Neither `run_foreground` nor `run_background` is right for an unload. Background is
    /// preemptible, and a preemption *after* the request has gone out drops the settle wait, letting
    /// chat start streaming against a runner that is mid-teardown — measured at ~850 ms, during which
    /// a request re-attaches to the dying runner and gets a truncated or blank answer. Foreground
    /// would work but signals `chat_waiting`, which makes any queued background job bail and retry
    /// for no reason.
    ///
    /// So this simply takes the lane and holds it. The cost is that a chat arriving during a release
    /// waits for it — bounded by the settle timeout, and only reachable in the seconds after a quiet
    /// period expires. The alternative is a torn stream scored as a strike, and three of those cool
    /// the endpoint down for chat as well, for up to five minutes. A short wait is the better trade
    /// and it is the reason this is not preemptible.
    pub async fn run_exclusive<F>(&self, fut: F) -> F::Output
    where
        F: std::future::Future,
    {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let _busy = SlotBusyGuard(self);
        let _lane = self.lane.lock().await;
        fut.await
    }

    /// Claim the model for work PM has spawned but which has not reached the lane yet. Released on
    /// drop, so a job that panics or returns early cannot strand the claim.
    pub fn hold(&self) -> ReleaseHold {
        self.holds.fetch_add(1, Ordering::SeqCst);
        ReleaseHold(self.holds.clone())
    }

    /// Calls occupying or waiting for the lane right now.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Background jobs spawned but not yet at the lane.
    pub fn holds(&self) -> usize {
        self.holds.load(Ordering::SeqCst)
    }

    /// How long since the last call finished. `None` when none ever has.
    ///
    /// Deliberately says nothing about whether the slot is busy right now — `in_flight` and `holds`
    /// are separate inputs to [`crate::residency::should_release`], and folding them in here would
    /// make that reducer's own gates unreachable, leaving the release decision spread across two
    /// places with the pure, tested one contributing nothing.
    pub fn quiet_for(&self, now: Instant) -> Option<Duration> {
        let last = (*self.last_active.lock().ok()?)?;
        Some(now.saturating_duration_since(last))
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
    windows: Mutex<std::collections::HashMap<String, (WindowInfo, Instant)>>,
    /// The (endpoint, model) pairs PM has itself caused to be loaded.
    ///
    /// The scope rule for releasing: **PM only ever unloads a model PM loaded.** A model someone
    /// started from a terminal is theirs, and a settings pane that quietly frees it is a scheduler
    /// acting on something nobody consented to.
    ///
    /// In-memory, and rebuilt on every launch — which is correct rather than a limitation. The
    /// server keeps its models loaded across PM restarts, so on launch PM has caused zero loads and
    /// everything resident belongs to the user until PM loads it itself. Residency seen through
    /// `/api/ps` is deliberately NOT evidence here: that route does not care who loaded a model,
    /// which is exactly what makes it useless as a proof of ownership.
    ///
    /// A set rather than one entry per role, because both roles can name the same model — chat
    /// letting go must not free what background is about to use.
    ///
    /// Stored as a `(base_url, model)` TUPLE rather than a joined key. The obvious
    /// `format!("{base}::{model}")` is unrecoverable for an IPv6 endpoint, which PM explicitly
    /// supports (`classify_ip` has loopback, ULA and link-local arms): splitting
    /// `http://[::1]:11434::gemma3:4b` on its first `::` lands inside the address literal and yields
    /// a base URL of `http://[`. Every release path would then silently do nothing, and only for
    /// IPv6 — invisible to any test that does not go through the accessor.
    pm_loaded: Mutex<std::collections::HashSet<(String, String)>>,
    /// Endpoints that answered an unload with "no such route".
    ///
    /// llama-server and LM Studio have no unload gesture at all, and neither does a proxy that
    /// forwards only `/v1`. Without a latch the scheduler asks such a server every twenty seconds
    /// for the life of the process — over four thousand pointless requests a day at a server that
    /// can never satisfy one. A permanent property of the endpoint, so learning it once is enough.
    no_unload_route: Mutex<std::collections::HashSet<String>>,
    /// The release policy as last read from settings, with its quiet period.
    ///
    /// Cached because a model can be resident while the vault is LOCKED — PM loaded it, the user
    /// locked up and walked away, and the card stays occupied. Releasing has to keep working there,
    /// but the policy lives in an encrypted settings row that cannot be read with the vault shut. So
    /// PM remembers the last policy it could read and keeps honouring it. Without this the feature
    /// would quietly stop at exactly the moment someone leaves the machine — which is the moment it
    /// is most obviously supposed to work.
    release_policy: Mutex<Option<(crate::residency::ReleasePolicy, Duration)>>,
    last_probe: Mutex<Option<Instant>>,
    /// The RESULT of the last reachability observation, so a debounced status read can report what
    /// was actually last seen. `None` means nothing has been observed yet, which is NOT the same as
    /// reachable — the status chip must never claim health it has not witnessed.
    last_reachable: Mutex<Option<bool>>,
    /// The last hardware scan (#296), cached until a `force` re-scan. Reset on restart with the rest
    /// of this runtime — hardware rarely changes mid-session, and a stale figure is one click away.
    hardware: Mutex<Option<crate::hardware::Hardware>>,
    /// The last on-disk model crawl (#449), cached on the same terms as the hardware scan: it walks
    /// real directories, so it is not something to redo on every Workbench repaint.
    disk_models: Mutex<Option<crate::local_disk::DiskScan>>,
    /// The one in-flight (or just-finished) model pull. Backend-owned so the job survives the
    /// settings view unmounting (the tab router unmounts on every switch) and so a remounted view
    /// can re-read where things stand instead of re-arming the Download button mid-download.
    pull: Mutex<Option<PullState>>,
}

/// What a UI needs to render the one model pull: live progress while `running`, and the terminal
/// outcome after (an `error`, the `"cancelled"` status, or a completed pull). Kept until the next
/// pull replaces it, so a view that was unmounted when the download failed still gets to say so.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PullSnapshot {
    /// The tag being pulled (`hf.co/<repo>:<QUANT>`).
    pub model: String,
    /// Ollama's latest status line ("pulling manifest", "downloading", "verifying sha256", …);
    /// `"cancelled"` after a cancel.
    pub status: String,
    /// Bytes fetched / total for the layer currently downloading, when Ollama reports them.
    pub completed_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// Still streaming. `false` is terminal: consult `error` and `status`.
    pub running: bool,
    pub error: Option<String>,
}

struct PullState {
    snap: PullSnapshot,
    cancel: Arc<Notify>,
}

impl LocalRuntime {
    /// Record a call's outcome against host health (poison-tolerant — health is advisory).
    ///
    /// Real traffic is better evidence of reachability than a 30 s probe, so an outcome that settles
    /// the question also updates the last-known figure. `Neutral` (a chat preemption — the GPU was
    /// briefly busy, not the host's fault) says nothing either way and leaves it alone.
    pub fn record(&self, outcome: CallOutcome) {
        if let Ok(mut h) = self.health.lock() {
            h.observe(outcome, Instant::now());
        }
        match outcome {
            // `Alive` counts as reached: the host ANSWERED — a 503 while a model warms up or a 4xx
            // about a model id is a reply, which is exactly what reachability means here.
            CallOutcome::Ok | CallOutcome::Alive => self.set_last_reachable(true),
            CallOutcome::Strike => self.set_last_reachable(false),
            CallOutcome::Neutral => {}
        }
    }

    /// The last observed reachability, or `None` if nothing has been observed yet (poison-tolerant).
    pub fn last_reachable(&self) -> Option<bool> {
        self.last_reachable.lock().ok().and_then(|r| *r)
    }

    /// Remember what a probe or a real call just observed (poison-tolerant — advisory, like health).
    pub fn set_last_reachable(&self, reachable: bool) {
        if let Ok(mut r) = self.last_reachable.lock() {
            *r = Some(reachable);
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

    /// Remember the release policy, so it survives the vault being locked.
    pub fn cache_release_policy(&self, policy: crate::residency::ReleasePolicy, idle: Duration) {
        if let Ok(mut p) = self.release_policy.lock() {
            *p = Some((policy, idle));
        }
    }

    /// The last policy PM was able to read. `None` before the first successful read — and `None` is
    /// NOT "release nothing by default": it means PM has never known the policy, so it must not act.
    pub fn cached_release_policy(&self) -> Option<(crate::residency::ReleasePolicy, Duration)> {
        *self.release_policy.lock().ok()?
    }

    /// Record that PM is about to put this model on the wire.
    ///
    /// Called BEFORE the request, deliberately. A cold load that then times out has still left the
    /// model resident on the server, and a marker written only on success would leave PM unable to
    /// free the very memory its own failed call reserved. The same reasoning covers a preempted
    /// call, whose load may still complete server-side after PM dropped the request.
    pub fn mark_pm_loaded(&self, base_url: &str, model: &str) {
        if let Ok(mut set) = self.pm_loaded.lock() {
            set.insert((base_url.to_string(), model.to_string()));
        }
    }

    /// Whether PM caused this model to be loaded, and may therefore release it.
    pub fn is_pm_loaded(&self, base_url: &str, model: &str) -> bool {
        self.pm_loaded
            .lock()
            .map(|s| s.contains(&(base_url.to_string(), model.to_string())))
            .unwrap_or(false)
    }

    /// Forget a model PM no longer owns — released by PM, or observed to have gone away by itself.
    ///
    /// The second case is what makes the ownership rule true rather than merely stated. A marker is
    /// written when PM puts a model on the wire, but the model can leave without PM: Ollama evicts
    /// under memory pressure, and a user can `ollama stop` it. If the marker outlived that, then the
    /// next time the USER loaded that same model themselves PM would still believe it owned it — and
    /// would unload their model out from under their terminal. Which is exactly the
    /// acting-without-consent the rule exists to prevent, and the one failure here a user could not
    /// recover from by noticing.
    pub fn clear_pm_loaded(&self, base_url: &str, model: &str) {
        if let Ok(mut set) = self.pm_loaded.lock() {
            set.remove(&(base_url.to_string(), model.to_string()));
        }
    }

    /// Remember that this endpoint cannot unload, so PM stops asking.
    pub fn mark_no_unload_route(&self, base_url: &str) {
        if let Ok(mut set) = self.no_unload_route.lock() {
            set.insert(base_url.to_string());
        }
    }

    /// Whether PM has learned that this endpoint has no unload route.
    pub fn has_no_unload_route(&self, base_url: &str) -> bool {
        self.no_unload_route
            .lock()
            .map(|s| s.contains(base_url))
            .unwrap_or(false)
    }

    /// Every (endpoint, model) PM currently believes it loaded, as `(base_url, model)` pairs.
    pub fn pm_loaded_pairs(&self) -> Vec<(String, String)> {
        self.pm_loaded
            .lock()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn cached_window(&self, base_url: &str, model: &str) -> Option<WindowInfo> {
        let key = Self::window_key(base_url, model);
        self.windows.lock().ok()?.get(&key).map(|(info, _)| *info)
    }

    /// Whether the cached window for this key is due a re-probe: absent, or older than
    /// [`tunables::WINDOW_REPROBE_INTERVAL`]. The cache used to be write-once per process, which
    /// left it blind to the one change its own refusal message tells the user to make (restart the
    /// server with a bigger window) — and to the quieter inverse, a server restarted smaller.
    pub fn window_probe_due(&self, base_url: &str, model: &str) -> bool {
        let key = Self::window_key(base_url, model);
        match self.windows.lock() {
            Ok(w) => w
                .get(&key)
                .is_none_or(|(_, at)| at.elapsed() >= tunables::WINDOW_REPROBE_INTERVAL),
            Err(_) => false,
        }
    }

    pub fn cache_window(&self, base_url: &str, model: &str, info: WindowInfo) {
        if let Ok(mut w) = self.windows.lock() {
            w.insert(Self::window_key(base_url, model), (info, Instant::now()));
        }
    }

    // ---- the one-pull-at-a-time registry (see `PullSnapshot`) ----

    /// Claim the pull slot for `model`. `None` when a pull is already running — the caller refuses
    /// rather than racing two `/api/pull`s at the same server. On success, returns the handle a
    /// `cancel_pull` will notify (`notify_one`, so a cancel that lands before the puller awaits is
    /// stored, not lost).
    pub fn begin_pull(&self, model: &str) -> Option<Arc<Notify>> {
        let mut p = self.pull.lock().ok()?;
        if p.as_ref().is_some_and(|s| s.snap.running) {
            return None;
        }
        let cancel = Arc::new(Notify::new());
        *p = Some(PullState {
            snap: PullSnapshot {
                model: model.to_string(),
                status: "starting".to_string(),
                completed_bytes: None,
                total_bytes: None,
                running: true,
                error: None,
            },
            cancel: cancel.clone(),
        });
        Some(cancel)
    }

    /// Record one progress tick against the running pull (poison-tolerant, like the caches).
    pub fn update_pull(&self, progress: &crate::openai_compat::PullProgress) {
        if let Ok(mut p) = self.pull.lock() {
            if let Some(s) = p.as_mut().filter(|s| s.snap.running) {
                s.snap.status = progress.status.clone();
                s.snap.completed_bytes = progress.completed_bytes;
                s.snap.total_bytes = progress.total_bytes;
            }
        }
    }

    /// Mark the running pull terminal. `error: None` with the status left as streamed = success;
    /// the snapshot survives until the next `begin_pull` so an unmounted UI can still report it.
    pub fn finish_pull(&self, error: Option<String>) {
        if let Ok(mut p) = self.pull.lock() {
            if let Some(s) = p.as_mut() {
                s.snap.running = false;
                s.snap.error = error;
            }
        }
    }

    /// Mark the running pull cancelled: terminal, deliberate, not an error.
    pub fn finish_pull_cancelled(&self) {
        if let Ok(mut p) = self.pull.lock() {
            if let Some(s) = p.as_mut() {
                s.snap.running = false;
                s.snap.status = "cancelled".to_string();
                s.snap.error = None;
            }
        }
    }

    /// The current pull snapshot (running or last-terminal), for a UI (re)mounting.
    pub fn active_pull(&self) -> Option<PullSnapshot> {
        self.pull.lock().ok()?.as_ref().map(|s| s.snap.clone())
    }

    /// Ask the running pull to stop. Returns whether there was one to ask.
    pub fn cancel_pull(&self) -> bool {
        if let Ok(p) = self.pull.lock() {
            if let Some(s) = p.as_ref().filter(|s| s.snap.running) {
                s.cancel.notify_one();
                return true;
            }
        }
        false
    }

    /// Test-only: age a cached window entry so staleness paths can be exercised without sleeping.
    #[cfg(test)]
    pub fn backdate_window(&self, base_url: &str, model: &str, age: Duration) {
        if let Ok(mut w) = self.windows.lock() {
            if let Some((_, at)) = w.get_mut(&Self::window_key(base_url, model)) {
                *at = Instant::now() - age;
            }
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

    /// The last cached hardware scan, if any (poison-tolerant).
    pub fn cached_hardware(&self) -> Option<crate::hardware::Hardware> {
        self.hardware.lock().ok()?.clone()
    }

    /// Store a fresh hardware scan (poison-tolerant).
    pub fn cache_hardware(&self, hw: crate::hardware::Hardware) {
        if let Ok(mut h) = self.hardware.lock() {
            *h = Some(hw);
        }
    }

    /// The last cached on-disk model crawl, if any (poison-tolerant).
    pub fn cached_disk_models(&self) -> Option<crate::local_disk::DiskScan> {
        self.disk_models.lock().ok()?.clone()
    }

    /// Store a fresh on-disk model crawl (poison-tolerant).
    pub fn cache_disk_models(&self, scan: crate::local_disk::DiskScan) {
        if let Ok(mut d) = self.disk_models.lock() {
            *d = Some(scan);
        }
    }

    /// Drop the cached crawl so the next read re-walks — after a re-scan, or a change to which
    /// folders are crawled.
    pub fn clear_disk_models(&self) {
        if let Ok(mut d) = self.disk_models.lock() {
            *d = None;
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
        // A prompt PM refused to send says nothing about the host: it must neither strike a healthy
        // server (a background sizing problem would otherwise put CHAT into cooldown — HealthState
        // is shared across roles) nor clear the strikes of a failing one.
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::PromptTooLarge),
            CallOutcome::Neutral
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
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::UnrecognisedResponse),
            CallOutcome::Alive,
            "a 200 PM could not read settles liveness more firmly than a 404 does — striking it \
             would eject a working server and hide the body behind a cooldown"
        );
        // The stream kinds keep their strike: those really are the host failing mid-flight.
        assert_eq!(
            CallOutcome::for_failure(&LocalFailKind::MalformedStream),
            CallOutcome::Strike
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

    /// `Ok` is the strongest signal in the enum — it clears the strike streak, lifts an active
    /// cooldown AND resets the ejection count that drives the escalation. A host that answers but
    /// never finishes must not get that, or a server truncating every reply looks perfectly healthy
    /// however many times it has already been ejected. `Alive` is the honest reading: it responded.
    #[test]
    fn alive_says_the_host_responded_without_wiping_its_record() {
        let mut h = HealthState::default();
        let t0 = Instant::now();

        // Three strikes: ejected, and the escalation counter is now 1.
        for _ in 0..tunables::COOLDOWN_FAILURE_THRESHOLD {
            h.observe(CallOutcome::Strike, t0);
        }
        assert!(h.in_cooldown(t0), "three strikes ⇒ resting");

        // An answered-but-truncated call clears the streak and does NOT lift the cooldown early.
        h.observe(CallOutcome::Alive, t0);
        assert!(
            h.in_cooldown(t0),
            "a host that answered without finishing has not earned its way out"
        );

        // A genuinely clean reply does lift it, and resets the escalation.
        h.observe(CallOutcome::Ok, t0);
        assert!(!h.in_cooldown(t0));
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

    #[test]
    fn a_cached_window_ages_out_instead_of_living_forever() {
        use crate::openai_compat::WindowSource;
        // Write-once was blind in both directions: the user follows PM's own refusal message
        // (raise the context length, restart the server) and PM keeps refusing against the dead
        // server's window; or the server comes back SMALLER and a stale proven ceiling lets
        // oversized prompts through to be silently head-cut.
        let rt = LocalRuntime::default();
        let (url, model) = ("http://127.0.0.1:11434", "llama3.2");
        assert!(
            rt.window_probe_due(url, model),
            "no entry at all is exactly what a probe fixes"
        );
        rt.cache_window(
            url,
            model,
            WindowInfo {
                tokens: 4096,
                source: WindowSource::LoadedModel,
            },
        );
        assert!(
            !rt.window_probe_due(url, model),
            "a fresh entry is not re-probed — the hot path must stay probe-free"
        );
        rt.backdate_window(url, model, tunables::WINDOW_REPROBE_INTERVAL);
        assert!(
            rt.window_probe_due(url, model),
            "an entry older than the interval is due — however proven it was when written"
        );
        // The entry itself is still served while the background re-probe runs: stale evidence is
        // better than none for SIZING, and the refresh is what bounds how stale it can get.
        assert_eq!(rt.cached_window(url, model).map(|w| w.tokens), Some(4096));
    }

    #[test]
    fn reachability_is_remembered_and_starts_unknown_rather_than_healthy() {
        let rt = LocalRuntime::default();
        // Nothing observed yet. The status command reads this as unreachable on purpose: the chip
        // must never claim health it has not witnessed. It used to answer `!in_cooldown` instead,
        // which is green for a server that has never once been reached.
        assert_eq!(rt.last_reachable(), None);

        // Real traffic settles the question as well as a probe does.
        rt.record(CallOutcome::Ok);
        assert_eq!(rt.last_reachable(), Some(true));

        // One strike is enough to stop claiming reachable — well before any cooldown opens, which is
        // exactly the window where a failed chat call used to leave the chip green.
        rt.record(CallOutcome::Strike);
        assert_eq!(rt.last_reachable(), Some(false));
        assert!(
            !rt.health().in_cooldown(Instant::now()),
            "no cooldown after one strike"
        );

        // A 503-while-loading or a 4xx means the host ANSWERED — that is what reachable means here.
        rt.record(CallOutcome::Alive);
        assert_eq!(rt.last_reachable(), Some(true));

        // A chat preemption is not the host's fault and says nothing either way.
        rt.record(CallOutcome::Neutral);
        assert_eq!(rt.last_reachable(), Some(true));
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
        // THE test for the release feature. `run_background` has three exits and the preemption arm
        // returns from inside a `select!`, dropping the call future — so a decrement written after
        // the select would never run for this path. A leaked count means PM believes it is
        // permanently busy and silently never releases the graphics card again, which is a bug whose
        // only symptom is the absence of something happening.
        assert_eq!(
            slot.in_flight(),
            0,
            "a preempted background call must still let go of the slot"
        );
        assert!(slot.quiet_for(Instant::now()).is_some());
    }

    #[tokio::test]
    async fn every_way_out_of_the_slot_lets_go_of_it() {
        use std::sync::Arc;
        let slot = Arc::new(LocalSlot::default());
        assert_eq!(slot.in_flight(), 0);
        // Nothing has run, so there is no quiet period running — which is NOT a quiet period of
        // zero, and collapsing the two would release the card before PM had ever used it.
        assert_eq!(slot.quiet_for(Instant::now()), None);

        slot.run_foreground(async { 1u8 }).await;
        assert_eq!(slot.in_flight(), 0, "after a completed foreground call");

        assert!(matches!(
            slot.run_background(async { 2u8 }).await,
            SlotOutcome::Ran(2)
        ));
        assert_eq!(slot.in_flight(), 0, "after a completed background call");

        // The early bail, before the `select!` is ever reached: chat is already waiting.
        slot.chat_waiting.store(true, Ordering::SeqCst);
        assert!(matches!(
            slot.run_background(async { 3u8 }).await,
            SlotOutcome::Preempted
        ));
        slot.chat_waiting.store(false, Ordering::SeqCst);
        assert_eq!(slot.in_flight(), 0, "after the early chat-waiting bail");

        // And a panic inside the call, which unwinds past every decrement that is not a guard.
        let panicking = {
            let slot = slot.clone();
            tokio::spawn(async move {
                slot.run_foreground(async { panic!("inside the slot") })
                    .await
            })
        };
        assert!(panicking.await.is_err(), "the panic propagated");
        assert_eq!(slot.in_flight(), 0, "after a panic inside the call");
    }

    #[test]
    fn model_ownership_survives_an_ipv6_endpoint() {
        // The obvious `format!("{base}::{model}")` key is unrecoverable here: splitting
        // `http://[::1]:11434::gemma3:4b` on its FIRST `::` lands inside the address literal and
        // yields a base URL of `http://[`. Every release path would then post to an unresolvable
        // host and silently do nothing — for IPv6 endpoints only, which PM explicitly supports
        // (`classify_ip` has loopback, ULA and link-local arms). Invisible to any test that does not
        // go through the pairs accessor, which is why this one does.
        let rt = LocalRuntime::default();
        let v6 = "http://[::1]:11434";
        rt.mark_pm_loaded(v6, "gemma3:4b");
        assert!(rt.is_pm_loaded(v6, "gemma3:4b"));
        assert_eq!(
            rt.pm_loaded_pairs(),
            vec![(v6.to_string(), "gemma3:4b".to_string())],
            "the endpoint must round-trip intact, colons and all"
        );

        rt.clear_pm_loaded(v6, "gemma3:4b");
        assert!(rt.pm_loaded_pairs().is_empty());
    }

    #[test]
    fn an_endpoint_that_cannot_unload_is_only_asked_once() {
        // llama-server and LM Studio have no unload route, and neither does a proxy forwarding only
        // `/v1`. Without this latch the release scheduler posts at one every 20 s for the life of
        // the process — over four thousand requests a day at a server that can never satisfy one.
        let rt = LocalRuntime::default();
        assert!(!rt.has_no_unload_route("http://127.0.0.1:8080"));
        rt.mark_no_unload_route("http://127.0.0.1:8080");
        assert!(rt.has_no_unload_route("http://127.0.0.1:8080"));
        // Learned per endpoint, not globally — pointing PM at an Ollama afterwards must still work.
        assert!(!rt.has_no_unload_route("http://127.0.0.1:11434"));
    }

    #[test]
    fn ownership_is_never_inherited_across_a_restart() {
        // `LocalRuntime` is rebuilt on every launch while the server keeps its models loaded across
        // PM restarts. So on launch PM has caused zero loads and everything resident belongs to the
        // user until PM loads it itself. A fresh runtime claiming nothing is what makes that true.
        let rt = LocalRuntime::default();
        assert!(rt.pm_loaded_pairs().is_empty());
        assert!(!rt.is_pm_loaded("http://127.0.0.1:11434", "gemma3:4b"));
    }

    #[tokio::test]
    async fn a_hold_keeps_the_model_even_while_the_slot_is_empty() {
        use std::sync::Arc;
        let slot = Arc::new(LocalSlot::default());
        slot.run_foreground(async { 1u8 }).await;
        assert!(
            slot.quiet_for(Instant::now()).is_some(),
            "quiet after a call"
        );

        // `send_message` spawns its follow-up jobs AFTER the reply returns, so the slot is genuinely
        // empty in between. Releasing in that window makes all of them cold-load.
        let hold = slot.hold();
        assert_eq!(
            slot.holds(),
            1,
            "the hold is visible to the release decision"
        );
        drop(hold);
        assert_eq!(slot.holds(), 0, "and released when the job ends");
    }
}
