// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The provider dispatch seam: the ONE place every chat/completion routes through, so a role's
//! provider choice (cloud vs a local endpoint) is decided once and can gain new inputs — the
//! power-aware policy (#432) — without touching a single call site.
//!
//! This PR is a **behaviour-frozen refactor**. With no local endpoint configured (the only possible
//! state until #297's live provider lands) every role resolves to the OpenRouter cloud arm with
//! EXACTLY the key + ordered model list it used before, and [`complete`]/[`stream_chat`] call the
//! unchanged `openrouter` functions with identical arguments. The seam is a dispatch enum behind the
//! same two verbs, not a rewrite of the call sites' semantics — proven mechanically by
//! `resolve_provider_with_an_empty_context_is_a_direct_preference_lookup` plus an untouched
//! `openrouter.rs`.

use std::time::Instant;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{Error, Result};
use crate::local_slot::{
    loading_retry_backoff, preemption_retry_delay, tunables, CallOutcome, SlotOutcome,
};
use crate::openai_compat::{self, LocalFailKind, LocalFailure};
use crate::openrouter::{self, ChatMessage, Completion};
use crate::secret::Secret;
use crate::settings::{
    effective_models, BACKGROUND_AUTO_SWITCH_KEY, BACKGROUND_MODELS_KEY, CHAT_AUTO_SWITCH_KEY,
    CHAT_MODELS_KEY,
};
use crate::{db, secrets, AppState};

/// Which of PM's two AI roles a request belongs to. Chat is the interactive, user-facing model;
/// Background is every unattended `complete()` consumer (summaries, titles, briefing, proposals).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Chat,
    Background,
}

/// A role's provider preference. `Cloud` is the default for any role whose routing setting is unset
/// — which is EVERY role until #297's live provider introduces the local settings — so a
/// config-less install routes exactly as it does today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderPref {
    Cloud,
    Local,
    LocalThenCloud,
}

/// The per-role routing preferences, read from settings.
#[derive(Clone, Copy, Debug)]
pub struct RoutingPrefs {
    pub chat: ProviderPref,
    pub background: ProviderPref,
}

impl RoutingPrefs {
    pub fn for_role(&self, role: Role) -> ProviderPref {
        match role {
            Role::Chat => self.chat,
            Role::Background => self.background,
        }
    }

    #[cfg(test)]
    fn uniform(pref: ProviderPref) -> Self {
        Self {
            chat: pref,
            background: pref,
        }
    }
}

/// Runtime signals that can influence routing at dispatch time. **Deliberately inert today** — it
/// holds no fields. It is threaded through EVERY dispatch path (built inside [`resolve`], never by a
/// caller) so the power-aware provider policy (#432) can add battery / AC state here and change
/// routing in ONE place, without touching a single call site. Do NOT delete it or "simplify" it
/// away because it is currently empty: the emptiness IS the banked seam, and constructing it inside
/// `resolve` is precisely what keeps a future field from rippling out to the 13 dispatch sites.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeContext {}

impl RuntimeContext {
    /// Read the current runtime signals. Empty today; #432 populates it (battery / AC / power-saver
    /// state) at the I/O edge, consistent with the fit-math vs hardware-scan split.
    fn current() -> Self {
        Self {}
    }
}

/// The effective provider routing for a request, after [`resolve_provider`] applies runtime policy
/// to the raw preference. Today it mirrors the preference 1:1; #432 is where an empty-context
/// identity becomes a real policy (e.g. force `Cloud` on battery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderChoice {
    Cloud,
    Local,
    LocalThenCloud,
}

/// Decide a role's effective provider from its preference and the current runtime context. **Pure**
/// — no I/O — so it is exhaustively unit-tested and the byte-identical invariant is mechanical, not
/// argued. `runtime` is intentionally unread TODAY: the power-aware policy (#432) will read
/// battery / AC state from it HERE to override the preference, which is why it is a parameter of
/// this one function rather than something the call sites compute. Keeping it in the signature is
/// what makes that feature a change to this function alone. Do not remove it.
pub fn resolve_provider(
    role: Role,
    prefs: &RoutingPrefs,
    runtime: &RuntimeContext,
) -> ProviderChoice {
    let _ = runtime; // reserved for #432 — see the doc comment; not a dead parameter.
    match prefs.for_role(role) {
        ProviderPref::Cloud => ProviderChoice::Cloud,
        ProviderPref::Local => ProviderChoice::Local,
        ProviderPref::LocalThenCloud => ProviderChoice::LocalThenCloud,
    }
}

/// Which provider actually served a completion — recorded on every usage row so cost/latency
/// accounting can tell local from cloud, and so the chat honesty surface (#297 PR6) can render it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Cloud,
    Local,
}

impl Provider {
    /// The stable token stored in `usage_log.provider`.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Cloud => "cloud",
            Provider::Local => "local",
        }
    }
}

/// Why a request was served by cloud instead of the local endpoint the user preferred. A power-
/// policy switch is kept a categorically distinct variant so it can NEVER be represented as — or
/// collapsed into — a failure (Bobby, item 4). PR3 produces only the failure-family reasons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FallbackReason {
    /// The local leg was attempted and failed for a concrete wire reason; we fell back to cloud.
    HardFailure(LocalFailKind),
    /// The local host is inside its dead-host cooldown after repeated failures, so the request was
    /// routed to cloud WITHOUT attempting local this turn. Failure-derived, but not a fresh failure.
    Cooldown,
    /// The configured endpoint's address now resolves to a public host over cleartext, so the
    /// call-time posture gate refused it and the request went to cloud instead. NOT a failure and
    /// NOT failure-derived: the local host may be perfectly healthy — what changed is where its
    /// name points. Kept distinct from [`FallbackReason::HardFailure`] precisely so it can never be
    /// read as evidence the host is dead.
    EndpointRefused,
    /// BANKED for the power-aware provider policy (#432): a DELIBERATE user policy (local on AC /
    /// cloud on battery), categorically NOT a failure. No producer exists in PR3 (the feature is a
    /// deferred board card) — the variant is banked NOW so a later implementer physically cannot fold
    /// a policy switch into a hard-failure value. Do not remove it or "clean up" the unused variant.
    #[allow(dead_code)]
    PowerPolicy,
}

impl FallbackReason {
    /// A stable snake_case token for `usage_log.fallback_reason` and the honesty surface. The
    /// failure family is prefixed `hard_failure:`; a power-policy switch is never in that family.
    pub fn as_log_str(&self) -> String {
        match self {
            FallbackReason::HardFailure(kind) => format!("hard_failure:{}", fail_kind_slug(kind)),
            FallbackReason::Cooldown => "cooldown".to_string(),
            FallbackReason::EndpointRefused => "endpoint_refused".to_string(),
            FallbackReason::PowerPolicy => "power_policy".to_string(),
        }
    }
}

/// A stable slug for a wire failure kind — the failure-family detail in `as_log_str`.
fn fail_kind_slug(kind: &LocalFailKind) -> &'static str {
    match kind {
        LocalFailKind::Refused => "refused",
        LocalFailKind::Timeout => "timeout",
        LocalFailKind::MalformedStream => "malformed_stream",
        LocalFailKind::UnrecognisedResponse => "unrecognised_response",
        LocalFailKind::DegenerateStream => "degenerate_stream",
        LocalFailKind::ModelLoading => "model_loading",
        LocalFailKind::ServerError(_) => "server_error",
        LocalFailKind::ClientError(_) => "client_error",
        LocalFailKind::ReplyTooLarge => "reply_too_large",
    }
}

/// Normalized metadata about how a completion was actually served — parallel to the raw
/// [`Completion`] the model returned. Threaded to the usage logger (provider/latency/fallback
/// columns, #297 PR3 migration v37) and to the chat honesty surface (#297 PR6).
#[derive(Clone, Debug)]
pub struct CallMeta {
    pub provider: Provider,
    /// Wall-clock latency of the leg that actually served the reply, in milliseconds.
    pub latency_ms: u64,
    /// Set when the request was NOT served by the user's preferred local endpoint — why, and which
    /// local model was displaced. `None` on a plain success (cloud-preferred, or local succeeded).
    pub fallback: Option<FallbackReason>,
    pub displaced_local_model: Option<String>,
}

impl CallMeta {
    fn cloud(latency: std::time::Duration) -> Self {
        Self {
            provider: Provider::Cloud,
            latency_ms: latency.as_millis() as u64,
            fallback: None,
            displaced_local_model: None,
        }
    }

    fn local(latency: std::time::Duration) -> Self {
        Self {
            provider: Provider::Local,
            latency_ms: latency.as_millis() as u64,
            fallback: None,
            displaced_local_model: None,
        }
    }

    fn cloud_fallback(
        latency: std::time::Duration,
        reason: FallbackReason,
        displaced_local_model: String,
    ) -> Self {
        Self {
            provider: Provider::Cloud,
            latency_ms: latency.as_millis() as u64,
            fallback: Some(reason),
            displaced_local_model: Some(displaced_local_model),
        }
    }
}

/// A [`Completion`] plus the normalized [`CallMeta`] about how it was served. The gateway verbs
/// return this; a call site binds `let LlmOutcome { completion, meta } = …` so its existing use of
/// the completion fields is unchanged and only the accounting reads `meta`.
pub struct LlmOutcome {
    pub completion: Completion,
    pub meta: CallMeta,
}

/// Nudge any listening UI (the Local AI tab, the chat sidebar's provider line) to refetch
/// `local_llm_status` because the local endpoint's health may have just changed — recovered to
/// reachable, or dropped into a dead-host cooldown. A payload-less ping: the frontend reads the
/// fresh snapshot itself, and `local_llm_status` debounces its actual probe
/// ([`local_slot::tunables::HEALTH_PROBE_DEBOUNCE`]) so a burst of pings can't hammer the user's
/// server. Fired only where health can transition (an Ok resets strikes; a failure may open a
/// cooldown); cooldown EXPIRY is time-based, so the frontend also refetches once at the deadline.
/// `pub(crate)` so the endpoint set/clear commands ([`local_ai`]) fire the same event through one
/// owner of the name string.
pub(crate) fn ping_status(app: &AppHandle) {
    let _ = app.emit("local-llm://status", ());
}

/// The cloud arm's hydrated inputs: the API key and the ordered model list (auto-switch fallback).
pub struct CloudArm {
    pub key: Secret,
    pub models: Vec<String>,
}

/// The local arm's hydrated inputs: the normalized base URL, the single model id chosen for the
/// role, and an optional bearer token (kept in the keychain, never handed to the webview).
pub struct LocalArm {
    pub base_url: String,
    pub model: String,
    pub token: Option<Secret>,
}

/// The resolved route for a request — which provider to use, hydrated with what it needs. Cloud is
/// the only reachable arm on a config-less install; the local arms are built once #297 PR3's Local
/// AI settings are configured. `LocalThenCloud` carries BOTH arms so the executor can fall back
/// without re-resolving.
pub enum RoutePlan {
    Cloud(CloudArm),
    LocalOnly(LocalArm),
    LocalThenCloud { local: LocalArm, cloud: CloudArm },
}

impl RoutePlan {
    /// The primary model id this route will try FIRST — used to attribute logged spend when the
    /// server did not report which model actually served the request. For the local arms this is the
    /// local model; a fallback records the actually-served (cloud) model via [`Completion::model`].
    pub fn primary_model_id(&self) -> &str {
        match self {
            RoutePlan::Cloud(arm) => arm.models.first().map(String::as_str).unwrap_or_default(),
            RoutePlan::LocalOnly(local) | RoutePlan::LocalThenCloud { local, .. } => &local.model,
        }
    }

    /// The ordered model list this route will use — for the cost logger, which prices per model. A
    /// local arm has exactly one model, borrowed as a one-element slice.
    pub fn models(&self) -> &[String] {
        match self {
            RoutePlan::Cloud(arm) => &arm.models,
            RoutePlan::LocalOnly(local) | RoutePlan::LocalThenCloud { local, .. } => {
                std::slice::from_ref(&local.model)
            }
        }
    }
}

/// Settings keys for the per-role routing preference. Absent → `Cloud`, which is what makes the seam
/// strictly additive: a config-less install never touches a local code path.
pub(crate) const CHAT_ROUTING_KEY: &str = "local_llm_chat_routing";
pub(crate) const BACKGROUND_ROUTING_KEY: &str = "local_llm_background_routing";

/// Settings keys for the local endpoint. The base URL is shared across roles (one server); the model
/// is chosen per role. The bearer token lives in the keychain, never in settings.
pub(crate) const LOCAL_BASE_URL_KEY: &str = "local_llm_base_url";
pub(crate) const LOCAL_CHAT_MODEL_KEY: &str = "local_llm_chat_model";
pub(crate) const LOCAL_BACKGROUND_MODEL_KEY: &str = "local_llm_background_model";

/// Parse a stored routing preference. Absent (every install today), `"cloud"`, or an unrecognised
/// value all resolve to `Cloud` — the strictly-additive default.
fn parse_pref(raw: Option<String>) -> ProviderPref {
    match raw.as_deref() {
        Some("local") => ProviderPref::Local,
        Some("local-then-cloud") => ProviderPref::LocalThenCloud,
        _ => ProviderPref::Cloud,
    }
}

fn routing_prefs(conn: &rusqlite::Connection) -> Result<RoutingPrefs> {
    Ok(RoutingPrefs {
        chat: parse_pref(crate::db::get_setting(conn, CHAT_ROUTING_KEY)?),
        background: parse_pref(crate::db::get_setting(conn, BACKGROUND_ROUTING_KEY)?),
    })
}

/// Resolve the route for a role: decide the provider (pure), then hydrate the chosen arm(s). Returns
/// `None` when no provider is usable (no key AND no local endpoint), so each caller keeps its own
/// no-provider behaviour (a background job skips; an interactive command returns
/// [`no_provider_message`]). Takes only `role`; the [`RuntimeContext`] is built HERE, never by the
/// caller, so adding a future input (#432's power state) never re-plumbs a single dispatch site.
///
/// Every DB read below takes the lock briefly and drops it — the mutex is non-reentrant, so nothing
/// holds a lock across `resolve`'s return or across the caller's later work.
pub fn resolve(app: &AppHandle, role: Role) -> Result<Option<RoutePlan>> {
    let state = app.state::<AppState>();

    let choice = {
        let conn = state.conn()?;
        let prefs = routing_prefs(&conn)?;
        resolve_provider(role, &prefs, &RuntimeContext::current())
    };

    let plan = match choice {
        ProviderChoice::Cloud => cloud_arm(app, role)?.map(RoutePlan::Cloud),
        ProviderChoice::Local => local_arm(app, role)?.map(RoutePlan::LocalOnly),
        ProviderChoice::LocalThenCloud => match (local_arm(app, role)?, cloud_arm(app, role)?) {
            // Both configured: the real local-then-cloud route (the executor falls back in-arm).
            (Some(local), Some(cloud)) => Some(RoutePlan::LocalThenCloud { local, cloud }),
            // Local configured, no cloud key: honour the local preference with no fallback available.
            (Some(local), None) => Some(RoutePlan::LocalOnly(local)),
            // Local not configured: fall through to cloud, exactly as before local existed.
            (None, Some(cloud)) => Some(RoutePlan::Cloud(cloud)),
            (None, None) => None,
        },
    };
    Ok(plan)
}

/// Hydrate the cloud arm for a role: the role's key + effective model list, or `None` with no key.
fn cloud_arm(app: &AppHandle, role: Role) -> Result<Option<CloudArm>> {
    // The role's key: chat uses the primary key; background prefers the dedicated background key and
    // falls back to the primary — exactly as the call sites did before this seam.
    let key = match role {
        Role::Chat => secrets::get_openrouter_key()?,
        Role::Background => secrets::get_background_or_primary_key()?,
    };
    let Some(key) = key else {
        return Ok(None);
    };
    let (models_key, auto_key) = match role {
        Role::Chat => (CHAT_MODELS_KEY, CHAT_AUTO_SWITCH_KEY),
        Role::Background => (BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY),
    };
    let state = app.state::<AppState>();
    let models = {
        let conn = state.conn()?;
        effective_models(&conn, models_key, auto_key)?
    };
    Ok(Some(CloudArm { key, models }))
}

/// Hydrate the local arm for a role: the shared base URL + the role's model + the optional bearer
/// token, or `None` when the endpoint isn't fully configured (no URL, or no model for the role).
fn local_arm(app: &AppHandle, role: Role) -> Result<Option<LocalArm>> {
    let state = app.state::<AppState>();
    let (base_url, model) = {
        let conn = state.conn()?;
        let base_url = db::get_setting(&conn, LOCAL_BASE_URL_KEY)?;
        let model_key = match role {
            Role::Chat => LOCAL_CHAT_MODEL_KEY,
            Role::Background => LOCAL_BACKGROUND_MODEL_KEY,
        };
        let model = db::get_setting(&conn, model_key)?;
        (base_url, model)
    };
    let (Some(base_url), Some(model)) = (base_url, model) else {
        return Ok(None);
    };
    if base_url.trim().is_empty() || model.trim().is_empty() {
        return Ok(None);
    }
    let token = secrets::get_local_llm_endpoint_token()?;
    Ok(Some(LocalArm {
        base_url,
        model,
        token,
    }))
}

/// Whether a local endpoint is usable as a CHAT provider from settings alone — a base URL and a chat
/// model, both present and non-blank. This is the settings half of [`local_arm`] for [`Role::Chat`]
/// minus the keychain token read, factored out as a pure `&Connection` predicate so the keyless-
/// onboarding gate (#295) can ask "is this user AI-ready via a local model?" and so it is unit-
/// testable. The routing preference is deliberately ignored: leniency errs toward the honest
/// `no_provider_message()` guard rather than re-showing a local-only user the onboarding wizard.
pub(crate) fn local_chat_configured(conn: &Connection) -> Result<bool> {
    let base_url = db::get_setting(conn, LOCAL_BASE_URL_KEY)?;
    let model = db::get_setting(conn, LOCAL_CHAT_MODEL_KEY)?;
    Ok(match (base_url, model) {
        (Some(b), Some(m)) => !b.trim().is_empty() && !m.trim().is_empty(),
        _ => false,
    })
}

/// Run a non-streaming completion through the resolved route, returning the completion plus how it
/// was served. Background consumers (summaries, titles, proposals) call this.
pub async fn complete(
    app: &AppHandle,
    plan: &RoutePlan,
    messages: &[ChatMessage],
    cache_prefix: bool,
) -> Result<LlmOutcome> {
    match plan {
        RoutePlan::Cloud(arm) => {
            let start = Instant::now();
            let completion =
                openrouter::complete(arm.key.expose(), &arm.models, messages, cache_prefix).await?;
            Ok(LlmOutcome {
                completion,
                meta: CallMeta::cloud(start.elapsed()),
            })
        }
        RoutePlan::LocalOnly(local) => run_local_complete(app, local, messages, None).await,
        RoutePlan::LocalThenCloud { local, cloud } => {
            run_local_complete(app, local, messages, Some(cloud)).await
        }
    }
}

/// The local arm of [`complete`]: consume the single-inference slot (preemptible by chat), classify
/// the outcome for the circuit breaker, and — for `LocalThenCloud` — fall back to cloud on any hard
/// failure. Background consumption is atomic (nothing is shown mid-stream), so a cloud retry after a
/// failed local leg is always safe.
///
/// "Background waits and retries" (#297): rather than deferring a whole idle-gated scheduler cycle,
/// this retries IN-PROCESS — bounded by a TOTAL-ELAPSED budget, not an attempt count — for the two
/// transient cases only: a chat PREEMPTION (the GPU was briefly busy; not a fault) and a host that
/// ANSWERED "model loading" (alive, warming up). The two get different budgets: a warming model is
/// worth waiting out for the whole cold-load window ([`tunables::LOADING_RETRY_BUDGET`]), a busy GPU
/// is not ([`tunables::PREEMPTION_RETRY_BUDGET`] is short — defer to the idle scheduler, the right
/// backstop for "chat is using the GPU"). Every OTHER failure is a strike: NOT retried here (that
/// would be the hot loop against a reloading server the research warns against) — it falls back to
/// cloud, or surfaces. Past the budget the job returns to its scheduler for a next-tick retry with its
/// cursor unadvanced, so a user mid-conversation never traps it spinning.
async fn run_local_complete(
    app: &AppHandle,
    local: &LocalArm,
    messages: &[ChatMessage],
    cloud: Option<&CloudArm>,
) -> Result<LlmOutcome> {
    let state = app.state::<AppState>();
    let rt = &state.local_ai;

    // Cooldown gate: skip the local attempt entirely while the host rests after repeated failures.
    if !rt.available() {
        return match cloud {
            Some(cloud) => {
                cloud_complete(
                    cloud,
                    messages,
                    FallbackReason::Cooldown,
                    local.model.clone(),
                )
                .await
            }
            None => Err(Error::Other(cooldown_message(rt))),
        };
    }

    // Call-time posture gate: the stored base URL is as often a hostname as an IP, and its address
    // is only ever classified when it is SAVED. Asked ONCE here, deliberately OUTSIDE the retry
    // loop below — a warming host can iterate a dozen-plus times and must not re-resolve each pass.
    // A refusal is never recorded against the circuit breaker (see `endpoint_refused_now`).
    if crate::local_ai::endpoint_refused_now(&local.base_url).await {
        return match cloud {
            Some(cloud) => {
                cloud_complete(
                    cloud,
                    messages,
                    FallbackReason::EndpointRefused,
                    local.model.clone(),
                )
                .await
            }
            None => Err(Error::Other(crate::local_ai::CALL_TIME_REFUSAL.into())),
        };
    }

    let loop_start = Instant::now();
    let mut loading_recheck: u32 = 0;
    loop {
        let start = Instant::now();
        let token = local.token.as_ref().map(Secret::expose);
        let attempt = openai_compat::complete(&local.base_url, &local.model, token, messages);
        match rt.slot.run_background(attempt).await {
            SlotOutcome::Ran(Ok(completion)) => {
                rt.record(CallOutcome::Ok);
                ping_status(app);
                ensure_local_window_cached(app, local);
                return Ok(LlmOutcome {
                    completion,
                    meta: CallMeta::local(start.elapsed()),
                });
            }
            SlotOutcome::Preempted => {
                // A chat turn took the single GPU slot — not the host's fault (never a strike). Wait a
                // short jittered beat and retry: the retry mostly BLOCKS on the slot's lane until chat
                // yields (it does not hit the server), so it is cheap. Bounded by a short total-elapsed
                // budget — a persistently-busy GPU hands back to the idle scheduler rather than spinning.
                rt.record(CallOutcome::Neutral);
                if loop_start.elapsed() < tunables::PREEMPTION_RETRY_BUDGET {
                    tokio::time::sleep(preemption_retry_delay()).await;
                    continue;
                }
                return Err(Error::Other(
                    "the local model was busy with a chat request; it will retry shortly".into(),
                ));
            }
            SlotOutcome::Ran(Err(failure)) => {
                rt.record(CallOutcome::for_failure(&failure.kind));
                ping_status(app);
                // A host that ANSWERED "model loading" is alive and warming up — recheck with a capped
                // full-jitter backoff across the whole cold-load window, so the model completes locally
                // (honouring a local-then-cloud user's local preference, sparing needless cloud spend)
                // instead of being abandoned after a few seconds. Every other failure is a strike and is
                // NOT retried here — no hot loop against a reloading server.
                if matches!(failure.kind, LocalFailKind::ModelLoading)
                    && loop_start.elapsed() < tunables::LOADING_RETRY_BUDGET
                {
                    loading_recheck += 1;
                    tokio::time::sleep(loading_retry_backoff(loading_recheck)).await;
                    continue;
                }
                return match cloud {
                    Some(cloud) => {
                        cloud_complete(
                            cloud,
                            messages,
                            FallbackReason::HardFailure(failure.kind),
                            local.model.clone(),
                        )
                        .await
                    }
                    None => Err(local_failure_to_error(&failure)),
                };
            }
        }
    }
}

/// A cloud completion tagged as a FALLBACK (records the reason + the local model it displaced).
async fn cloud_complete(
    cloud: &CloudArm,
    messages: &[ChatMessage],
    reason: FallbackReason,
    displaced: String,
) -> Result<LlmOutcome> {
    let start = Instant::now();
    let completion =
        openrouter::complete(cloud.key.expose(), &cloud.models, messages, false).await?;
    Ok(LlmOutcome {
        completion,
        meta: CallMeta::cloud_fallback(start.elapsed(), reason, displaced),
    })
}

/// Stream a chat completion through the resolved route, forwarding each token to `on_token`, and
/// return the completion plus how it was served. The interactive chat path.
pub async fn stream_chat<F>(
    app: &AppHandle,
    plan: &RoutePlan,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
) -> Result<LlmOutcome>
where
    F: FnMut(&str),
{
    match plan {
        RoutePlan::Cloud(arm) => {
            let start = Instant::now();
            let completion = openrouter::stream_chat(
                arm.key.expose(),
                &arm.models,
                messages,
                cache_through,
                on_token,
            )
            .await?;
            Ok(LlmOutcome {
                completion,
                meta: CallMeta::cloud(start.elapsed()),
            })
        }
        RoutePlan::LocalOnly(local) => {
            run_local_stream(app, local, messages, cache_through, None, on_token).await
        }
        RoutePlan::LocalThenCloud { local, cloud } => {
            run_local_stream(app, local, messages, cache_through, Some(cloud), on_token).await
        }
    }
}

/// The local arm of [`stream_chat`]: chat is FOREGROUND, so it preempts any in-flight background
/// local call and is never itself preempted. Falls back to cloud ONLY before the first token — once
/// content has streamed, a mid-stream failure is surfaced as an error, never silently reissued.
async fn run_local_stream<F>(
    app: &AppHandle,
    local: &LocalArm,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    cloud: Option<&CloudArm>,
    mut on_token: F,
) -> Result<LlmOutcome>
where
    F: FnMut(&str),
{
    let state = app.state::<AppState>();
    let rt = &state.local_ai;

    if !rt.available() {
        return match cloud {
            Some(cloud) => {
                cloud_stream(
                    cloud,
                    messages,
                    cache_through,
                    on_token,
                    FallbackReason::Cooldown,
                    local.model.clone(),
                )
                .await
            }
            None => Err(Error::Other(cooldown_message(rt))),
        };
    }

    // The same call-time posture gate as `run_local_complete`, before a single chat token can leave
    // the machine. Nothing has been streamed yet, so falling back to cloud here is always clean.
    if crate::local_ai::endpoint_refused_now(&local.base_url).await {
        return match cloud {
            Some(cloud) => {
                cloud_stream(
                    cloud,
                    messages,
                    cache_through,
                    on_token,
                    FallbackReason::EndpointRefused,
                    local.model.clone(),
                )
                .await
            }
            None => Err(Error::Other(crate::local_ai::CALL_TIME_REFUSAL.into())),
        };
    }

    let start = Instant::now();
    let mut first = false;
    let token = local.token.as_ref().map(Secret::expose);
    let local_result = {
        let first = &mut first;
        let on_token = &mut on_token;
        let attempt = openai_compat::stream_chat(
            &local.base_url,
            &local.model,
            token,
            messages,
            |t: &str| {
                *first = true;
                on_token(t);
            },
        );
        rt.slot.run_foreground(attempt).await
    };
    match local_result {
        Ok(completion) => {
            rt.record(CallOutcome::Ok);
            ping_status(app);
            ensure_local_window_cached(app, local);
            Ok(LlmOutcome {
                completion,
                meta: CallMeta::local(start.elapsed()),
            })
        }
        Err(failure) => {
            rt.record(CallOutcome::for_failure(&failure.kind));
            ping_status(app);
            match cloud {
                // Nothing shown yet — a clean fallback to cloud is safe.
                Some(cloud) if !first => {
                    cloud_stream(
                        cloud,
                        messages,
                        cache_through,
                        on_token,
                        FallbackReason::HardFailure(failure.kind),
                        local.model.clone(),
                    )
                    .await
                }
                // Local-only, or the local leg already streamed content: surface the failure.
                _ => Err(local_failure_to_error(&failure)),
            }
        }
    }
}

/// A cloud stream tagged as a FALLBACK (records the reason + the local model it displaced).
async fn cloud_stream<F>(
    cloud: &CloudArm,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
    reason: FallbackReason,
    displaced: String,
) -> Result<LlmOutcome>
where
    F: FnMut(&str),
{
    let start = Instant::now();
    let completion = openrouter::stream_chat(
        cloud.key.expose(),
        &cloud.models,
        messages,
        cache_through,
        on_token,
    )
    .await?;
    Ok(LlmOutcome {
        completion,
        meta: CallMeta::cloud_fallback(start.elapsed(), reason, displaced),
    })
}

/// The message shown when NO provider is configured — provider-aware (a deliberate copy change from
/// the old "No OpenRouter API key set", #297 PR3, changelog-noted). Neutral between the cloud and
/// local paths so the keyless-onboarding direction (#295 PR7) reads right.
pub fn no_provider_message() -> String {
    "No AI provider is set up yet — add an OpenRouter key, or set up a local model in Settings → AI."
        .to_string()
}

/// After a successful local call the model is loaded, so `/slots` (llama-server) reports the real
/// context window — probe and cache it ONCE, in the background, so the context meter can show it on
/// its next poll without ever blocking a reply on the network. A no-op once the window is cached.
fn ensure_local_window_cached(app: &AppHandle, local: &LocalArm) {
    let state = app.state::<AppState>();
    if state
        .local_ai
        .cached_window(&local.base_url, &local.model)
        .is_some()
    {
        return;
    }
    // The catalog rung is a cheap in-memory lookup — resolve it here (sync) and hand it to the probe.
    let catalog = crate::local_catalog::context_window_for(&local.model);
    let app = app.clone();
    let base_url = local.base_url.clone();
    let model = local.model.clone();
    let token = local.token.as_ref().map(|s| s.expose().to_string());
    tauri::async_runtime::spawn(async move {
        // This task fires its own token-bearing request OUTSIDE both gateway gates, so it carries
        // the call-time posture check itself. Silent on refusal: the context meter simply keeps its
        // catalog/default rung, which is exactly what an unreachable `/slots` already does.
        if crate::local_ai::endpoint_refused_now(&base_url).await {
            return;
        }
        let info = openai_compat::probe_window(&base_url, &model, token.as_deref(), catalog).await;
        app.state::<AppState>()
            .local_ai
            .cache_window(&base_url, &model, info);
    });
}

/// The message when a local-only endpoint (no cloud fallback) is inside its dead-host cooldown.
fn cooldown_message(rt: &crate::local_slot::LocalRuntime) -> String {
    let secs = rt.health().cooldown_remaining(Instant::now()).as_secs();
    if secs > 0 {
        format!(
            "the local model endpoint is resting after repeated failures — it will retry automatically in about {secs}s"
        )
    } else {
        "the local model endpoint is temporarily unavailable — it will retry automatically"
            .to_string()
    }
}

/// Convert a wire failure into a friendly user-facing error for the local-only path (no fallback).
fn local_failure_to_error(failure: &LocalFailure) -> Error {
    let base = match failure.kind {
        LocalFailKind::Refused => {
            "couldn't reach the local model endpoint — is the server running?"
        }
        LocalFailKind::Timeout => "the local model didn't respond in time",
        LocalFailKind::ModelLoading => "the local model is still loading — try again in a moment",
        LocalFailKind::ClientError(_) => {
            "the local endpoint rejected the request — check the model id in Settings → Local AI"
        }
        LocalFailKind::ServerError(_) => "the local model server returned an error",
        LocalFailKind::MalformedStream => "the local model stream ended unexpectedly",
        LocalFailKind::UnrecognisedResponse => {
            "the local endpoint answered, but not in a shape PM recognises"
        }
        LocalFailKind::DegenerateStream => {
            "the local model got stuck repeating itself and was stopped"
        }
        LocalFailKind::ReplyTooLarge => "the local model reply was too large",
    };
    // For a server that ANSWERED (a bad request / a 5xx), surface its own words — it's the user's own
    // local server, so echoing its message is safe and the fastest way to diagnose a bad model id.
    let detail = failure.detail.trim();
    let msg = match failure.kind {
        // The body is the whole diagnosis for an unrecognised answer, so it rides along like a
        // server's own error text does — it is the user's own machine either way.
        LocalFailKind::ClientError(_)
        | LocalFailKind::ServerError(_)
        | LocalFailKind::UnrecognisedResponse
            if !detail.is_empty() =>
        {
            format!("{base} ({})", crate::error::truncate_detail(detail))
        }
        _ => base.to_string(),
    };
    Error::Other(msg)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unreadable_answer_is_not_reported_as_a_broken_stream() {
        // `probe()` and the non-streaming completion path both used `MalformedStream`, so a user
        // whose endpoint answered 200 with an unexpected body was told "the local model stream
        // ended unexpectedly" about a call with no stream in it — and the one diagnosable part,
        // the body, was dropped on the floor.
        let err = local_failure_to_error(&LocalFailure {
            kind: LocalFailKind::UnrecognisedResponse,
            detail: "the endpoint answered but did not look like an OpenAI /v1/models list".into(),
        });
        let msg = err.to_string();
        assert!(
            msg.contains("not in a shape PM recognises"),
            "should not claim a stream broke: {msg}"
        );
        assert!(!msg.contains("stream ended"), "{msg}");
        assert!(
            msg.contains("/v1/models list"),
            "the body is the whole diagnosis and must survive: {msg}"
        );
    }

    use super::*;

    /// The byte-identical invariant made mechanical: with an EMPTY runtime context, the resolver's
    /// output must equal the raw preference for every (role, preference) pair — there is no policy
    /// override yet, so routing is exactly a direct preference lookup. The same reasoning as making
    /// the IPC boundary enforced rather than merely documented (#432 items 1-3).
    #[test]
    fn resolve_provider_with_an_empty_context_is_a_direct_preference_lookup() {
        let ctx = RuntimeContext::default();
        for role in [Role::Chat, Role::Background] {
            for (pref, expected) in [
                (ProviderPref::Cloud, ProviderChoice::Cloud),
                (ProviderPref::Local, ProviderChoice::Local),
                (ProviderPref::LocalThenCloud, ProviderChoice::LocalThenCloud),
            ] {
                let prefs = RoutingPrefs::uniform(pref);
                assert_eq!(
                    resolve_provider(role, &prefs, &ctx),
                    expected,
                    "role {role:?} pref {pref:?} must route to {expected:?} with an empty context"
                );
            }
        }
    }

    /// A call-time endpoint refusal must not feed the dead-host circuit breaker. The host may be
    /// perfectly healthy — what changed is where its NAME points — so striking it would hand a
    /// rebound endpoint an escalating cooldown on top of the refusal, and would keep punishing it
    /// after DNS settled back. Both local arms therefore return from the refusal branch before any
    /// `rt.record`, and `local_ai::endpoint_refused_now` takes only a `&str` so it physically cannot
    /// reach the breaker.
    ///
    /// The second half is what makes the first non-vacuous: the same runtime, given the nearest
    /// wire failure (`Refused` — an endpoint that did not answer), DOES move. So "health unchanged"
    /// is a fact about the refusal path, not about an inert runtime.
    #[test]
    fn an_endpoint_refusal_records_no_circuit_breaker_strike() {
        let rt = crate::local_slot::LocalRuntime::default();
        let quiet = format!("{:?}", rt.health());
        assert!(rt.available(), "a fresh runtime is available");

        rt.record(CallOutcome::for_failure(&LocalFailKind::Refused));
        assert_ne!(
            format!("{:?}", rt.health()),
            quiet,
            "a wire failure IS a strike — so an unchanged health after a refusal is meaningful"
        );
    }

    /// The refusal reason is its own slug, never folded into the `hard_failure:` family, so nothing
    /// downstream (the usage log, the honesty strip) can read a policy refusal as a dead host.
    #[test]
    fn an_endpoint_refusal_logs_outside_the_hard_failure_family() {
        let slug = FallbackReason::EndpointRefused.as_log_str();
        assert_eq!(slug, "endpoint_refused");
        assert!(!slug.starts_with("hard_failure:"));
        assert_ne!(slug, FallbackReason::Cooldown.as_log_str());
    }

    /// The role dimension is independent of the pref dimension: `for_role` reads the right field.
    #[test]
    fn routing_prefs_reads_the_right_field_per_role() {
        let prefs = RoutingPrefs {
            chat: ProviderPref::Local,
            background: ProviderPref::Cloud,
        };
        assert_eq!(prefs.for_role(Role::Chat), ProviderPref::Local);
        assert_eq!(prefs.for_role(Role::Background), ProviderPref::Cloud);
    }

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn local_chat_configured_requires_a_nonblank_base_url_and_chat_model() {
        let conn = mem_conn();
        assert!(!local_chat_configured(&conn).unwrap()); // nothing set
        db::set_setting(&conn, LOCAL_BASE_URL_KEY, "http://localhost:11434").unwrap();
        assert!(!local_chat_configured(&conn).unwrap()); // base URL only
        db::set_setting(&conn, LOCAL_CHAT_MODEL_KEY, "   ").unwrap();
        assert!(!local_chat_configured(&conn).unwrap()); // model present but blank
        db::set_setting(&conn, LOCAL_CHAT_MODEL_KEY, "llama3").unwrap();
        assert!(local_chat_configured(&conn).unwrap()); // both present and non-blank
    }

    #[test]
    fn parse_pref_defaults_absent_and_unknown_to_cloud() {
        assert_eq!(parse_pref(None), ProviderPref::Cloud);
        assert_eq!(parse_pref(Some("cloud".into())), ProviderPref::Cloud);
        assert_eq!(parse_pref(Some("nonsense".into())), ProviderPref::Cloud);
        assert_eq!(parse_pref(Some("local".into())), ProviderPref::Local);
        assert_eq!(
            parse_pref(Some("local-then-cloud".into())),
            ProviderPref::LocalThenCloud
        );
    }

    #[test]
    fn primary_model_id_is_the_first_model() {
        let plan = RoutePlan::Cloud(CloudArm {
            key: Secret::from("k".to_string()),
            models: vec!["a/b".into(), "c/d".into()],
        });
        assert_eq!(plan.primary_model_id(), "a/b");

        let empty = RoutePlan::Cloud(CloudArm {
            key: Secret::from("k".to_string()),
            models: vec![],
        });
        assert_eq!(empty.primary_model_id(), "");
    }
}
