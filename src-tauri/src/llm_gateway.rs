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
//!
//! **The On battery policy (#432)** is the first real input to the seam. [`resolve`] snapshots the
//! settled power latch (in memory, before any DB guard), and only while it says the battery is low
//! does it read the policy's threshold, scope and consent from the open store — fresh, on the same
//! connection as the routing preference. [`resolve_provider`] then turns a "Local, fall back to
//! cloud" role into [`ProviderChoice::CloudForPower`] when every condition holds, and the plan
//! carries no local arm at all: nothing tries local on a request the policy moved, and the spend is
//! logged against the cloud model with `fallback_reason = "power_policy"`, a deliberate policy and
//! never a failure. "Local only" never moves. A request decides its route once, at dispatch; the
//! long multi-batch jobs re-resolve between batches through [`refresh_plan`].

use std::time::Instant;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use crate::context_budget;
use crate::error::{Error, Result};
use crate::local_slot::{
    loading_retry_backoff, preemption_retry_delay, tunables, CallOutcome, SlotOutcome,
};
use crate::openai_compat::{self, LocalFailKind, LocalFailure};
use crate::openrouter::{self, ChatMessage, Completion};
use crate::power::{Consent, PowerScope, PowerSettings, PowerSnapshot, PowerState};
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

/// Runtime signals that influence routing at dispatch time — today, the On battery policy (#432).
/// It is threaded through EVERY dispatch path and built only inside [`resolve`] (and, through
/// [`Self::from_parts`], by `local_llm_status` from the same inputs, so the status can never
/// describe a route resolve would not take). Do NOT delete it or move its construction out to the
/// callers: this type is the ONE place routing reads runtime state, and building it inside
/// `resolve` is what keeps a new field from rippling out to every dispatch site (15 today).
///
/// `Default` is inert — no field moves anything — which is what keeps
/// `resolve_provider_with_an_empty_context_is_a_direct_preference_lookup` a mechanical proof.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeContext {
    /// The settled latch says BatteryLow AND the open store's threshold is the one it was computed
    /// on.
    pub battery_low: bool,
    pub scope: PowerScope,
    /// The roles the user has agreed may go to the cloud on battery (per role: see
    /// [`crate::power::Consent`]).
    pub consent: Consent,
    /// "Keep using local until I quit PM" is on.
    pub keep_local: bool,
}

impl RuntimeContext {
    /// Pure. Inert unless the latch is BatteryLow, the store's threshold is not "never", and the
    /// latch was computed against THIS store's threshold — a vault switch or restore mid-tick reads
    /// as inert until the next tick re-latches against the new store.
    pub(crate) fn from_parts(
        snap: &PowerSnapshot,
        settings: &PowerSettings,
        keep_local: bool,
    ) -> Self {
        let agrees = snap.state == PowerState::BatteryLow
            && settings.threshold != 0
            && Some(settings.threshold) == snap.threshold;
        if !agrees {
            return Self::default();
        }
        Self {
            battery_low: true,
            scope: settings.scope,
            consent: settings.consent,
            keep_local,
        }
    }

    /// The one builder [`resolve`] uses. While the latch is not BatteryLow — which is nearly always
    /// — it returns the inert context WITHOUT reading a single setting. Otherwise it reads the policy
    /// on the caller's guard: never cached, so consent from one store can never route another's.
    fn current(snap: &PowerSnapshot, keep_local: bool, conn: &Connection) -> Self {
        if snap.state != PowerState::BatteryLow {
            return Self::default();
        }
        Self::from_parts(snap, &PowerSettings::read(conn), keep_local)
    }

    /// Whether the policy moves this role to the cloud right now.
    pub fn moves(&self, role: Role) -> bool {
        self.battery_low && self.scope.covers(role) && self.consent.covers(role) && !self.keep_local
    }
}

/// The effective provider routing for a request, after [`resolve_provider`] applies runtime policy
/// to the raw preference. With an inert context it mirrors the preference 1:1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderChoice {
    Cloud,
    Local,
    LocalThenCloud,
    /// A "Local, fall back to cloud" role the On battery policy (#432) has moved to the cloud.
    CloudForPower,
}

/// Decide a role's effective provider from its preference and the current runtime context. **Pure**
/// — no I/O — so it is exhaustively unit-tested and the byte-identical invariant is mechanical, not
/// argued. `runtime` is read HERE and nowhere else, which is what keeps the power policy a change to
/// this one function rather than to the call sites.
pub fn resolve_provider(
    role: Role,
    prefs: &RoutingPrefs,
    runtime: &RuntimeContext,
) -> ProviderChoice {
    match prefs.for_role(role) {
        ProviderPref::Cloud => ProviderChoice::Cloud,
        // "Local only" never moves: the Roles copy promises it uses the model you picked and fails
        // if that is unreachable, and a one-time consent must not quietly cover a role switched to
        // Local only afterwards (#432 decision 7).
        ProviderPref::Local => ProviderChoice::Local,
        ProviderPref::LocalThenCloud if runtime.moves(role) => ProviderChoice::CloudForPower,
        ProviderPref::LocalThenCloud => ProviderChoice::LocalThenCloud,
    }
}

/// Whether a role's cloud key can be read, for the status's "could the policy move this?" answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPresence {
    Present,
    Absent,
    /// The secret store could not be read. Never reported as "no key": it may well be there.
    Unreadable,
}

/// Why the On battery policy can NEVER move a role, as the section words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerBlocked {
    CloudRouting,
    NoLocalModel,
    NoKey,
    KeyUnreadable,
    LocalOnly,
}

/// Why this role can never be moved by the policy, or `None` if it can. The order is the copy's
/// priority: a keyless "Local only" user gets the keyless copy, because adding a key is the step
/// that comes first for them.
pub fn power_blocked(
    pref: ProviderPref,
    local_ready: bool,
    key: KeyPresence,
) -> Option<PowerBlocked> {
    if pref == ProviderPref::Cloud {
        return Some(PowerBlocked::CloudRouting);
    }
    if !local_ready {
        return Some(PowerBlocked::NoLocalModel);
    }
    match key {
        KeyPresence::Absent => return Some(PowerBlocked::NoKey),
        KeyPresence::Unreadable => return Some(PowerBlocked::KeyUnreadable),
        KeyPresence::Present => {}
    }
    if pref == ProviderPref::Local {
        return Some(PowerBlocked::LocalOnly);
    }
    None
}

/// Where the policy has a role right now, for the status. Computed from the same context
/// [`resolve_provider`] reads, so the two cannot disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerRoute {
    /// The policy is not acting on this role.
    Unchanged,
    /// It would move, but the user has not been asked yet — so it stays local.
    NeedsConsent,
    /// It would move, but "Keep using local until I quit PM" is on.
    KeptLocal,
    /// It has moved to the cloud.
    Cloud,
}

pub fn power_route(role: Role, ctx: &RuntimeContext, blocked: Option<PowerBlocked>) -> PowerRoute {
    if blocked.is_some() || !ctx.battery_low || !ctx.scope.covers(role) {
        return PowerRoute::Unchanged;
    }
    if ctx.keep_local {
        return PowerRoute::KeptLocal;
    }
    // Per role: a yes about background work is not a yes about chat.
    if !ctx.consent.covers(role) {
        return PowerRoute::NeedsConsent;
    }
    PowerRoute::Cloud
}

/// The one owner of the per-role key rule, shared by [`cloud_arm`] and the status: chat uses the
/// primary key; background prefers the dedicated background key and falls back to the primary —
/// exactly as the call sites did before this seam.
fn role_key(role: Role) -> Result<Option<Secret>> {
    match role {
        Role::Chat => secrets::get_openrouter_key(),
        Role::Background => secrets::get_background_or_primary_key(),
    }
}

/// Whether this role has a cloud key — an in-memory read of the secrets cache.
pub(crate) fn key_presence(role: Role) -> KeyPresence {
    match role_key(role) {
        Ok(Some(_)) => KeyPresence::Present,
        Ok(None) => KeyPresence::Absent,
        Err(_) => KeyPresence::Unreadable,
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
/// collapsed into — a failure (Bobby, item 4). Every reason but `PowerPolicy` is failure-family.
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
    /// The On battery policy (#432) moved this request to the cloud: a DELIBERATE user policy,
    /// categorically NOT a failure. Its one producer is [`RoutePlan::CloudForPower`]. Kept a variant
    /// of its own so a policy switch can never be folded into a hard-failure value, and filtered out
    /// of the failure strip by [`CallMeta::failure_fallback`].
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
        LocalFailKind::PromptTooLarge => "prompt_too_large",
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

    /// The fallback a UI may present as a failure-family notice. `None` for a power-policy route,
    /// which is a deliberate user policy and must never share the failure strip (#432).
    pub fn failure_fallback(&self) -> Option<&FallbackReason> {
        self.fallback
            .as_ref()
            .filter(|r| !matches!(r, FallbackReason::PowerPolicy))
    }

    /// The On battery policy sent this request to the cloud.
    pub fn power_routed(&self) -> bool {
        matches!(self.fallback, Some(FallbackReason::PowerPolicy))
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
/// The On battery policy (#432) fires it too, but only on a SETTLED change — the watcher's latch
/// moving, a policy write, the "keep using local" override — never once per poll.
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
    LocalThenCloud {
        local: LocalArm,
        cloud: CloudArm,
    },
    /// The On battery policy moved this request to the cloud. Carries NO LocalArm on purpose:
    /// nothing may try local on a request the policy moved (no reverse fallback — "Keep using local"
    /// is the escape, decision 5), and spend must be attributed to the cloud model.
    CloudForPower {
        cloud: CloudArm,
        displaced_local_model: String,
    },
}

impl RoutePlan {
    /// The primary model id this route will try FIRST — used to attribute logged spend when the
    /// server did not report which model actually served the request. For the local arms this is the
    /// local model; a fallback records the actually-served (cloud) model via [`Completion::model`].
    pub fn primary_model_id(&self) -> &str {
        match self {
            RoutePlan::Cloud(arm) | RoutePlan::CloudForPower { cloud: arm, .. } => {
                arm.models.first().map(String::as_str).unwrap_or_default()
            }
            RoutePlan::LocalOnly(local) | RoutePlan::LocalThenCloud { local, .. } => &local.model,
        }
    }

    /// The ordered model list this route will use — for the cost logger, which prices per model. A
    /// local arm has exactly one model, borrowed as a one-element slice.
    pub fn models(&self) -> &[String] {
        match self {
            RoutePlan::Cloud(arm) | RoutePlan::CloudForPower { cloud: arm, .. } => &arm.models,
            RoutePlan::LocalOnly(local) | RoutePlan::LocalThenCloud { local, .. } => {
                std::slice::from_ref(&local.model)
            }
        }
    }

    /// The On battery policy chose this route.
    pub fn is_power_routed(&self) -> bool {
        matches!(self, RoutePlan::CloudForPower { .. })
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

pub(crate) fn routing_prefs(conn: &rusqlite::Connection) -> Result<RoutingPrefs> {
    Ok(RoutingPrefs {
        chat: parse_pref(crate::db::get_setting(conn, CHAT_ROUTING_KEY)?),
        background: parse_pref(crate::db::get_setting(conn, BACKGROUND_ROUTING_KEY)?),
    })
}

/// Resolve the route for a role: decide the provider (pure), then hydrate the chosen arm(s). Returns
/// `None` when no provider is usable (no key AND no local endpoint), so each caller keeps its own
/// no-provider behaviour (a background job skips; an interactive command returns
/// [`no_provider_message`]). Takes only `role`; the [`RuntimeContext`] is built HERE, never by the
/// caller, so a new input never re-plumbs a single dispatch site.
///
/// Every DB read below takes the lock briefly and drops it — the mutex is non-reentrant, so nothing
/// holds a lock across `resolve`'s return or across the caller's later work. The power latch is read
/// BEFORE the DB guard is taken, so no lock is ever acquired while another is held.
pub fn resolve(app: &AppHandle, role: Role) -> Result<Option<RoutePlan>> {
    let state = app.state::<AppState>();
    // In-memory and read BEFORE the DB guard, so no lock is ever taken while another is held.
    let snap = state.local_ai.power_snapshot(Instant::now());
    let keep_local = state.local_ai.keep_local();

    let choice = {
        let conn = state.conn()?;
        let prefs = routing_prefs(&conn)?;
        resolve_provider(
            role,
            &prefs,
            &RuntimeContext::current(&snap, keep_local, &conn),
        )
    };

    let plan = match choice {
        ProviderChoice::Cloud => cloud_arm(app, role)?.map(RoutePlan::Cloud),
        ProviderChoice::Local => local_arm(app, role)?.map(RoutePlan::LocalOnly),
        ProviderChoice::LocalThenCloud | ProviderChoice::CloudForPower => hydrate_local_then_cloud(
            choice == ProviderChoice::CloudForPower,
            local_arm(app, role)?,
            cloud_arm(app, role)?,
        ),
    };
    Ok(plan)
}

/// The "Local, fall back to cloud" ladder, with the power policy's one substitution. Pure. The
/// policy only ever replaces a both-arms pair, so it adds no failure mode and can never route a
/// request to nothing: without a key or without a local model, the plan is exactly the one the
/// preference alone gives.
fn hydrate_local_then_cloud(
    power: bool,
    local: Option<LocalArm>,
    cloud: Option<CloudArm>,
) -> Option<RoutePlan> {
    match (local, cloud) {
        // The On battery policy moved it: the cloud arm alone, naming the local model it displaced.
        (Some(local), Some(cloud)) if power => Some(RoutePlan::CloudForPower {
            displaced_local_model: local.model,
            cloud,
        }),
        // Both configured: the real local-then-cloud route (the executor falls back in-arm).
        (Some(local), Some(cloud)) => Some(RoutePlan::LocalThenCloud { local, cloud }),
        // Local configured, no cloud key: honour the local preference with no fallback available.
        (Some(local), None) => Some(RoutePlan::LocalOnly(local)),
        // Local not configured: fall through to cloud, exactly as before local existed.
        (None, Some(cloud)) => Some(RoutePlan::Cloud(cloud)),
        (None, None) => None,
    }
}

/// Re-resolve between batches of a long background job, so a filing run that started on mains
/// follows the user onto battery (and back) at the next batch boundary rather than at the end of a
/// big import. Keeps the running plan on `Ok(None)` or `Err` (logged), so a started run is never
/// stranded by a vault that locked mid-run.
///
/// Deliberately NOT re-checked: the in-call retry loops in `run_local_complete` (the preemption and
/// model-loading budgets). That request has already started on its arm and may already have caused a
/// load; moving it mid-call would pay for the load and then not use it.
pub fn refresh_plan(app: &AppHandle, role: Role, plan: &mut RoutePlan) {
    match resolve(app, role) {
        Ok(Some(fresh)) => *plan = fresh,
        Ok(None) => {}
        Err(e) => {
            eprintln!(
                "llm_gateway: kept the running route — re-resolving between batches failed ({e})"
            )
        }
    }
}

/// Appended to a failed reply that the On battery policy sent to the cloud. The person most likely
/// to meet it is offline on battery, and the override is the one thing that gets them an answer.
pub(crate) const POWER_ROUTED_FAILED_HINT: &str = "you're on battery, so PM sent this to the cloud. To use your local model instead, turn on \"Keep using local until I quit PM\" in Settings → Local AI → On battery.";

pub(crate) fn power_route_error(e: Error) -> Error {
    Error::Other(format!("{e} — {POWER_ROUTED_FAILED_HINT}"))
}

/// Hydrate the cloud arm for a role: the role's key + effective model list, or `None` with no key.
fn cloud_arm(app: &AppHandle, role: Role) -> Result<Option<CloudArm>> {
    let key = role_key(role)?;
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
        // Straight to OpenRouter with the caller's own `cache_prefix`, NOT through `cloud_complete`,
        // which hard-codes `false`: the review and retag batches ask for the cached prefix and a
        // power route must not quietly cost them it.
        RoutePlan::CloudForPower {
            cloud,
            displaced_local_model,
        } => {
            let start = Instant::now();
            let completion =
                openrouter::complete(cloud.key.expose(), &cloud.models, messages, cache_prefix)
                    .await?;
            Ok(LlmOutcome {
                completion,
                meta: CallMeta::cloud_fallback(
                    start.elapsed(),
                    FallbackReason::PowerPolicy,
                    displaced_local_model.clone(),
                ),
            })
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

    // Fit gate. A background prompt is built from a batch size PM chose, so an overflow here is
    // PM's doing and PM is the one that can avoid it — but only once it knows the window, which is
    // why `run_local_complete` is also where the batchers get their ceiling from.
    if let Some(failure) = prompt_fit_failure(rt, local, messages) {
        rt.record(CallOutcome::for_failure(&failure.kind));
        // A refused call never reaches the wire, so nothing downstream would ever re-check the
        // evidence this refusal stands on — re-probe it here (no-op unless it has aged out). This
        // is what lets "raise the context length and restart the server" actually work without
        // also restarting PM.
        ensure_local_window_cached(app, local);
        return match cloud {
            // Cloud windows are orders of magnitude larger, so the prompt that did not fit locally
            // all but certainly fits there.
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

    let loop_start = Instant::now();
    let mut loading_recheck: u32 = 0;
    loop {
        let start = Instant::now();
        let token = local.token.as_ref().map(Secret::expose);
        // Mark BEFORE the wire, not on success: a cold load that then times out has still left the
        // model resident, and a marker written only on success would leave PM unable to free the
        // memory its own failed call reserved.
        rt.mark_pm_loaded(&local.base_url, &local.model);
        let attempt = openai_compat::complete(&local.base_url, &local.model, token, messages);
        match rt
            .slot
            .run_background(crate::local_slot::Lane::Background, attempt)
            .await
        {
            SlotOutcome::Ran(Ok(completion)) => {
                // A reply nothing can use is not a clean success. `Ok` clears the strike streak,
                // the cooldown AND the ejection count, so a server that truncates every reply (an
                // over-tight `num_predict`, a model that will not stop) — or returns a BLANK one
                // every time (a broken chat template is a real Ollama failure mode) — looked
                // perfectly healthy no matter how many times it had been ejected. The gate is the
                // primitive's own definition (`usable_text`), not just `truncated`, so both kinds
                // of useless 200 score `Alive`: it answered. That clears the streak, so this can
                // never cause a cooldown on its own, and it leaves the escalation record of a host
                // that keeps not delivering.
                rt.record(if completion.usable_text().is_none() {
                    CallOutcome::Alive
                } else {
                    CallOutcome::Ok
                });
                // It answered, so it is on the card — whatever the last `/api/ps` said. Written
                // BEFORE the ping, because the ping is what makes the sidebar re-read: without
                // this the footer could report "not loaded" seconds after the model replied, and
                // stay wrong until the next debounced probe half a minute later.
                rt.cache_resident(&local.base_url, &local.model, true);
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
        RoutePlan::CloudForPower {
            cloud,
            displaced_local_model,
        } => {
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
                meta: CallMeta::cloud_fallback(
                    start.elapsed(),
                    FallbackReason::PowerPolicy,
                    displaced_local_model.clone(),
                ),
            })
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

    // The same fit gate as the background path, and for a sharper reason: chat's system message
    // carries the persona AND the grounding instruction whose SECURITY paragraph is what stops
    // retrieved document text being read as instructions. A front cut removes that paragraph and
    // leaves the untrusted Sources block behind it, which is the one shape this repo does not
    // tolerate (`AGENTS.md`: ingested content is DATA, never instructions). Nothing has streamed
    // yet, so the fallback is clean; local-only surfaces it. The refusal message itself must name
    // Compress: the estimate here is pessimistic on purpose, so a refusal can fire well before the
    // context meter's measured 80% alert would have surfaced the Compress affordance — and once
    // refusals start, the measured numerator stops growing, so that alert may never come.
    if let Some(mut failure) = prompt_fit_failure(rt, local, messages) {
        rt.record(CallOutcome::for_failure(&failure.kind));
        // The evidence just mattered — make sure it is still true (no-op unless it has aged out).
        ensure_local_window_cached(app, local);
        failure
            .detail
            .push_str(" — or use Compress to shrink this conversation");
        return match cloud {
            Some(cloud) => {
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
            None => Err(local_failure_to_error(&failure)),
        };
    }

    let start = Instant::now();
    let mut first = false;
    let token = local.token.as_ref().map(Secret::expose);
    let local_result = {
        let first = &mut first;
        let on_token = &mut on_token;
        // Same rule as the background arm: marked before the request, so a load PM caused but never
        // got an answer from is still PM's to release.
        rt.mark_pm_loaded(&local.base_url, &local.model);
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
            // Same rule as the background arm: answered is not the same as delivered — a
            // truncated OR blank reply keeps the host's escalation record.
            rt.record(if completion.usable_text().is_none() {
                CallOutcome::Alive
            } else {
                CallOutcome::Ok
            });
            // Same rule as the background arm: a reply is proof of residency, and the ping below
            // is what makes the sidebar re-read.
            rt.cache_resident(&local.base_url, &local.model, true);
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

/// The window PM should SIZE a prompt against, and whether that number is proven.
///
/// The cache [`ensure_local_window_cached`] fills is empty until a call has already succeeded — both
/// proven rungs of the ladder (`/slots`, `/api/ps`) only answer while a model is RESIDENT, so there
/// is nothing to ask before the first call. That left the first background call of every process
/// unguarded, which is the worst one to leave unguarded: on a fresh store it is the 400-title
/// vocabulary call.
///
/// So an unknown window is treated as [`openai_compat::DEFAULT_CONTEXT`] — the same honest floor the
/// ladder itself falls to — and flagged as unproven. Never probes; safe on the hot path.
///
/// An UNPROVEN cached number is clamped to that same floor rather than trusted upward. The unproven
/// rung is `ModelsMeta` — llama-server's `n_ctx_train`, LM Studio's `max_context_length` — the
/// model's TRAINED capacity, a property of the weights and not of this load. Sizing batches to it
/// repeats the exact mistake that killed the Catalog rung (#792/#795): llama-server at its default
/// `--ctx-size 4096` reports a 32768 `n_ctx_train`, and one cached probe would have every later
/// batch built eight times too large, silently head-cut on a `--context-shift` server. A trained
/// capacity BELOW the floor is kept — the load cannot exceed the weights, so smaller is evidence.
fn sizing_window(rt: &crate::local_slot::LocalRuntime, local: &LocalArm) -> (i64, bool) {
    let floor = i64::from(openai_compat::DEFAULT_CONTEXT);
    match rt.cached_window(&local.base_url, &local.model) {
        Some(w) if w.source.is_proven() => (i64::from(w.tokens), true),
        Some(w) => (i64::from(w.tokens).min(floor), false),
        None => (floor, false),
    }
}

/// The prompt-token ceiling a BATCHER should size to. Always a number for a local route — sizing
/// against the conservative floor costs one under-filled batch on a big server and then corrects
/// itself, where sizing against nothing costs a silently decapitated prompt.
fn local_prompt_ceiling(rt: &crate::local_slot::LocalRuntime, local: &LocalArm) -> Option<i64> {
    context_budget::prompt_ceiling(Some(sizing_window(rt, local).0))
}

/// The prompt-token ceiling a REFUSAL may be raised against — `None` unless the window is proven.
///
/// The asymmetry is deliberate and it is the whole design. Sizing down on a guess is cheap and
/// self-correcting; refusing on a guess would block a job that would have fitted, and PM's guess is
/// a deliberately pessimistic floor. So PM shrinks on suspicion and refuses only on evidence.
fn local_refusal_ceiling(rt: &crate::local_slot::LocalRuntime, local: &LocalArm) -> Option<i64> {
    let (window, proven) = sizing_window(rt, local);
    proven
        .then(|| context_budget::prompt_ceiling(Some(window)))
        .flatten()
}

/// The prompt-token ceiling a caller should size its BATCH to, for an already-resolved route.
///
/// `None` means "send what you would have sent" — the cloud route, whose windows are orders of
/// magnitude larger than anything built here. A local route always gets a number: an unknown window
/// sizes to the conservative floor rather than to nothing (see [`sizing_window`]).
///
/// `LocalThenCloud` sizes to the LOCAL window even though cloud could take more — the local leg is
/// tried first, and a batch built for a 200k cloud window would be refused on every single call and
/// fall through, which is the opposite of what that preference asks for.
///
/// This is the seam that lets the batchers stop building prompts the gateway would only refuse.
/// [`prompt_fit_failure`] is the backstop for everything they cannot size — an unbounded project
/// list, one enormous document — and for the window changing under a running app.
pub fn prompt_ceiling_for(app: &AppHandle, plan: &RoutePlan) -> Option<i64> {
    let local = sizing_arm(plan)?;
    let state = app.state::<AppState>();
    local_prompt_ceiling(&state.local_ai, local)
}

/// The local arm a batch should be sized against, or `None` for a cloud route — including one the
/// On battery policy chose, which is sized like any other cloud route.
fn sizing_arm(plan: &RoutePlan) -> Option<&LocalArm> {
    match plan {
        RoutePlan::Cloud(_) | RoutePlan::CloudForPower { .. } => None,
        RoutePlan::LocalOnly(local) | RoutePlan::LocalThenCloud { local, .. } => Some(local),
    }
}

/// Refuse a prompt that cannot fit the window the server is actually serving, rather than letting it
/// be silently decapitated.
///
/// Returns the failure to raise, or `None` when the prompt fits — or when the window is not PROVEN,
/// because PM must never refuse a job on the strength of its own conservative guess. The
/// detail names both numbers and the setting that changes them, because that pair IS the fix and it
/// is otherwise invisible: the server accepts the oversized prompt, drops the front of it, and
/// answers 200 with a `prompt_tokens` measured after the cut.
fn prompt_fit_failure(
    rt: &crate::local_slot::LocalRuntime,
    local: &LocalArm,
    messages: &[ChatMessage],
) -> Option<LocalFailure> {
    let ceiling = local_refusal_ceiling(rt, local)?;
    // The window itself, for the message — not reconstructed from the ceiling, whose reserve now
    // scales with the window size.
    let (window, _) = sizing_window(rt, local);
    let est =
        context_budget::est_messages_tokens_upper(messages.iter().map(|m| m.content.as_str()));
    (est > ceiling).then(|| LocalFailure {
        kind: LocalFailKind::PromptTooLarge,
        // "up to": the estimate is deliberately pessimistic (25-100% over truth, by content class),
        // so stating it as the prompt's size would contradict the server's own counters.
        detail: format!(
            "this one needs up to about {est} tokens and the server is serving {window}; \
             raise the context length (Ollama: OLLAMA_CONTEXT_LENGTH, llama-server: --ctx-size, \
             LM Studio: the model's context length)"
        ),
    })
}

/// After a successful local call the model is loaded, so the server can be asked what it really
/// loaded — `/slots` on llama-server, `/api/ps` on Ollama. Probe and cache it in the background, so
/// the context meter can show it on its next poll without ever blocking a reply on the network.
///
/// A no-op while the cached window is younger than `WINDOW_REPROBE_INTERVAL`; older entries are
/// re-probed rather than trusted forever. Write-once was blind in both directions: the user follows
/// PM's own refusal message (raise the context length, restart the server) and PM keeps refusing
/// against the dead server's window; or the server comes back SMALLER and a stale proven ceiling
/// lets oversized prompts through to be silently head-cut. The refusal paths call this too — a
/// refused call is precisely the moment the cached evidence is worth re-checking, and the refusal
/// itself never reaches the wire, so nothing else would. A re-probe that finds no resident model
/// deliberately DOWNGRADES a proven entry to the unproven floor: the evidence expired with the
/// load it described, and an unproven entry sizes conservatively and never refuses.
///
/// Timing matters here: both proven rungs only answer while a model is RESIDENT, which is why this
/// runs after calls rather than at configure time.
fn ensure_local_window_cached(app: &AppHandle, local: &LocalArm) {
    let state = app.state::<AppState>();
    if !state
        .local_ai
        .window_probe_due(&local.base_url, &local.model)
    {
        return;
    }
    let app = app.clone();
    let base_url = local.base_url.clone();
    let model = local.model.clone();
    let token = local.token.as_ref().map(|s| s.expose().to_string());
    tauri::async_runtime::spawn(async move {
        // This task fires its own token-bearing request OUTSIDE both gateway gates, so it carries
        // the call-time posture check itself. Silent on refusal: the meter simply keeps the
        // conservative default, which is exactly what an unreachable `/slots` already does.
        if crate::local_ai::endpoint_refused_now(&base_url).await {
            return;
        }
        let info = openai_compat::probe_window(&base_url, &model, token.as_deref()).await;
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
///
/// Shared with the Local AI tab's test button so a failure gets ONE wording wherever it is met — the
/// same sentence in a settings panel and in a chat error is what lets someone match the two.
pub(crate) fn local_failure_to_error(failure: &LocalFailure) -> Error {
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
        LocalFailKind::PromptTooLarge => {
            "this job needs more context than your local server is serving, so PM didn't send it"
        }
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
        // The two numbers and the setting that changes them ARE the fix here, so the detail is the
        // whole point of the message rather than a diagnostic appended to it.
        | LocalFailKind::PromptTooLarge
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
    use super::*;
    use crate::local_slot::LocalRuntime;
    use crate::openai_compat::{WindowInfo, WindowSource};

    fn arm() -> LocalArm {
        LocalArm {
            base_url: "http://127.0.0.1:11434".into(),
            model: "qwen2.5:7b-instruct-q4_K_M".into(),
            token: None,
        }
    }

    fn msgs(chars: usize) -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                role: "system".into(),
                content: "You file documents. Reply with ONLY JSON.".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "x".repeat(chars),
            },
        ]
    }

    #[test]
    fn an_unknown_window_sizes_batches_but_never_refuses_a_job() {
        // Nothing cached — the state on the FIRST local call of every process, because both proven
        // rungs of the ladder need a resident model. The two ceilings must disagree here: batchers
        // get the conservative floor so they stop building oversized prompts from turn one, and the
        // refusal gate gets nothing, because refusing a job on PM's own guess would block work that
        // would have fitted.
        let rt = LocalRuntime::default();
        let local = arm();
        assert_eq!(
            local_prompt_ceiling(&rt, &local),
            Some(i64::from(openai_compat::DEFAULT_CONTEXT) - context_budget::REPLY_RESERVE_TOKENS),
            "batchers size against the floor when the window is unknown"
        );
        assert_eq!(
            local_refusal_ceiling(&rt, &local),
            None,
            "a guess must never refuse"
        );
        assert!(
            prompt_fit_failure(&rt, &local, &msgs(500_000)).is_none(),
            "not even an absurd prompt is refused on an unproven window"
        );
    }

    #[test]
    fn an_assumed_window_still_never_refuses() {
        // The ladder itself can CACHE the default — `/api/ps` answers "nothing resident" and
        // `pick_window` falls to its floor. That is a cached guess, not a measurement, and it must
        // behave exactly like no cache at all for refusal purposes.
        let rt = LocalRuntime::default();
        let local = arm();
        rt.cache_window(
            &local.base_url,
            &local.model,
            WindowInfo {
                tokens: openai_compat::DEFAULT_CONTEXT,
                source: WindowSource::Default,
            },
        );
        assert!(local_prompt_ceiling(&rt, &local).is_some());
        assert_eq!(local_refusal_ceiling(&rt, &local), None);
        assert!(prompt_fit_failure(&rt, &local, &msgs(500_000)).is_none());
    }

    #[test]
    fn a_proven_window_refuses_an_oversized_prompt_and_says_both_numbers() {
        let rt = LocalRuntime::default();
        let local = arm();
        // What Bobby's server actually reported: 4096, from Ollama's /api/ps.
        rt.cache_window(
            &local.base_url,
            &local.model,
            WindowInfo {
                tokens: 4096,
                source: WindowSource::LoadedModel,
            },
        );

        // A filing batch's ~10k characters of document text — the size `review::BATCH_SIZE` is
        // documented to produce — against a 4096-token server.
        let failure = prompt_fit_failure(&rt, &local, &msgs(10_000))
            .expect("10k characters cannot fit a 4096-token window");
        assert_eq!(failure.kind, LocalFailKind::PromptTooLarge);
        assert!(
            failure.detail.contains("4096"),
            "the served window is half the diagnosis: {}",
            failure.detail
        );
        assert!(
            failure.detail.contains("OLLAMA_CONTEXT_LENGTH"),
            "and the setting that changes it is the other half: {}",
            failure.detail
        );

        // The same prompt is fine on a server serving 32768 — the number the runner guides tell
        // people to set.
        rt.cache_window(
            &local.base_url,
            &local.model,
            WindowInfo {
                tokens: 32_768,
                source: WindowSource::LoadedModel,
            },
        );
        assert!(prompt_fit_failure(&rt, &local, &msgs(10_000)).is_none());
    }

    #[test]
    fn an_unproven_models_meta_window_never_sizes_upward() {
        // `ModelsMeta` is the model's TRAINED capacity (`n_ctx_train` / `max_context_length`) — a
        // property of the weights, not of this load. llama-server at its default `--ctx-size 4096`
        // reports a 32768 n_ctx_train; trusting it for sizing would rebuild the exact #792 defect
        // the Catalog rung was deleted for, one probe after the first successful call.
        let rt = LocalRuntime::default();
        let local = arm();
        rt.cache_window(
            &local.base_url,
            &local.model,
            WindowInfo {
                tokens: 32_768,
                source: WindowSource::ModelsMeta,
            },
        );
        assert_eq!(
            local_prompt_ceiling(&rt, &local),
            Some(i64::from(openai_compat::DEFAULT_CONTEXT) - context_budget::REPLY_RESERVE_TOKENS),
            "an unproven claim above the floor is clamped to the floor"
        );
        assert_eq!(local_refusal_ceiling(&rt, &local), None);
        assert!(prompt_fit_failure(&rt, &local, &msgs(500_000)).is_none());

        // BELOW the floor the claim is kept: the load cannot exceed the weights, so a small trained
        // capacity is real evidence — for sizing, never for refusal.
        rt.cache_window(
            &local.base_url,
            &local.model,
            WindowInfo {
                tokens: 2048,
                source: WindowSource::ModelsMeta,
            },
        );
        assert_eq!(local_prompt_ceiling(&rt, &local), Some(2048 - 512));
        assert_eq!(local_refusal_ceiling(&rt, &local), None);
    }

    #[test]
    fn a_proven_tiny_window_still_sizes_and_still_refuses() {
        // A PROVEN window at or below the reply reserve used to yield ceiling `None`, which every
        // consumer read as "no ceiling — send everything": the smallest servers, the ones a
        // head-cut hurts most, got the largest unguarded prompts. Now the reserve scales down and
        // both the sizing and the refusal stay armed.
        let rt = LocalRuntime::default();
        let local = arm();
        rt.cache_window(
            &local.base_url,
            &local.model,
            WindowInfo {
                tokens: 1024,
                source: WindowSource::LoadedModel,
            },
        );
        assert_eq!(
            local_prompt_ceiling(&rt, &local),
            Some(768),
            "a measured 1024-token window keeps three quarters for the prompt"
        );
        assert_eq!(local_refusal_ceiling(&rt, &local), Some(768));
        let failure = prompt_fit_failure(&rt, &local, &msgs(10_000))
            .expect("10k characters cannot fit a 1024-token window");
        assert_eq!(failure.kind, LocalFailKind::PromptTooLarge);
        assert!(
            failure.detail.contains("1024"),
            "the message names the real window, not one reconstructed from the ceiling: {}",
            failure.detail
        );
    }

    #[test]
    fn a_refused_prompt_reads_as_pm_s_doing_not_the_server_s() {
        let err = local_failure_to_error(&LocalFailure {
            kind: LocalFailKind::PromptTooLarge,
            detail: "this one needs about 4000 tokens and the server is serving 4096".into(),
        });
        let msg = err.to_string();
        assert!(
            msg.contains("PM didn't send it"),
            "the user must not read this as their server failing: {msg}"
        );
        assert!(msg.contains("4096"), "the numbers ride along: {msg}");
        assert_eq!(
            fail_kind_slug(&LocalFailKind::PromptTooLarge),
            "prompt_too_large"
        );
    }

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

    /// The byte-identical invariant made mechanical: with an EMPTY runtime context, the resolver's
    /// output must equal the raw preference for every (role, preference) pair — the power policy
    /// acts only through a populated context, so on mains routing is exactly a direct preference
    /// lookup. The same reasoning as making the IPC boundary enforced rather than merely documented
    /// (#432 items 1-3).
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

    // ---- the On battery policy (#432) ----

    use crate::power::PowerReading;

    fn snap(state: PowerState, threshold: Option<u8>) -> PowerSnapshot {
        PowerSnapshot {
            reading: PowerReading::default(),
            state,
            threshold,
        }
    }

    fn yes(consent: bool) -> Consent {
        if consent {
            Consent::ALL
        } else {
            Consent::NONE
        }
    }

    fn settings(threshold: u8, scope: PowerScope, consent: bool) -> PowerSettings {
        PowerSettings {
            threshold,
            scope,
            consent: yes(consent),
        }
    }

    fn low(scope: PowerScope, consent: bool, keep_local: bool) -> RuntimeContext {
        RuntimeContext {
            battery_low: true,
            scope,
            consent: yes(consent),
            keep_local,
        }
    }

    fn is_inert(ctx: &RuntimeContext) -> bool {
        !ctx.battery_low
            && ctx.scope == PowerScope::Both
            && ctx.consent == Consent::NONE
            && !ctx.keep_local
    }

    fn cloud(models: &[&str]) -> CloudArm {
        CloudArm {
            key: Secret::from("k".to_string()),
            models: models.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn r2_the_context_is_inert_unless_the_latch_and_the_store_agree() {
        let on = settings(60, PowerScope::Chat, true);
        assert!(is_inert(&RuntimeContext::from_parts(
            &snap(PowerState::Battery, Some(60)),
            &on,
            true
        )));
        assert!(is_inert(&RuntimeContext::from_parts(
            &snap(PowerState::BatteryLow, Some(60)),
            &settings(0, PowerScope::Chat, true),
            true
        )));
        // A vault switched mid-tick: the latch was computed on another store's threshold.
        assert!(is_inert(&RuntimeContext::from_parts(
            &snap(PowerState::BatteryLow, Some(60)),
            &settings(40, PowerScope::Chat, true),
            true
        )));
        // A stale latch already reads as Mains.
        assert!(is_inert(&RuntimeContext::from_parts(
            &snap(PowerState::Mains, Some(60)),
            &on,
            false
        )));
        let ctx = RuntimeContext::from_parts(&snap(PowerState::BatteryLow, Some(60)), &on, true);
        assert!(ctx.battery_low);
        assert_eq!(ctx.scope, PowerScope::Chat);
        assert_eq!(ctx.consent, Consent::ALL);
        assert!(ctx.keep_local);
    }

    #[test]
    fn r3_on_a_low_battery_only_local_then_cloud_moves() {
        let ctx = low(PowerScope::Both, true, false);
        for role in [Role::Chat, Role::Background] {
            for (pref, expected) in [
                (ProviderPref::Cloud, ProviderChoice::Cloud),
                (ProviderPref::Local, ProviderChoice::Local),
                (ProviderPref::LocalThenCloud, ProviderChoice::CloudForPower),
            ] {
                assert_eq!(
                    resolve_provider(role, &RoutingPrefs::uniform(pref), &ctx),
                    expected,
                    "{role:?} {pref:?}"
                );
            }
        }
    }

    #[test]
    fn r4_r5_the_scope_picks_the_role() {
        let prefs = RoutingPrefs::uniform(ProviderPref::LocalThenCloud);
        let chat = low(PowerScope::Chat, true, false);
        assert_eq!(
            resolve_provider(Role::Chat, &prefs, &chat),
            ProviderChoice::CloudForPower
        );
        assert_eq!(
            resolve_provider(Role::Background, &prefs, &chat),
            ProviderChoice::LocalThenCloud
        );
        let background = low(PowerScope::Background, true, false);
        assert_eq!(
            resolve_provider(Role::Background, &prefs, &background),
            ProviderChoice::CloudForPower
        );
        assert_eq!(
            resolve_provider(Role::Chat, &prefs, &background),
            ProviderChoice::LocalThenCloud
        );
    }

    #[test]
    fn a_yes_about_one_role_never_moves_the_other() {
        // Asked while only background could move, the user said yes to background work. Chat
        // becoming movable later (a key added, its routing changed, the scope widened) must be asked
        // about, not carried in on the earlier answer.
        let prefs = RoutingPrefs::uniform(ProviderPref::LocalThenCloud);
        let ctx = RuntimeContext {
            battery_low: true,
            scope: PowerScope::Both,
            consent: Consent::NONE.with(PowerScope::Background),
            keep_local: false,
        };
        assert_eq!(
            resolve_provider(Role::Background, &prefs, &ctx),
            ProviderChoice::CloudForPower
        );
        assert_eq!(
            resolve_provider(Role::Chat, &prefs, &ctx),
            ProviderChoice::LocalThenCloud
        );
        assert_eq!(
            power_route(Role::Chat, &ctx, None),
            PowerRoute::NeedsConsent
        );
        assert_eq!(power_route(Role::Background, &ctx, None), PowerRoute::Cloud);
    }

    #[test]
    fn r6_r7_no_consent_or_the_override_keeps_it_local() {
        let prefs = RoutingPrefs::uniform(ProviderPref::LocalThenCloud);
        for ctx in [
            low(PowerScope::Both, false, false),
            low(PowerScope::Both, true, true),
        ] {
            for role in [Role::Chat, Role::Background] {
                assert_eq!(
                    resolve_provider(role, &prefs, &ctx),
                    ProviderChoice::LocalThenCloud
                );
            }
        }
    }

    #[test]
    fn r8_power_blocked_reports_in_the_copys_order() {
        use KeyPresence::*;
        for local_ready in [true, false] {
            for key in [Present, Absent, Unreadable] {
                assert_eq!(
                    power_blocked(ProviderPref::Cloud, local_ready, key),
                    Some(PowerBlocked::CloudRouting)
                );
            }
        }
        assert_eq!(
            power_blocked(ProviderPref::LocalThenCloud, false, Present),
            Some(PowerBlocked::NoLocalModel)
        );
        assert_eq!(
            power_blocked(ProviderPref::LocalThenCloud, true, Absent),
            Some(PowerBlocked::NoKey)
        );
        assert_eq!(
            power_blocked(ProviderPref::LocalThenCloud, true, Unreadable),
            Some(PowerBlocked::KeyUnreadable)
        );
        assert_eq!(
            power_blocked(ProviderPref::Local, true, Present),
            Some(PowerBlocked::LocalOnly)
        );
        assert_eq!(
            power_blocked(ProviderPref::Local, true, Absent),
            Some(PowerBlocked::NoKey),
            "a keyless Local only user gets the keyless copy"
        );
        assert_eq!(
            power_blocked(ProviderPref::LocalThenCloud, true, Present),
            None
        );
    }

    #[test]
    fn r9_power_route_rows() {
        let r = Role::Chat;
        assert_eq!(
            power_route(
                r,
                &low(PowerScope::Both, true, false),
                Some(PowerBlocked::NoKey)
            ),
            PowerRoute::Unchanged
        );
        assert_eq!(
            power_route(r, &RuntimeContext::default(), None),
            PowerRoute::Unchanged
        );
        assert_eq!(
            power_route(r, &low(PowerScope::Background, true, false), None),
            PowerRoute::Unchanged
        );
        assert_eq!(
            power_route(r, &low(PowerScope::Both, false, true), None),
            PowerRoute::KeptLocal
        );
        assert_eq!(
            power_route(r, &low(PowerScope::Both, false, false), None),
            PowerRoute::NeedsConsent
        );
        assert_eq!(
            power_route(r, &low(PowerScope::Both, true, false), None),
            PowerRoute::Cloud
        );
    }

    /// The status and routing must never disagree: across every combination, the status says
    /// "cloud" exactly when `resolve_provider` would move the role AND `resolve` would find both arms
    /// to hydrate.
    #[test]
    fn r10_the_status_says_cloud_exactly_when_routing_moves_the_role() {
        use KeyPresence::*;
        for role in [Role::Chat, Role::Background] {
            for pref in [
                ProviderPref::Cloud,
                ProviderPref::Local,
                ProviderPref::LocalThenCloud,
            ] {
                for local_ready in [true, false] {
                    for key in [Present, Absent, Unreadable] {
                        for battery_low in [true, false] {
                            for scope in
                                [PowerScope::Chat, PowerScope::Background, PowerScope::Both]
                            {
                                for consent in [
                                    Consent::ALL,
                                    Consent::NONE,
                                    Consent::NONE.with(PowerScope::Chat),
                                    Consent::NONE.with(PowerScope::Background),
                                ] {
                                    for keep_local in [true, false] {
                                        let ctx = RuntimeContext {
                                            battery_low,
                                            scope,
                                            consent,
                                            keep_local,
                                        };
                                        let status = power_route(
                                            role,
                                            &ctx,
                                            power_blocked(pref, local_ready, key),
                                        ) == PowerRoute::Cloud;
                                        let routed = resolve_provider(
                                            role,
                                            &RoutingPrefs::uniform(pref),
                                            &ctx,
                                        ) == ProviderChoice::CloudForPower
                                            && local_ready
                                            && key == Present;
                                        assert_eq!(
                                            status, routed,
                                            "{role:?} {pref:?} ready={local_ready} {key:?} {ctx:?}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn r11_hydration_substitutes_only_a_both_arms_pair() {
        match hydrate_local_then_cloud(
            true,
            Some(LocalArm {
                model: "gemma".into(),
                ..arm()
            }),
            Some(cloud(&["a", "b"])),
        ) {
            Some(RoutePlan::CloudForPower {
                cloud,
                displaced_local_model,
            }) => {
                assert_eq!(cloud.models, vec!["a".to_string(), "b".to_string()]);
                assert_eq!(displaced_local_model, "gemma");
            }
            _ => panic!("a power route with both arms is CloudForPower"),
        }
        assert!(matches!(
            hydrate_local_then_cloud(false, Some(arm()), Some(cloud(&["a"]))),
            Some(RoutePlan::LocalThenCloud { .. })
        ));
        assert!(matches!(
            hydrate_local_then_cloud(true, Some(arm()), None),
            Some(RoutePlan::LocalOnly(_))
        ));
        assert!(matches!(
            hydrate_local_then_cloud(true, None, Some(cloud(&["a"]))),
            Some(RoutePlan::Cloud(_))
        ));
        assert!(hydrate_local_then_cloud(true, None, None).is_none());
    }

    #[test]
    fn r12_a_power_plan_is_a_cloud_plan_for_attribution_and_sizing() {
        let plan = RoutePlan::CloudForPower {
            cloud: cloud(&["a", "b"]),
            displaced_local_model: "gemma".into(),
        };
        assert_eq!(plan.primary_model_id(), "a");
        assert_eq!(plan.models(), ["a".to_string(), "b".to_string()]);
        assert!(plan.is_power_routed());
        assert!(sizing_arm(&plan).is_none(), "sized like any cloud route");
        assert!(!RoutePlan::LocalThenCloud {
            local: arm(),
            cloud: cloud(&["a"])
        }
        .is_power_routed());
        assert!(sizing_arm(&RoutePlan::LocalOnly(arm())).is_some());
    }

    #[test]
    fn r13_a_power_route_is_never_a_failure() {
        let meta = CallMeta::cloud_fallback(
            std::time::Duration::from_millis(5),
            FallbackReason::PowerPolicy,
            "gemma".into(),
        );
        assert!(meta.failure_fallback().is_none());
        assert!(meta.power_routed());
        let slug = meta.fallback.as_ref().unwrap().as_log_str();
        assert_eq!(slug, "power_policy");
        assert!(!slug.starts_with("hard_failure:"));

        let failed = CallMeta::cloud_fallback(
            std::time::Duration::from_millis(5),
            FallbackReason::HardFailure(LocalFailKind::Timeout),
            "gemma".into(),
        );
        assert!(failed.failure_fallback().is_some());
        assert!(!failed.power_routed());
    }

    /// The wire spelling the TypeScript mirror (`types.ts`) is written against.
    #[test]
    fn the_power_enums_serialize_as_the_frontend_mirrors_them() {
        let json = |v: serde_json::Value| v.as_str().unwrap().to_string();
        for (route, wire) in [
            (PowerRoute::Unchanged, "unchanged"),
            (PowerRoute::NeedsConsent, "needs_consent"),
            (PowerRoute::KeptLocal, "kept_local"),
            (PowerRoute::Cloud, "cloud"),
        ] {
            assert_eq!(json(serde_json::to_value(route).unwrap()), wire);
        }
        for (blocked, wire) in [
            (PowerBlocked::CloudRouting, "cloud_routing"),
            (PowerBlocked::NoLocalModel, "no_local_model"),
            (PowerBlocked::NoKey, "no_key"),
            (PowerBlocked::KeyUnreadable, "key_unreadable"),
            (PowerBlocked::LocalOnly, "local_only"),
        ] {
            assert_eq!(json(serde_json::to_value(blocked).unwrap()), wire);
        }
    }

    #[test]
    fn r14_a_failed_power_route_names_the_override() {
        let msg = power_route_error(Error::Other("x".into())).to_string();
        assert!(msg.contains('x'));
        assert!(msg.contains(POWER_ROUTED_FAILED_HINT));
        assert!(msg.contains("Keep using local until I quit PM"));
    }
}
