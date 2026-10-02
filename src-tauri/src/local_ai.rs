// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Tauri command surface + I/O orchestration for the user-configured local endpoint (#297):
//! auto-detect the three named servers, the posture-checked endpoint check (resolve the address and
//! refuse to send a token + chats in the clear to a public host, warn when the server is exposed on
//! the LAN), config get/set, model listing, and a live status snapshot. All of it is Rust-side —
//! the CSP allows no direct network from the webview. The pure runtime discipline lives in
//! [`crate::local_slot`]; the wire in [`crate::openai_compat`].
//!
//! Threat model note (Bobby): the risk here is that the USER'S model server may be exposed, not that
//! PM is attackable. PM cannot secure a server it does not run — so the checks INFORM (refuse public
//! cleartext, warn on LAN exposure) and the copy says so plainly.

use std::net::IpAddr;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::llm_gateway::{
    self, KeyPresence, PowerRoute, Role, BACKGROUND_ROUTING_KEY, CHAT_ROUTING_KEY,
    LOCAL_BACKGROUND_MODEL_KEY, LOCAL_BASE_URL_KEY, LOCAL_CHAT_MODEL_KEY,
};
use crate::local_slot::{classify_ip, posture_for, EndpointClass, PostureVerdict};
use crate::power::{self, PowerReading, PowerSettings, PowerSnapshot};
use crate::{
    better_fit, db, fit, hardware, local_catalog, local_disk, openai_compat, paths, power_source,
    residency, secrets, AppState,
};

/// The three servers PM knows how to auto-detect, by their default loopback port.
const KNOWN_PORTS: &[(u16, &str)] = &[
    (11434, "Ollama"),
    (1234, "LM Studio"),
    (8080, "llama-server"),
];

/// Settings key: an extra folder to include in the on-disk model crawl (#449), for weights kept
/// somewhere PM wouldn't think to look (a `--local-dir` download, a shared model library on another
/// drive). Absent = crawl only the runners' own locations.
pub const LOCAL_MODEL_SCAN_DIR_KEY: &str = "local_model_scan_dir";

// ---------------------------------------------------------------------------------------------
// Auto-detect
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct DetectedEndpoint {
    pub url: String,
    pub label: String,
    pub models: Vec<String>,
}

/// Probe the three known loopback ports and return each that answers like an OpenAI `/v1/models`
/// server. A port that is merely open (some other service) is rejected by the shape check, so this
/// never claims a server that isn't one.
#[tauri::command]
pub async fn probe_local_llm_ports() -> Result<Vec<DetectedEndpoint>> {
    let mut found = Vec::new();
    for (port, label) in KNOWN_PORTS {
        let url = format!("http://127.0.0.1:{port}");
        if let Ok(models) = openai_compat::probe(&url, None).await {
            found.push(DetectedEndpoint {
                url,
                label: (*label).to_string(),
                models,
            });
        }
    }
    Ok(found)
}

// ---------------------------------------------------------------------------------------------
// Endpoint check — resolve the address, apply the http posture, probe reachability + LAN exposure
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct EndpointCheck {
    /// The endpoint answered like an OpenAI `/v1/models` server.
    pub reachable: bool,
    /// The URL after normalisation (bare base, no trailing `/` or `/v1`).
    pub normalized_url: String,
    /// The model ids it serves (empty when unreachable or refused). Reported verbatim, INCLUDING
    /// embedding/reranking models — this is "what the server serves", and shrinking it would
    /// misreport the endpoint in the reachability readout.
    pub models: Vec<String>,
    /// The subset of `models` that can actually answer a chat turn — `models` minus the embedders.
    /// Anything that binds a model to a role picks from HERE, never from `models[0]`.
    pub assignable: Vec<String>,
    /// Where the resolved address sits: `"loopback"` | `"private"` | `"public"`.
    pub posture: String,
    /// The http/https verdict: `"ok"` | `"warn_unencrypted"` | `"refused_public_cleartext"`.
    pub scheme_verdict: String,
    /// The (loopback) server ALSO answers on a non-loopback interface — bound to 0.0.0.0, so anyone
    /// on the user's network can reach it. A plain warning; PM can't fix a server it doesn't run.
    pub exposed_on_network: bool,
    /// A human note to show (a warning or the refusal reason), or `None` when all-clear.
    pub message: Option<String>,
}

/// Check a candidate endpoint before it is saved: normalise it, RESOLVE the address (never trust the
/// hostname string — `localhost` can resolve anywhere), apply the http posture, then — unless the
/// posture refuses it — probe reachability and whether a loopback server is also exposed on the LAN.
#[tauri::command]
pub async fn check_local_llm_endpoint(url: String, token: Option<String>) -> Result<EndpointCheck> {
    let normalized = openai_compat::normalize_base_url(&url)?;
    let (scheme, host, port) = split_scheme_host_port(&normalized)?;
    let class = resolve_endpoint_class(&host, port).await?;
    let verdict = posture_for(&scheme, class);

    // Refuse a public cleartext endpoint outright — no probe, nothing sent.
    if verdict == PostureVerdict::RefusePublicCleartext {
        return Ok(EndpointCheck {
            reachable: false,
            normalized_url: normalized,
            models: Vec::new(),
            assignable: Vec::new(),
            posture: class_str(class).to_string(),
            scheme_verdict: verdict_str(verdict).to_string(),
            exposed_on_network: false,
            message: Some(
                "Refusing to send your token and chat text in the clear to a public address. \
                 Use an https URL, or a server on your own machine or private network."
                    .to_string(),
            ),
        });
    }

    let probe = openai_compat::probe(&normalized, token.as_deref()).await;
    let (reachable, models, reach_note) = match probe {
        Ok(models) => (true, models, None),
        Err(f) => (
            false,
            Vec::new(),
            // Lead with the diagnosis, not the symptom. "Couldn't reach the endpoint" plus a
            // transport error tells someone what happened and nothing about what to do; the gateway
            // has said "is the server running?" on this exact failure since #297, and the two
            // surfaces disagreeing meant the one people meet FIRST was the less useful of them.
            Some(format!(
                "Couldn't reach it — is the server running? ({})",
                crate::error::truncate_detail(&f.detail)
            )),
        ),
    };

    // Exposure probe (only meaningful for a loopback endpoint): does the same server also answer on
    // this machine's LAN address? If so it is bound to 0.0.0.0 and reachable by others on the network.
    let exposed = if reachable && class == EndpointClass::Loopback {
        probe_lan_exposure(&scheme, port, token.as_deref()).await
    } else {
        false
    };

    let message = build_check_message(verdict, exposed, reach_note);
    let assignable: Vec<String> = models
        .iter()
        .filter(|id| !local_catalog::is_embedding_or_reranker(id))
        .cloned()
        .collect();
    Ok(EndpointCheck {
        reachable,
        normalized_url: normalized,
        models,
        assignable,
        posture: class_str(class).to_string(),
        scheme_verdict: verdict_str(verdict).to_string(),
        exposed_on_network: exposed,
        message,
    })
}

fn class_str(class: EndpointClass) -> &'static str {
    match class {
        EndpointClass::Loopback => "loopback",
        EndpointClass::PrivateRemote => "private",
        EndpointClass::PublicRemote => "public",
    }
}

fn verdict_str(verdict: PostureVerdict) -> &'static str {
    match verdict {
        PostureVerdict::Ok => "ok",
        PostureVerdict::WarnUnencrypted => "warn_unencrypted",
        PostureVerdict::RefusePublicCleartext => "refused_public_cleartext",
    }
}

/// Compose the human note from the posture verdict, the LAN-exposure finding, and any reachability
/// problem. Honest that PM can't secure a server it doesn't run.
fn build_check_message(
    verdict: PostureVerdict,
    exposed: bool,
    reach_note: Option<String>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(note) = reach_note {
        parts.push(note);
    }
    if verdict == PostureVerdict::WarnUnencrypted {
        parts.push(
            "This connection is unencrypted (http) — fine on a trusted network, but the traffic is \
             visible to others on it."
                .to_string(),
        );
    }
    if exposed {
        parts.push(
            "This server also answers on your network, not just this machine — anyone on your \
             network can reach it. PM can't secure a server it doesn't run; restrict the server \
             itself (e.g. bind it to localhost) if that isn't intended."
                .to_string(),
        );
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

// ---------------------------------------------------------------------------------------------
// Address resolution + LAN exposure — the I/O behind the posture checks
// ---------------------------------------------------------------------------------------------

/// Split a normalised base URL into (scheme, host, port). The normalised form has no path, so the
/// authority is everything after `://`. IPv6 literals are bracketed (`[::1]:11434`).
fn split_scheme_host_port(base_url: &str) -> Result<(String, String, u16)> {
    let (scheme, authority) = base_url
        .split_once("://")
        .ok_or_else(|| Error::Other("the endpoint URL is missing a scheme".into()))?;
    let default_port = if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        80
    };
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        // [ipv6]:port
        let (h, tail) = rest
            .split_once(']')
            .ok_or_else(|| Error::Other("malformed IPv6 endpoint URL".into()))?;
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(default_port);
        (h.to_string(), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(default_port))
    } else {
        (authority.to_string(), default_port)
    };
    if host.is_empty() {
        return Err(Error::Other("the endpoint URL has no host".into()));
    }
    Ok((scheme.to_string(), host, port))
}

/// Classify an endpoint by its RESOLVED address (not the hostname string). An IP literal classifies
/// with no DNS; a name is resolved off the async runtime. If a name resolves to several addresses,
/// the MOST public one wins — a name that resolves to both 127.0.0.1 and a public address must not
/// be treated as loopback.
async fn resolve_endpoint_class(host: &str, port: u16) -> Result<EndpointClass> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(classify_ip(ip));
    }
    let hostport = format!("{host}:{port}");
    let addrs = tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        hostport.to_socket_addrs().map(|it| it.collect::<Vec<_>>())
    })
    .await
    .map_err(|e| Error::Other(format!("address resolver task failed: {e}")))?
    .map_err(|_| Error::Other("couldn't resolve the endpoint host".into()))?;

    addrs
        .iter()
        .map(|a| classify_ip(a.ip()))
        .max_by_key(|c| class_rank(*c))
        .ok_or_else(|| Error::Other("the endpoint host resolved to no address".into()))
}

fn class_rank(c: EndpointClass) -> u8 {
    match c {
        EndpointClass::Loopback => 0,
        EndpointClass::PrivateRemote => 1,
        EndpointClass::PublicRemote => 2,
    }
}

// ---------------------------------------------------------------------------------------------
// The CALL-TIME posture gate — the same verdict `set_local_llm_endpoint` enforces at save time,
// re-asked at the I/O edge because DNS can move under a stored hostname.
// ---------------------------------------------------------------------------------------------

/// The one wording every call-time refusal uses — the four endpoint commands and the chat/background
/// gateway alike — so a user who hits this sees one explanation rather than five phrasings. The
/// save-time message in [`set_local_llm_endpoint`] stays separate because "won't save" is not what
/// happened here.
pub(crate) const CALL_TIME_REFUSAL: &str =
    "won't send your token and chats in the clear to a public address — this endpoint's address now \
     resolves to a public host over http. Use https, or a server on your own machine or private \
     network.";

/// Re-apply the save-time posture at CALL time. Posture is a property of the RESOLVED address, but
/// it is only ever decided when the endpoint is saved: a stored hostname's DNS answer is free to
/// change afterwards (a moved host, a recycled name, a rebinding record), and every path then sends
/// the bearer token — and on the chat paths the full chat text — to whatever it now points at.
///
/// Refuses ONLY `(http, public)` — bit-for-bit the verdict `set_local_llm_endpoint` enforces at the
/// storage boundary, via the same [`posture_for`]. Loopback, LAN and tunnelled (CGNAT/Tailscale)
/// endpoints, and every https endpoint anywhere, behave exactly as before: no policy change, so no
/// user breakage.
///
/// A resolution FAILURE is deliberately NOT a refusal. Failing open on "don't know" and closed only
/// on a positive public-cleartext classification is load-bearing: making a DNS blip a refusal would
/// cost a local-then-cloud user their cloud fallback, and an endpoint that truly cannot be resolved
/// fails as `Refused` a moment later anyway. Do not collapse that asymmetry in a tidy-up.
///
/// Takes only the base URL — never the circuit breaker. A refusal is a verdict on the ADDRESS, not
/// evidence the host is dead, so it must never be recorded as a strike.
///
/// **This NARROWS the window; it does not close it.** `resolve_endpoint_class` runs its own
/// lookup and reqwest performs a second, independent one when the request is actually sent, so the
/// gap shrinks from "unbounded since the endpoint was saved" to "between our lookup and reqwest's".
/// Closing it needs a pinned-IP resolver and per-host clients.
pub(crate) async fn endpoint_refused_now(base_url: &str) -> bool {
    let Ok((scheme, host, port)) = split_scheme_host_port(base_url) else {
        return false;
    };
    // An IP literal short-circuits inside `resolve_endpoint_class` with no DNS at all, so the
    // overwhelmingly common `http://127.0.0.1:11434` case costs nothing per call.
    let Ok(class) = resolve_endpoint_class(&host, port).await else {
        return false;
    };
    posture_for(&scheme, class) == PostureVerdict::RefusePublicCleartext
}

/// The configured endpoint as the call-time gate sees it.
enum Endpoint {
    /// No base URL is configured.
    Unconfigured,
    /// Configured, gate passed: the base URL and the optional bearer token.
    Ready(String, Option<crate::secret::Secret>),
    /// Configured, but its address resolves public over cleartext right now — nothing may be sent.
    Refused,
}

/// The shared prologue for every command that talks to the configured endpoint: read the stored base
/// URL, apply the call-time gate, then fetch the token. One helper rather than four near-identical
/// prologues, so a future caller inherits the gate by construction rather than by review — while
/// each caller still decides what a refusal MEANS for it (a hard error for an explicit user action,
/// a quiet degrade for a best-effort readout).
///
/// The token is fetched only AFTER the gate passes, so a refused endpoint costs no keychain read.
///
/// Ordering is load-bearing (rule #4 — the DB mutex is not reentrant): the `state.conn()` guard is
/// scoped to the read block and dropped before the first `.await`. Never widen that block.
async fn configured_endpoint(app: &AppHandle) -> Result<Endpoint> {
    let base_url = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, LOCAL_BASE_URL_KEY)?
    };
    let Some(base_url) = base_url else {
        return Ok(Endpoint::Unconfigured);
    };
    if endpoint_refused_now(&base_url).await {
        return Ok(Endpoint::Refused);
    }
    Ok(Endpoint::Ready(
        base_url,
        secrets::get_local_llm_endpoint_token()?,
    ))
}

/// Whether a loopback server is ALSO reachable on this machine's LAN address (i.e. bound to
/// 0.0.0.0). Best-effort: if the LAN address can't be determined, report not-exposed.
async fn probe_lan_exposure(scheme: &str, port: u16, token: Option<&str>) -> bool {
    let Some(lan_ip) = local_lan_ip() else {
        return false;
    };
    let host = match lan_ip {
        IpAddr::V6(v6) => format!("[{v6}]"),
        IpAddr::V4(v4) => v4.to_string(),
    };
    let url = format!("{scheme}://{host}:{port}");
    openai_compat::probe(&url, token).await.is_ok()
}

/// This machine's primary LAN address via the "connect a UDP socket, read the local addr" trick —
/// it sends NO packets, it only makes the OS pick the outbound interface. `None` when it can't be
/// determined (offline / unusual network). Loopback is filtered out (that's not a LAN address).
fn local_lan_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr()
        .ok()
        .map(|a| a.ip())
        .filter(|ip| !ip.is_loopback())
}

// ---------------------------------------------------------------------------------------------
// Config get/set — the base URL, per-role model + routing, and the optional token
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct LocalLlmConfig {
    pub base_url: Option<String>,
    pub chat_model: Option<String>,
    pub background_model: Option<String>,
    /// `"cloud"` | `"local"` | `"local-then-cloud"` (absent → `"cloud"`).
    pub chat_routing: String,
    pub background_routing: String,
    /// A bearer token is stored (presence only — the value never leaves Rust).
    pub has_token: bool,
}

#[tauri::command]
pub fn get_local_llm_config(state: State<'_, AppState>) -> Result<LocalLlmConfig> {
    let conn = state.conn()?;
    let routing = |key: &str| -> Result<String> {
        Ok(db::get_setting(&conn, key)?.unwrap_or_else(|| "cloud".to_string()))
    };
    Ok(LocalLlmConfig {
        base_url: db::get_setting(&conn, LOCAL_BASE_URL_KEY)?,
        chat_model: db::get_setting(&conn, LOCAL_CHAT_MODEL_KEY)?,
        background_model: db::get_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY)?,
        chat_routing: routing(CHAT_ROUTING_KEY)?,
        background_routing: routing(BACKGROUND_ROUTING_KEY)?,
        has_token: secrets::has_local_llm_endpoint_token()?,
    })
}

/// Normalise + save the endpoint base URL. Enforces the http posture at the storage boundary too
/// (defence in depth): a public cleartext URL is REFUSED, never stored. Returns the normalised URL.
#[tauri::command]
pub async fn set_local_llm_endpoint(app: AppHandle, url: String) -> Result<String> {
    let normalized = openai_compat::normalize_base_url(&url)?;
    let (scheme, host, port) = split_scheme_host_port(&normalized)?;
    let class = resolve_endpoint_class(&host, port).await?;
    if posture_for(&scheme, class) == PostureVerdict::RefusePublicCleartext {
        return Err(Error::Other(
            "won't save a public http endpoint — a token and chats would travel in the clear. \
             Use https, or a server on your own machine or private network."
                .into(),
        ));
    }
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    db::set_setting(&conn, LOCAL_BASE_URL_KEY, &normalized)?;
    drop(conn);
    // The release passes decide who may be sent the saved token from this cache. Refreshed here, not
    // only on the next release tick: the UI saves a new URL and its token in one click, and a store
    // that closed inside that gap would otherwise pair the new token with the old server.
    state
        .local_ai
        .cache_release_endpoint(Some(normalized.clone()));
    // The last test proved a model answered on the OLD server. Cleared in the backend, not just in
    // the view, because the view re-reads this snapshot every time it mounts.
    state.local_ai.clear_finished_test();
    // A newly-configured endpoint should light up the chat sidebar / status chip at once.
    crate::llm_gateway::ping_status(&app);
    Ok(normalized)
}

/// Forget the local endpoint entirely: the base URL, both role models, and the token. Routing
/// preferences are left as-is (absent base URL already makes them fall through to cloud).
#[tauri::command]
pub fn clear_local_llm_endpoint(app: AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    db::delete_setting(&conn, LOCAL_BASE_URL_KEY)?;
    db::delete_setting(&conn, LOCAL_CHAT_MODEL_KEY)?;
    db::delete_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY)?;
    drop(conn);
    // No endpoint now owns a token — see `set_local_llm_endpoint`.
    state.local_ai.cache_release_endpoint(None);
    secrets::clear_local_llm_endpoint_token()?;
    state.local_ai.clear_finished_test();
    // A forgotten endpoint should drop the chat sidebar's provider line to zero pixels at once.
    crate::llm_gateway::ping_status(&app);
    Ok(())
}

#[tauri::command]
pub fn set_local_llm_role_model(app: AppHandle, role: String, model: String) -> Result<()> {
    let key = role_model_key(&role)?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    if model.trim().is_empty() {
        db::delete_setting(&conn, key)?;
    } else {
        db::set_setting(&conn, key, model.trim())?;
    }
    drop(conn);
    // The sidebar's per-role model rows (#794) read the status snapshot; without a ping they named
    // the OLD model until the next real call — for the background role, potentially hours. Same
    // rule as the endpoint set/clear above: a settings change the sidebar reports must show at once.
    crate::llm_gateway::ping_status(&app);
    Ok(())
}

#[tauri::command]
pub fn set_local_llm_routing(app: AppHandle, role: String, pref: String) -> Result<()> {
    let key = role_routing_key(&role)?;
    // Validate the preference string so an unknown value can't silently read back as "cloud".
    if !matches!(pref.as_str(), "cloud" | "local" | "local-then-cloud") {
        return Err(Error::Other(format!("unknown routing preference '{pref}'")));
    }
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    db::set_setting(&conn, key, &pref)?;
    drop(conn);
    // Same as the role-model write: routing decides which model the sidebar names.
    crate::llm_gateway::ping_status(&app);
    Ok(())
}

/// A new token is a different server as far as a test result is concerned: the last pass proved a
/// model answered with the OLD credential. Cleared here, as the endpoint commands do, because the
/// view re-reads the backend's finished test every time it mounts.
#[tauri::command]
pub fn set_local_llm_token(state: State<'_, AppState>, token: String) -> Result<()> {
    secrets::set_local_llm_endpoint_token(&token)?;
    state.local_ai.clear_finished_test();
    Ok(())
}

#[tauri::command]
pub fn clear_local_llm_token(state: State<'_, AppState>) -> Result<()> {
    secrets::clear_local_llm_endpoint_token()?;
    state.local_ai.clear_finished_test();
    Ok(())
}

fn role_model_key(role: &str) -> Result<&'static str> {
    match role {
        "chat" => Ok(LOCAL_CHAT_MODEL_KEY),
        "background" => Ok(LOCAL_BACKGROUND_MODEL_KEY),
        other => Err(Error::Other(format!("unknown role '{other}'"))),
    }
}

fn role_routing_key(role: &str) -> Result<&'static str> {
    match role {
        "chat" => Ok(CHAT_ROUTING_KEY),
        "background" => Ok(BACKGROUND_ROUTING_KEY),
        other => Err(Error::Other(format!("unknown role '{other}'"))),
    }
}

// ---------------------------------------------------------------------------------------------
// Model listing + live status
// ---------------------------------------------------------------------------------------------

/// The models the CONFIGURED endpoint currently serves (for the model pickers). Errors with a
/// friendly message if nothing is configured or it can't be reached.
#[tauri::command]
pub async fn list_local_llm_models(app: AppHandle) -> Result<Vec<ServedModel>> {
    let (base_url, token) = match configured_endpoint(&app).await? {
        Endpoint::Ready(base_url, token) => (base_url, token),
        Endpoint::Unconfigured => {
            return Err(Error::Other("no local endpoint is configured".into()))
        }
        Endpoint::Refused => return Err(Error::Other(CALL_TIME_REFUSAL.into())),
    };
    let ids = openai_compat::probe(&base_url, token.as_ref().map(|s| s.expose()))
        .await
        .map_err(|f| {
            Error::Other(format!(
                "couldn't list models ({})",
                crate::error::truncate_detail(&f.detail)
            ))
        })?;
    // Every id is returned — the picker shows an embedder DISABLED with the reason, rather than
    // dropping it. A model the user can see in Ollama but not in PM's list reads as a PM bug; a
    // model shown with "can't answer chats" reads as an explanation. It also makes a false positive
    // from `is_embedding_or_reranker` visible instead of silent.
    Ok(ids.into_iter().map(ServedModel::classify).collect())
}

/// Ask the user's own (loopback) Ollama to download `model` into itself, streaming progress on
/// `on_event`. Ollama is the only local runner PM knows with a native pull API — for LM Studio /
/// llama-server the tab shows a copy-paste command instead. PM downloads nothing itself and proxies
/// nothing: this triggers the user's server to fetch the weights. Errors with a friendly message when
/// no endpoint is configured or the pull fails (e.g. the endpoint isn't Ollama, so `/api/pull` 404s).
#[tauri::command]
pub async fn pull_local_model(
    app: AppHandle,
    model: String,
    on_event: tauri::ipc::Channel<openai_compat::PullProgress>,
) -> Result<()> {
    let (base_url, token) = match configured_endpoint(&app).await? {
        Endpoint::Ready(base_url, token) => (base_url, token),
        Endpoint::Unconfigured => {
            return Err(Error::Other("no local endpoint is configured".into()))
        }
        Endpoint::Refused => return Err(Error::Other(CALL_TIME_REFUSAL.into())),
    };
    // The job is BACKEND-owned from here: the settings view unmounts on every tab switch, and a
    // pull that lived in component state came back as a re-armed Download button over a server
    // still saturating the connection — one click away from a second concurrent `/api/pull` of the
    // same tag. `begin_pull` is also the only-one-at-a-time guard, and the snapshot it maintains
    // is what a remounted view re-reads (`active_local_pull`).
    let Some(cancel) = app.state::<AppState>().local_ai.begin_pull(&model) else {
        let running = app
            .state::<AppState>()
            .local_ai
            .active_pull()
            .map(|s| s.model)
            .unwrap_or_default();
        return Err(Error::Other(format!(
            "a model download is already running ({running}) — wait for it or cancel it first"
        )));
    };
    let progress_app = app.clone();
    let pull = openai_compat::pull_ollama_model(
        &base_url,
        &model,
        token.as_ref().map(|s| s.expose()),
        |p| {
            progress_app.state::<AppState>().local_ai.update_pull(&p);
            let _ = on_event.send(p);
        },
    );
    // Cancellation drops the pull future, which aborts the HTTP request — Ollama ties the download
    // to the request context, so the server-side pull stops too (partial blobs are kept for a
    // resume). A cancel is deliberate, so it lands as Ok with a "cancelled" snapshot, not an error.
    let outcome = tokio::select! {
        outcome = pull => Some(outcome),
        _ = cancel.notified() => None,
    };
    // The pull just changed what is on disk, so the cached crawl (#449) is now a lie — and it is
    // cached for the whole process, so without this PM answers its own download with its own
    // pre-download picture until the app restarts. Cleared unconditionally: a pull that dies (or is
    // cancelled) after Ollama has written the manifest would otherwise leave real weights invisible.
    //
    // Lock-safe: `clear_disk_models` takes only `LocalRuntime.disk_models` for one assignment and
    // never re-enters `state.conn()`, and `configured_endpoint` above dropped its connection before
    // its own await. Same shape as `set_local_model_scan_dir`.
    app.state::<AppState>().local_ai.clear_disk_models();
    let state = app.state::<AppState>();
    match outcome {
        Some(Ok(())) => {
            state.local_ai.finish_pull(None);
            Ok(())
        }
        Some(Err(f)) => {
            let msg = format!(
                "couldn't download the model ({})",
                crate::error::truncate_detail(&f.detail)
            );
            state.local_ai.finish_pull(Some(msg.clone()));
            Err(Error::Other(msg))
        }
        None => {
            state.local_ai.finish_pull_cancelled();
            Ok(())
        }
    }
}

/// The one in-flight (or last-terminal) pull, for a settings view (re)mounting — the snapshot half
/// of the backend-owned job `pull_local_model` runs.
#[tauri::command]
pub fn active_local_pull(state: State<'_, AppState>) -> Option<crate::local_slot::PullSnapshot> {
    state.local_ai.active_pull()
}

/// Stop the running pull. Returns whether there was one to stop.
#[tauri::command]
pub fn cancel_local_pull(state: State<'_, AppState>) -> bool {
    state.local_ai.cancel_pull()
}

#[derive(Serialize)]
pub struct LocalLlmStatus {
    /// A base URL is configured.
    pub configured: bool,
    /// The endpoint answered on the last observation — a `/v1/models` probe, or a real call whose
    /// outcome settled the question. `false` until something has actually been observed.
    pub reachable: bool,
    /// The host is resting inside its dead-host cooldown after repeated failures.
    pub in_cooldown: bool,
    /// Seconds left on the cooldown (0 when not in one).
    pub cooldown_remaining_s: u64,
    /// Whether the reachability figure came from a fresh probe this call, or is the last-known value
    /// (a probe was skipped by the debounce so a fast-polling UI can't spam the user's server).
    pub probed_now: bool,
    /// The local model bound to Chat, but ONLY when chat routing actually sends chat to it. `None`
    /// while the role goes to cloud, including while the On battery policy has moved it — the cloud
    /// model is then the true answer for that row.
    ///
    /// Here because the model footer used to read the OpenRouter list for both rows and had no
    /// access to routing at all — so a machine answering every turn from its own GPU displayed a
    /// cloud model's name, and the local line underneath said only "connected". Read as a set, the
    /// footer stated the exact inverse of what was happening.
    pub chat_local_model: Option<String>,
    /// The same for background work (filing, titles, summaries, learning).
    pub background_local_model: Option<String>,
    /// The context window the server is actually serving for the model bound to the demanding role
    /// — background if it is local, else chat. `None` until a call has loaded a model, because both
    /// proven rungs of the ladder only answer while one is resident.
    ///
    /// Here because it is the number that explains the symptom. PM already probes it, caches it, and
    /// sizes every prompt against it — and until now the only thing that could read it was the chat
    /// meter. A user whose server is serving Ollama's default 4096 has no way to learn that from PM,
    /// and no reason to connect it to filing suddenly getting worse.
    pub served_window: Option<u32>,
    /// Whether [`Self::served_window`] was measured (`/slots`, `/api/ps`) or is PM's conservative
    /// floor. The UI must not present a guess as a reading.
    pub served_window_proven: bool,
    /// WHICH rung answered: `"slots"` | `"loaded_model"` | `"models_meta"` | `"default"`, or `None`
    /// when nothing has been measured yet. The same field, spelled the same way, that the chat
    /// context meter already ships (`conversations.rs` → `ContextMeter.tsx`), so there is one
    /// convention for this and not two.
    ///
    /// The boolean above is not enough, because the two unproven rungs are wrong in OPPOSITE
    /// directions and saying "estimate" for both hides that. `Default` is PM's floor, an
    /// under-estimate. `ModelsMeta` is the server's claim about the MODEL — its trained capacity,
    /// an over-estimate of this load, and the exact confusion that made PM read 32768 off a model
    /// card while the server served 4096 (#792). `served_window` reports that number RAW while
    /// `llm_gateway::sizing_window` clamps it to the floor, so without the source the panel can
    /// show a reassuring 32768 while PM is quietly compressing everything to fit 4096.
    pub window_source: Option<String>,
    /// A local call for this role is in flight RIGHT NOW — the model is answering, or is queued
    /// behind something else that is.
    ///
    /// Counted from the moment the call enters the slot rather than from the moment it reaches the
    /// server, because both mean the same thing to someone reading the footer: PM is asking, and the
    /// answer is not back. Housekeeping (an unload) counts for neither role.
    pub chat_answering: bool,
    pub background_answering: bool,
    /// Whether the role's model is on the graphics card. `None` is "PM cannot tell" — the endpoint
    /// has no `/api/ps` (llama-server, LM Studio, a `/v1`-only proxy), or nothing has been observed
    /// recently enough to still be worth saying. It must never be rendered as "not loaded": that
    /// inversion is the whole reason this is three-valued.
    ///
    /// Describes the role's BOUND local model, also while it is parked by the On battery policy —
    /// a parked model can still be holding the card, which is exactly what the section says.
    pub chat_loaded: Option<bool>,
    pub background_loaded: Option<bool>,
    /// PM itself handed this model back, on the user's own release policy. Only meaningful while the
    /// model is not loaded, and it is what separates "your server let it go" from "you asked PM to".
    /// Like `*_loaded`, about the BOUND model, also while it is parked by the On battery policy.
    pub chat_released: bool,
    pub background_released: bool,
    /// The On battery policy (#432): what the machine reads, what PM is acting on, and where each
    /// role stands. Computed from the same functions `resolve` uses.
    pub power: PowerView,
}

/// One role's standing under the On battery policy.
#[derive(Serialize, Clone, Debug)]
pub struct PowerRoleView {
    pub route: PowerRoute,
    /// Why the policy can never move this role, or `None` if it can.
    pub blocked: Option<llm_gateway::PowerBlocked>,
    /// The role's bound local model (parked while `route` is `Cloud`). `None` for a cloud-routed
    /// role.
    pub local_model: Option<String>,
    /// Where this role's requests really go right now — the route `resolve` would build, from the
    /// preference, the endpoint, the model and the key together. What every route sentence in the
    /// Local AI tab is worded from, so none of them is a guess from the preference alone.
    pub effective: llm_gateway::EffectiveRoute,
    /// Whether this role's cloud key can be read.
    pub cloud_key: KeyPresence,
}

/// The On battery policy's state, for the status snapshot. No getter of its own: the stored values
/// and the live state ride `local_llm_status`, like the release policy rides `GpuResidency`.
#[derive(Serialize, Clone, Debug)]
pub struct PowerView {
    /// The latest raw reading, before settling.
    pub source: power::PowerSource,
    pub percent: Option<u8>,
    pub has_battery: bool,
    /// What PM acts on — `Mains` while the latch is stale.
    pub state: power::PowerState,
    /// The open store's threshold; 0 = never.
    pub threshold: u8,
    /// threshold + 15, so the UI does no arithmetic.
    pub return_at: u8,
    pub roles: power::PowerScope,
    /// The roles the user has said may go to the cloud on battery — a scope, or `None` for none.
    pub consent: power::Consent,
    /// Either role's route is `NeedsConsent`.
    pub consent_needed: bool,
    pub keep_local: bool,
    /// Some OpenRouter key exists, for either role. A role's `NoKey` alone can't say this: chat uses
    /// only the main key, so a background-key-only setup reads `NoKey` for chat while a cloud
    /// provider is very much set up — and the keyless copy would then tell the user it isn't.
    pub any_cloud_key: bool,
    pub chat: PowerRoleView,
    pub background: PowerRoleView,
}

/// The status's power answer, built from the SAME pure functions `resolve` uses, so the section can
/// never describe a route the gateway would not take. Pure, so it is unit-tested.
#[allow(clippy::too_many_arguments)]
pub(crate) fn power_view(
    snap: &PowerSnapshot,
    settings: &PowerSettings,
    keep_local: bool,
    prefs: &llm_gateway::RoutingPrefs,
    endpoint_set: bool,
    chat_bound: Option<&str>,
    background_bound: Option<&str>,
    chat_key: KeyPresence,
    background_key: KeyPresence,
) -> PowerView {
    let ctx = llm_gateway::RuntimeContext::from_parts(snap, settings, keep_local);
    let role = |role: Role, bound: Option<&str>, key: KeyPresence| {
        let local_ready = endpoint_set && bound.is_some();
        let blocked = llm_gateway::power_blocked(prefs.for_role(role), local_ready, key);
        let route = match llm_gateway::power_route(role, &ctx, blocked) {
            // Plugged back in, with the latch still waiting out its minute: there is nothing left to
            // ask about. The question says "you're on battery" and offers the cloud "until you plug
            // in" — both false now — and an answer would only take effect after PM has gone back
            // to local anyway. Routing is unaffected: without consent this role was local already.
            // Only a positive AC reading: one unreadable sample on battery is not "plugged in", and
            // hiding the question then would leave the readout claiming nothing can move.
            PowerRoute::NeedsConsent if snap.reading.source == power::PowerSource::Ac => {
                PowerRoute::Unchanged
            }
            route => route,
        };
        PowerRoleView {
            route,
            blocked,
            local_model: bound.map(str::to_string),
            effective: llm_gateway::effective_route(
                prefs.for_role(role),
                local_ready,
                key,
                route == PowerRoute::Cloud,
            ),
            cloud_key: key,
        }
    };
    let chat = role(Role::Chat, chat_bound, chat_key);
    let background = role(Role::Background, background_bound, background_key);
    PowerView {
        source: snap.reading.source,
        percent: snap.reading.percent,
        has_battery: snap.reading.has_battery,
        state: snap.state,
        threshold: settings.threshold,
        return_at: power::return_at(settings.threshold),
        roles: settings.scope,
        consent: settings.consent,
        consent_needed: chat.route == PowerRoute::NeedsConsent
            || background.route == PowerRoute::NeedsConsent,
        keep_local,
        // Background falls back to the main key, so its presence covers both.
        any_cloud_key: chat_key == KeyPresence::Present || background_key == KeyPresence::Present,
        chat,
        background,
    }
}

/// The local model the status REPORTS for a role: none while the On battery policy has it on the
/// cloud, so the sidebar names the model actually answering.
fn reported_local(bound: Option<String>, route: PowerRoute) -> Option<String> {
    if route == PowerRoute::Cloud {
        None
    } else {
        bound
    }
}

/// The local model a role will really use, or `None` when the role goes to cloud.
///
/// Pure over the two settings so the "what is actually answering" question has one answer, testable
/// without a database. `"cloud"` (and an absent preference, which parses to it) means the local
/// binding is irrelevant however it is set; both local preferences mean local is tried FIRST, which
/// is what the footer is reporting.
pub fn role_local_model(routing: Option<&str>, bound: Option<&str>) -> Option<String> {
    match routing.unwrap_or("cloud") {
        // `trim`, matching the gateway's own emptiness test (`local_arm`) — a whitespace-only
        // stored model must not make the sidebar name a model routing treats as unconfigured.
        "local" | "local-then-cloud" => bound
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// A live status snapshot for the Local AI tab / the chat honesty surface (#297 PR5/PR6). Reads the
/// in-memory circuit-breaker state, and — at most once per [`tunables::HEALTH_PROBE_DEBOUNCE`] — runs
/// one `/v1/models` reachability probe so a fast UI poll can't hammer the user's server.
#[tauri::command]
pub async fn local_llm_status(app: AppHandle) -> Result<LocalLlmStatus> {
    // The power latch is in memory and read BEFORE the DB guard, as `resolve` does.
    let (snap, keep_local) = {
        let state = app.state::<AppState>();
        (
            state.local_ai.power_snapshot(std::time::Instant::now()),
            state.local_ai.keep_local(),
        )
    };
    // One connection for every setting this needs, dropped before the first await.
    let (base_url, configured, chat_bound, background_bound, prefs, power_settings) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let base_url = db::get_setting(&conn, LOCAL_BASE_URL_KEY)?;
        let configured = base_url.is_some();
        let chat = role_local_model(
            db::get_setting(&conn, CHAT_ROUTING_KEY)?.as_deref(),
            db::get_setting(&conn, LOCAL_CHAT_MODEL_KEY)?.as_deref(),
        );
        let background = role_local_model(
            db::get_setting(&conn, BACKGROUND_ROUTING_KEY)?.as_deref(),
            db::get_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY)?.as_deref(),
        );
        let prefs = llm_gateway::routing_prefs(&conn)?;
        let power_settings = PowerSettings::read(&conn);
        (
            base_url,
            configured,
            chat,
            background,
            prefs,
            power_settings,
        )
    };
    if !configured {
        return Ok(LocalLlmStatus {
            configured: false,
            reachable: false,
            in_cooldown: false,
            cooldown_remaining_s: 0,
            probed_now: false,
            chat_local_model: None,
            background_local_model: None,
            served_window: None,
            served_window_proven: false,
            window_source: None,
            chat_answering: false,
            background_answering: false,
            chat_loaded: None,
            background_loaded: None,
            chat_released: false,
            background_released: false,
            // The keys are read here too — in-memory reads of the secrets cache, after the guard has
            // closed — so `effective`, `cloud_key` and `any_cloud_key` are true for a user with a
            // cloud key who hasn't connected a server yet. Nothing can move: with no endpoint every
            // role's `blocked` stays `NoLocalModel` or `CloudRouting`.
            power: power_view(
                &snap,
                &power_settings,
                keep_local,
                &prefs,
                false,
                chat_bound.as_deref(),
                background_bound.as_deref(),
                llm_gateway::key_presence(Role::Chat),
                llm_gateway::key_presence(Role::Background),
            ),
        });
    }

    // In-memory reads of the secrets cache, after the guard has closed. Only the two REPORTED model
    // fields below change with the route; every internal use keeps the BOUND models, because a
    // parked model can still be loaded, released and probed.
    let power = power_view(
        &snap,
        &power_settings,
        keep_local,
        &prefs,
        base_url.as_deref().is_some_and(|b| !b.trim().is_empty()),
        chat_bound.as_deref(),
        background_bound.as_deref(),
        llm_gateway::key_presence(Role::Chat),
        llm_gateway::key_presence(Role::Background),
    );

    let (in_cooldown, cooldown_remaining_s) = {
        let state = app.state::<AppState>();
        let health = state.local_ai.health();
        let now = std::time::Instant::now();
        (
            health.in_cooldown(now),
            health.cooldown_remaining(now).as_secs(),
        )
    };

    // Debounced reachability probe: only actually hit the server if enough time has passed.
    let probe_now = {
        let state = app.state::<AppState>();
        state.local_ai.probe_debounce_elapsed()
    };
    // The prologue (and with it the call-time posture gate) runs only INSIDE the debounce: the gate
    // may cost a DNS lookup for a hostname endpoint, and this command is polled by the UI far more
    // often than it probes. A refused endpoint reports unreachable rather than erroring — the status
    // chip must keep rendering — and no token is fetched or sent.
    let reachable = if probe_now {
        let observed = match configured_endpoint(&app).await? {
            Endpoint::Ready(base_url, token) => {
                let tok = token.as_ref().map(|s| s.expose());
                let ok = openai_compat::probe(&base_url, tok).await.is_ok();
                // Learn the served window PASSIVELY, on a tick that is already happening.
                //
                // The proven rungs do not care WHO loaded the model, so this picks the number up
                // whenever one is resident for any reason — the user's own `ollama run`, another
                // app, or a previous PM session. That last case is not an edge: this cache lives on
                // `LocalRuntime`, which is rebuilt on every launch, while the server keeps its model
                // loaded across PM restarts. So "PM has never measured this" was the state of every
                // app START, not only of a fresh install, and the only thing that could clear it was
                // a completed local call.
                //
                // Written ONLY when a proven rung answers. Recording PM's own floor here would
                // replace an honest "not measured yet" with a number the panel attributes to the
                // user's server, and would start `window_probe_due` throttling the post-call probe
                // that is this cache's only other writer.
                if !ok {
                    // PM asked and got nothing back, so it no longer knows what this server holds.
                    // Without this the last reading stands for its whole TTL — and stands in the
                    // SAME footer as the "unreachable" line directly beneath it, which is a display
                    // contradicting itself rather than admitting it cannot see.
                    app.state::<AppState>().local_ai.clear_resident(&base_url);
                }
                if ok {
                    let mut models: Vec<&str> =
                        [background_bound.as_deref(), chat_bound.as_deref()]
                            .into_iter()
                            .flatten()
                            .collect();
                    // One role usually, and very often the same model on both.
                    models.dedup();
                    // Nothing bound to either role means there is nothing to ask ABOUT, and both
                    // rungs answer per model: an endpoint connected but not yet assigned — the state
                    // every setup passes through — must not start paying for two requests a tick.
                    //
                    // One pass of the ladder for the ENDPOINT, questioned per model — `/slots`
                    // describes llama-server's single load and `/api/ps` lists everything Ollama
                    // holds, so neither gets more informative by being asked twice. Two roles on
                    // two models used to cost four requests on this tick; they now cost the two a
                    // single role already did.
                    if !models.is_empty() {
                        let probe = openai_compat::probe_live(&base_url, tok).await;
                        let state = app.state::<AppState>();
                        // A server that 404s `/api/ps` has no unload gesture either — the same fact
                        // the release path latches, learned here on a tick that was happening
                        // anyway rather than only after an unload has been attempted and failed.
                        //
                        // Latched HERE and not in the residency command, because here it is
                        // corroborated: `ok` above means `/v1/models` answered, so a server that
                        // then 404s `/api/ps` is genuinely not an Ollama, rather than an
                        // intermediary answering 404 for everything while the real server is
                        // momentarily unrouted. This latch is permanent for the session, so it must
                        // only be set on evidence that cannot be a blip.
                        if probe.no_ollama_api() {
                            state.local_ai.mark_no_unload_route(&base_url);
                        }
                        for model in &models {
                            if let Some(info) = probe.window_for(model) {
                                state.local_ai.cache_window(&base_url, model, info);
                            }
                        }
                        // Learned on the SAME two requests. `/api/ps` answers for the ENDPOINT, so
                        // it answers for every role model or for none — and "for none" clears the
                        // last reading rather than leaving it to age out, because a stale "loaded"
                        // outliving PM's ability to check it is the one answer worse than none.
                        match probe.residency() {
                            Some(resident) => {
                                for model in &models {
                                    let here = openai_compat::model_in(resident, model);
                                    state.local_ai.cache_resident(&base_url, model, here);
                                }
                            }
                            None => state.local_ai.clear_resident(&base_url),
                        }
                    }
                }
                ok
            }
            Endpoint::Refused | Endpoint::Unconfigured => false,
        };
        app.state::<AppState>()
            .local_ai
            .set_last_reachable(observed);
        observed
    } else {
        // No fresh probe this call — report the LAST KNOWN result, which is what this field's own
        // documentation always claimed and the code never did. It used to infer liveness from
        // `!in_cooldown`, and those are not the same thing: a host can fail twice before any
        // cooldown opens. The failure path made that concrete — a failed chat call EMITS the status
        // event, the UI refetches, the 30 s debounce skips the probe, and with one or two strikes
        // there is no cooldown yet, so the chip turned green at the exact moment chat broke. Nothing
        // observed yet reads as unreachable: the chip must never claim health it has not witnessed.
        app.state::<AppState>()
            .local_ai
            .last_reachable()
            .unwrap_or(false)
    };

    // Background first: it is the demanding role — the one sending index-matched arrays over several
    // documents — so when the two roles run different models its window is the one worth reporting.
    // The fallback is on CACHE PRESENCE, not on which role is bound: a background model that has
    // never answered has no cache entry, and reporting "unknown" for it while the CHAT model's
    // window is proven-small hid the very warning the number exists to raise. Whichever role's
    // window is actually known is more honest than none.
    let (served_window, served_window_proven, window_source) = {
        let state = app.state::<AppState>();
        let window_for = |model: Option<&str>| -> Option<openai_compat::WindowInfo> {
            match (base_url.as_deref(), model) {
                (Some(url), Some(m)) => state.local_ai.cached_window(url, m),
                _ => None,
            }
        };
        match window_for(background_bound.as_deref()).or_else(|| window_for(chat_bound.as_deref()))
        {
            Some(w) => (
                Some(w.tokens),
                w.source.is_proven(),
                Some(w.source.as_str().to_string()),
            ),
            None => (None, false, None),
        }
    };

    // The live half. Every one of these is an in-memory read — two atomic loads and two short-lived
    // mutexes — so the footer's cadence costs the user's server nothing beyond the debounced probe
    // above. Deliberately NOT routed through the slot: `LocalSlot`'s guards stamp the quiet clock on
    // the way out, so a status read that took the lane would keep the idle-release timer permanently
    // fresh and the graphics card would never come back.
    let (chat_answering, background_answering, chat_loaded, background_loaded) = {
        let state = app.state::<AppState>();
        let loaded = |model: Option<&str>| -> Option<bool> {
            match (base_url.as_deref(), model) {
                (Some(url), Some(m)) => state.local_ai.cached_resident(url, m),
                _ => None,
            }
        };
        (
            state
                .local_ai
                .slot
                .role_in_flight(crate::local_slot::Lane::Chat)
                > 0,
            state
                .local_ai
                .slot
                .role_in_flight(crate::local_slot::Lane::Background)
                > 0,
            loaded(chat_bound.as_deref()),
            loaded(background_bound.as_deref()),
        )
    };
    let released = |model: Option<&str>| -> bool {
        match (base_url.as_deref(), model) {
            (Some(url), Some(m)) => app.state::<AppState>().local_ai.was_released_by_pm(url, m),
            _ => false,
        }
    };
    Ok(LocalLlmStatus {
        configured: true,
        reachable,
        in_cooldown,
        cooldown_remaining_s,
        probed_now: probe_now,
        chat_answering,
        background_answering,
        chat_loaded,
        background_loaded,
        chat_released: released(chat_bound.as_deref()),
        background_released: released(background_bound.as_deref()),
        chat_local_model: reported_local(chat_bound, power.chat.route),
        background_local_model: reported_local(background_bound, power.background.route),
        served_window,
        served_window_proven,
        window_source,
        power,
    })
}

// ---------------------------------------------------------------------------------------------
// Hardware scan + model recommendations (#296) — the Workbench data layer. Backend-only in PR4
// (no ipc.ts wrapper yet); the Local AI tab consumes these in PR5.
// ---------------------------------------------------------------------------------------------

/// Scan the machine (RAM/CPU/disk/GPU). Cached on the runtime; `force` re-scans. Kept separate from
/// [`local_model_recommendations`] so the (slower) scan caches independently of a recommendations
/// refresh. The probes are blocking, so they run off the async runtime.
#[tauri::command]
pub async fn local_hardware_scan(app: AppHandle, force: bool) -> Result<hardware::Hardware> {
    if !force {
        if let Some(hw) = app.state::<AppState>().local_ai.cached_hardware() {
            return Ok(hw);
        }
    }
    // The Workbench has one "Re-scan" button covering everything it reads about this machine, so a
    // forced scan also drops the on-disk model crawl — otherwise a model downloaded since the tab
    // opened would stay invisible until restart.
    app.state::<AppState>().local_ai.clear_disk_models();
    let hw = scan_hardware(&app).await?;
    Ok(hw)
}

// ---------------------------------------------------------------------------------------------
// "A better-fitting model is available" (#437)
// ---------------------------------------------------------------------------------------------

/// Whether there is a better-fitting local model worth mentioning right now, and what it is.
///
/// Two independent questions, deliberately kept apart: **is it time to look** — the user's rescan
/// cadence ([`local_catalog::rescan_due`]) — and **is there anything worth saying** — the pure
/// comparison in [`better_fit::suggest`]. Both must say yes.
///
/// Cheap enough for the app shell to poll as the user moves around: it reuses the cached hardware
/// scan and on-disk crawl (running each at most once per session), and everything after that is pure.
#[tauri::command]
pub async fn local_better_fit_notice(app: AppHandle) -> Result<Option<better_fit::Suggestion>> {
    let cat = local_catalog::catalog();

    // Is it even time to look? `manual` never fires; the default only fires when a shipped update
    // brought a newer catalog than the one the user last acknowledged.
    let (base_url, chat_model, background_model, due) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let cadence = local_catalog::RescanCadence::from_setting(
            db::get_setting(&conn, local_catalog::RESCAN_CADENCE_KEY)?.as_deref(),
        );
        let seen = db::get_setting(&conn, local_catalog::CATALOG_VERSION_SEEN_KEY)?
            .and_then(|s| s.parse::<u32>().ok());
        let last =
            db::get_setting_time(&conn, local_catalog::LAST_RESCAN_KEY).map(|t| t.timestamp());
        let due = local_catalog::rescan_due(
            cadence,
            seen,
            cat.catalog_version,
            last,
            chrono::Utc::now().timestamp(),
        );
        (
            db::get_setting(&conn, LOCAL_BASE_URL_KEY)?,
            db::get_setting(&conn, LOCAL_CHAT_MODEL_KEY)?,
            db::get_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY)?,
            due,
        )
    };
    if !due || base_url.is_none() {
        return Ok(None);
    }

    let hardware = match app.state::<AppState>().local_ai.cached_hardware() {
        Some(hw) => hw,
        None => scan_hardware(&app).await?,
    };
    let fit_hw = fit::FitHardware {
        // Free RAM re-read LIVE, not taken from the cached scan: it is the one scanned field that
        // moves while the app is open, and a verdict frozen to whatever the machine looked like when
        // the tab was first opened is a verdict about a machine that no longer exists. Everything
        // else here stays cached — the GPU, CPU and disk probes are the expensive half and they do
        // not change mid-session. Falls back to the scanned figure where the platform won't say.
        available_ram_gb: crate::hardware::available_ram_gb().unwrap_or(hardware.available_ram_gb),
        vram_gb: hardware.vram_gb,
        gpu_bandwidth_gbps: hardware.gpu_bandwidth_gbps,
        unified_memory: hardware.unified_memory,
    };

    // Which curated models the user already has a usable copy of (#449) — a suggestion they can act
    // on for free. Derived exactly as PM's pick derives the models you already have
    // ([`size_for_machine`]), so "already on this device" names only a copy the pick could itself
    // choose: one the connected server can serve, with a runnable config of its own.
    //
    // Both sources, because the crawl alone is not enough to answer this: on a packaged Linux
    // install it cannot read Ollama's store, so every model the user has pulled is missing from it.
    // What the endpoint serves is the second source, and for Ollama it IS the store — `/v1/models`
    // lists what has been pulled, not what is resident. Best-effort: the gate, the keychain or the
    // server being unavailable degrades to the crawl's answer rather than failing a passive notice.
    let disk = disk_scan(&app).await;
    let served = match configured_endpoint(&app)
        .await
        .unwrap_or(Endpoint::Unconfigured)
    {
        Endpoint::Ready(url, token) => probe_served(&app, &url, token.as_ref().map(|s| s.expose()))
            .await
            .unwrap_or_default(),
        Endpoint::Refused | Endpoint::Unconfigured => Vec::new(),
    };
    let bound: Vec<String> = [&chat_model, &background_model]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let pick = size_for_machine(&fit_hw, served, &disk.models, base_url.as_deref(), &bound).pick;
    let on_disk = notice_copy(&pick);

    Ok(better_fit_suggestion(
        &fit_hw,
        &on_disk,
        chat_model,
        background_model,
    ))
}

/// The notice's decision once everything it reads has been read. Pure — the catalogue is compiled
/// in — so the catalogue-level tests run exactly what the command does.
fn better_fit_suggestion(
    fit_hw: &fit::FitHardware,
    on_disk: &[String],
    chat_model: Option<String>,
    background_model: Option<String>,
) -> Option<better_fit::Suggestion> {
    let entries: Vec<&local_catalog::CatalogEntry> = local_catalog::catalog()
        .entries
        .iter()
        .filter(|e| e.fit == local_catalog::FitClass::Computed)
        .collect();
    let candidates: Vec<better_fit::Candidate> = entries
        .iter()
        .map(|e| {
            // Judged exactly as PM's pick judges it — verdict and footprint both from the config the
            // pick would run, at the context PM runs it at — so the notice can never suggest a model
            // the pick at the top of the tab would refuse (one that only fits system RAM beside a
            // graphics card, or one too slow for background work), nor turn away the one it chose.
            // The card's own fit, at the model's trained context, was what this read before: gemma 4
            // 12b's 262144-token card was Tight on the dev laptop, so the notice named Qwen3.5 9B
            // directly above a pick card naming gemma 4 12b.
            let option = catalogue_option(e, fit_hw, &local_catalog::entry_to_spec(e));
            let config = option.judged.config.as_ref();
            better_fit::Candidate {
                repo: e.repo.clone(),
                display_name: e.display_name.clone(),
                parameters_b: e.parameters_b,
                verdict: config.map_or(fit::Verdict::Unknown, |c| c.verdict),
                footprint_gb: config.and_then(|c| c.est_memory_gb),
                on_disk: on_disk.iter().any(|r| r == &e.repo),
                pick_eligible: config.is_some() && option.tag.is_some(),
            }
        })
        .collect();

    // The baseline is whatever the user already runs — the BEST of it, so someone with a large chat
    // model isn't nagged about something that only beats their small background one. Judged as the
    // candidates are, so the two sides are compared at the same context: a 131072-token Llama 3.2 3B
    // was a halved context on the dev laptop at 10 GB free, and the notice stayed silent while the
    // pick card offered a 12B.
    let assigned: Vec<better_fit::Candidate> = [chat_model, background_model]
        .into_iter()
        .flatten()
        .filter(|m| !m.trim().is_empty())
        .filter_map(|m| local_catalog::match_installed(&m).map(|e| e.repo.clone()))
        .filter_map(|repo| candidates.iter().find(|c| c.repo == repo).cloned())
        .collect();

    let current = better_fit::baseline(assigned.iter());
    // What a suggestion has to share the machine with: the model on the role `baseline` did NOT
    // pick, which it otherwise drops on the floor. Without this PM can talk someone into a model
    // that fits only if the machine is holding nothing else — manufacturing the very swapping the
    // co-residency line beside it was added to describe.
    //
    // That model is charged at its highest-quality config that fits free memory, at the context PM
    // runs it at — not the smaller one the pick would run, since it is the user's file and not PM's
    // choice. That over-states the model the user already has (the catalogue picks the best quant
    // that fits, not the file they downloaded), which can suppress a suggestion that would in fact
    // have fitted. That is the safe direction for something PM volunteers unprompted.
    let beside = current.and_then(|cur| {
        let other = assigned.iter().find(|c| c.repo != cur.repo)?;
        let e = entries.iter().find(|e| e.repo == other.repo)?;
        let spec = fit::ModelSpec {
            target_context: better_fit::pick_context(e.context_length, None),
            ..local_catalog::entry_to_spec(e)
        };
        fit::fit(&spec, fit_hw)
            .est_memory_gb
            .map(|footprint_gb| better_fit::Beside {
                footprint_gb,
                budget_gb: fit::ram_budget_gb(fit_hw),
            })
    });
    better_fit::suggest(current, &candidates, beside)
}

/// Acknowledge the better-fit notice: record that the user has seen this catalog's evaluation, which
/// silences it until their cadence says to look again (a newer catalog on the default setting, or the
/// next week/month on the timed ones).
///
/// This is the write side of three settings that PR4 defined and read but nothing ever wrote — so
/// `rescan_due` was permanently true for every user. Nothing surfaced it before this card, which is
/// why it was invisible rather than noisy.
#[tauri::command]
pub fn dismiss_local_better_fit(state: State<'_, AppState>) -> Result<()> {
    let conn = state.conn()?;
    db::set_setting(
        &conn,
        local_catalog::CATALOG_VERSION_SEEN_KEY,
        &local_catalog::catalog().catalog_version.to_string(),
    )?;
    db::set_setting(
        &conn,
        local_catalog::LAST_RESCAN_KEY,
        &chrono::Utc::now().to_rfc3339(),
    )?;
    Ok(())
}

/// How often PM re-checks whether a better-fitting model has appeared. `manual` turns the notice off
/// without hiding the control that would bring it back.
#[tauri::command]
pub fn set_local_model_rescan_cadence(state: State<'_, AppState>, cadence: String) -> Result<()> {
    // Round-trip through the enum so an unknown string can't be persisted — it parses to the default.
    let parsed = local_catalog::RescanCadence::from_setting(Some(cadence.as_str()));
    let conn = state.conn()?;
    db::set_setting(
        &conn,
        local_catalog::RESCAN_CADENCE_KEY,
        parsed.as_setting(),
    )?;
    Ok(())
}

/// One model the server currently has loaded.
#[derive(serde::Serialize)]
pub struct ResidentEntry {
    pub model: String,
    /// Total bytes the server placed for it, in GiB.
    pub size_gb: f64,
    /// The share the server reports as being on the GPU, in GiB. A FLOOR: it excludes the CUDA
    /// context and compute buffers, and was measured 1.25 GB low on a real load. Never rendered as
    /// "this is what your card is holding".
    pub size_vram_gb: f64,
    /// PM caused this load, so PM may release it. A model started from a terminal is never PM's.
    pub pm_loaded: bool,
}

/// What is on the graphics card, and what PM is allowed to do about it.
#[derive(serde::Serialize)]
pub struct GpuResidency {
    /// `None` when PM could not ask — no endpoint, unreachable, or a server with no such route.
    /// Emphatically not the same as `Some([])`, which is a server answering that it holds nothing.
    pub resident: Option<Vec<ResidentEntry>>,
    pub vram_gb: Option<f64>,
    /// Connected external displays PM can attribute to a dedicated card. Linux only — neither
    /// Windows nor macOS will say which chip drives an output. Reported, never acted on.
    pub dgpu_displays: Vec<String>,
    pub policy: String,
    pub idle_minutes: u64,
    /// This endpoint answered an unload with "no such route", so the two active policies cannot do
    /// anything here. llama-server holds a model for its whole process life and LM Studio has no
    /// unload gesture; offering a picker that silently does nothing would be worse than saying so.
    pub no_unload_route: bool,
}

/// What the graphics card is holding, for the Local AI tab's lifecycle section.
#[tauri::command]
pub async fn local_gpu_residency(app: AppHandle) -> Result<GpuResidency> {
    let (policy, idle_minutes) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let policy = residency::ReleasePolicy::from_setting(
            db::get_setting(&conn, residency::RELEASE_POLICY_KEY)?.as_deref(),
        );
        let idle = residency::idle_after(
            db::get_setting(&conn, residency::RELEASE_IDLE_MINUTES_KEY)?.as_deref(),
        );
        (policy, idle.as_secs() / 60)
    };
    // ONE endpoint resolution. It costs a DB read, the call-time posture gate (a DNS lookup for a
    // hostname endpoint) and a keychain read, and this command used to do all of it twice — once for
    // the residency read and again, further down, purely to ask which base URL to look the unload
    // latch up under.
    let endpoint = configured_endpoint(&app).await?;
    let (resident, no_unload_route) = match &endpoint {
        Endpoint::Ready(base_url, token) => {
            let answer =
                openai_compat::ollama_ps(base_url, token.as_ref().map(|s| s.expose())).await;
            let state = app.state::<AppState>();
            // Deliberately does NOT latch `no_unload_route` on a 404 here, though it could: this
            // command asks `/api/ps` cold, with nothing corroborating that the server is up at all,
            // and an intermediary can answer 404 for everything while the real server is briefly
            // unrouted (a dropped ngrok tunnel returns exactly that). The latch is permanent for the
            // session, so it is set only where a successful `/v1/models` probe has just proved the
            // server IS answering — in `local_llm_status`, which runs every 30 s while an endpoint
            // is configured, so nothing is lost by waiting for it.
            let rows = answer.models().map(|models| {
                models
                    .into_iter()
                    .map(|m| ResidentEntry {
                        pm_loaded: state.local_ai.is_pm_loaded(base_url, &m.model),
                        model: m.model,
                        size_gb: m.size_gb,
                        size_vram_gb: m.size_vram_gb,
                    })
                    .collect()
            });
            (rows, state.local_ai.has_no_unload_route(base_url))
        }
        Endpoint::Refused | Endpoint::Unconfigured => (None, false),
    };
    let vram_gb = app
        .state::<AppState>()
        .local_ai
        .cached_hardware()
        .and_then(|h| h.vram_gb);
    // Reads every DRM connector's `status` file. Blocking, so it goes where the rest of the blocking
    // hardware probes go — off the async runtime — rather than stalling a worker on a sysfs walk.
    let dgpu_displays = tokio::task::spawn_blocking(hardware::dgpu_displays)
        .await
        .unwrap_or_default();
    Ok(GpuResidency {
        resident,
        vram_gb,
        no_unload_route,
        dgpu_displays,
        policy: policy.as_setting().to_string(),
        idle_minutes,
    })
}

/// Hand the graphics card back now, on the user's say-so.
///
/// Ignores the policy entirely — this is somebody asking, not a timer firing — but keeps every other
/// rule, including proving PM still owns what it is about to free. Returns how many models were
/// CONFIRMED gone, so the UI can say "nothing to release" rather than implying it did something. A
/// request PM could not confirm deliberately does not count.
#[tauri::command]
pub async fn release_local_gpu(app: AppHandle) -> Result<usize> {
    let Endpoint::Ready(base_url, token) = configured_endpoint(&app).await? else {
        return Ok(0);
    };
    let state = app.state::<AppState>();
    let summary = release_pm_models(&state, &base_url, token.as_ref().map(|s| s.expose())).await;
    crate::llm_gateway::ping_status(&app);
    Ok(summary.freed)
}

/// How often to look at whether the card can be handed back.
///
/// Must be comfortably shorter than the shortest quiet period a user can choose
/// ([`residency::MIN_IDLE_MINUTES`] is one minute), or the setting would silently round up.
const RELEASE_TICK: std::time::Duration = std::time::Duration::from_secs(20);

/// Watch for the local slot going quiet, and give the graphics card back when the policy says to.
///
/// A tenth scheduler rather than a passenger on one of the nine that exist, and the reason is a real
/// distinction rather than tidiness: every other loop gates on `state.idle_for()` — how long since
/// the USER did something — while this one gates on how long the SLOT has been quiet. They are
/// different clocks and conflating them is wrong in both directions. Someone reading a long document
/// is "idle" while a background job is mid-generation, and someone typing quickly is "active" while
/// the card has been untouched for an hour.
///
/// It also cannot gate on the vault being unlocked, the way the others do: a model stays resident
/// after the user locks up and walks away, which is precisely when the card is worth handing back.
/// The policy is read whenever it can be and remembered, so a locked vault keeps honouring the last
/// one PM saw.
pub fn spawn_release_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RELEASE_TICK).await;
            release_tick(&app).await;
        }
    });
}

/// One pass of the release scheduler. Split out so the ordering — read, decide, then act — is
/// legible, and so the DB guard demonstrably closes before the first await (rule #4).
async fn release_tick(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Refresh the policy if the vault is open; otherwise fall back to the last one PM could read.
    let fresh = state.conn().ok().map(|conn| {
        let read = |key: &str| db::get_setting(&conn, key).ok().flatten();
        let cfg = residency::ReleaseConfig {
            policy: residency::ReleasePolicy::from_setting(
                read(residency::RELEASE_POLICY_KEY).as_deref(),
            ),
            idle_after: residency::idle_after(read(residency::RELEASE_IDLE_MINUTES_KEY).as_deref()),
            battery_idle_after: residency::battery_idle_after(
                read(residency::BATTERY_IDLE_MINUTES_KEY).as_deref(),
            ),
        };
        (cfg, read(LOCAL_BASE_URL_KEY))
    });
    if let Some((cfg, base_url)) = &fresh {
        state.local_ai.cache_release_policy(*cfg);
        state.local_ai.cache_release_endpoint(base_url.clone());
    }
    // `None` here means PM has never been able to read the policy, which is not the same as "the
    // default policy" — it must not act on a setting it has never seen.
    let Some(cfg) = state.local_ai.cached_release_policy() else {
        return;
    };
    let now = std::time::Instant::now();
    // Every input handed over whole, and the decision made in one place. Nothing is pre-checked
    // here: a caller that filters first and then asks makes the pure reducer's own gates unreachable,
    // which leaves the real decision spread across two files with the tested one contributing
    // nothing. `quiet_for` of `None` means no call has ever run, which zero expresses correctly —
    // zero is never past a quiet period.
    let inputs = residency::ReleaseInputs {
        policy: cfg.policy,
        pm_loaded: !state.local_ai.pm_loaded_pairs().is_empty(),
        in_flight: state.local_ai.slot.in_flight(),
        holds: state.local_ai.slot.holds(),
        quiet_for: state.local_ai.slot.quiet_for(now).unwrap_or_default(),
        idle_after: cfg.idle_after,
        // The SETTLED latch, never a raw reading: a cable wiggle must not release a model.
        on_battery_for: state.local_ai.power_on_battery_for(now),
        battery_idle_after: cfg.battery_idle_after,
    };
    if !residency::should_release(&inputs) {
        return;
    }
    // With the store closed the endpoint can't be read, so PM visits every endpoint it loaded
    // something on. Before #432 this returned here instead, which left the cached policy — kept
    // precisely so a locked vault would keep honouring it — unable to release anything at all.
    let endpoints = residency::release_endpoints(
        fresh.as_ref().map(|(_, b)| b.as_deref()),
        &state.local_ai.pm_loaded_pairs(),
    );
    let token = secrets::get_local_llm_endpoint_token()
        .ok()
        .flatten()
        .map(|s| s.expose().to_string());
    // The token is the configured endpoint's alone. A closed store sends PM to every endpoint it
    // loaded on, and an old one must not be handed the current one's credential.
    let configured = state.local_ai.cached_release_endpoint();
    let mut changed = false;
    for base_url in endpoints {
        // An endpoint that has already told PM it has no unload route never gets asked again.
        // llama-server and LM Studio have none, and neither does a proxy forwarding only `/v1` —
        // without this latch the scheduler posts at one of them every twenty seconds for the life of
        // the process.
        if state.local_ai.has_no_unload_route(&base_url) {
            continue;
        }
        let token = residency::token_for(&base_url, configured.as_deref(), token.as_deref());
        let pass = release_pm_models(&state, &base_url, token).await;
        // Only a pass that changed something the status reports is worth a ping. A marker left on
        // an endpoint the open-store path no longer visits keeps `should_release` true every tick,
        // and pinging for a pass that found nothing to do there would refetch the status every
        // twenty seconds for the rest of the session — on battery, of all times.
        changed |= pass.freed > 0 || pass.unconfirmed > 0 || pass.no_route;
    }
    if changed {
        crate::llm_gateway::ping_status(app);
    }
}

/// The On battery watcher (#432). Its own loop on purpose: `release_tick` can wait on the slot's
/// lane behind a minutes-long stream (`release_pm_models` unloads inside it), and the power poll
/// must not stall with it. Reads FIRST, so an install unplugged at launch has a reading at once. No
/// shutdown path: `process::exit` ends it, like every other scheduler.
pub fn spawn_power_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            power_tick(&app).await;
            tokio::time::sleep(power::POLL).await;
        }
    });
}

/// One watcher sample: read the machine (off the runtime, bounded), read the threshold if the store
/// is open, feed the latch, and ping the status ONLY on a settled change — the Local AI tab refetches
/// on every ping, uncoalesced, so a per-poll ping would be a status refetch every thirty seconds
/// that told it nothing.
async fn power_tick(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let reading = match state.local_ai.begin_power_read() {
        // The previous read is still stuck: an Unknown sample, and no second thread.
        None => PowerReading::default(),
        Some(claim) => {
            let read = tokio::task::spawn_blocking(move || {
                let _claim = claim;
                power_source::read()
            });
            match tokio::time::timeout(power::READ_TIMEOUT, read).await {
                Ok(Ok(r)) => r,
                // A timeout, or a JoinError (the read panicked): Unknown, which never spends money.
                _ => PowerReading::default(),
            }
        }
    };
    // The guard drops at the end of the statement. An Err (the vault is shut, or the read failed)
    // is `None`, which keeps the last threshold the latch knew.
    let threshold = state
        .conn()
        .ok()
        .and_then(|c| db::get_setting(&c, power::THRESHOLD_KEY).ok())
        .map(|v| power::threshold_from(v.as_deref()));
    let now = std::time::Instant::now();
    // The wall clock first: it is the only one that saw a suspend (see `PowerTracker::observe_wall`).
    let woke = state
        .local_ai
        .power_observe_wall(std::time::SystemTime::now(), now);
    if state.local_ai.power_observe(reading, threshold, now) || woke {
        crate::llm_gateway::ping_status(app);
    }
}

/// What one release pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseSummary {
    /// Models confirmed gone.
    pub freed: usize,
    /// Requests that were accepted but never seen to take effect.
    pub unconfirmed: usize,
    /// This endpoint has no unload route, so nothing was attempted after the first answer.
    pub no_route: bool,
}

/// Release every model PM owns on this endpoint, proving ownership first.
///
/// Shared by the timer and the button so the rules live in one place, because two of them are easy
/// to get subtly wrong in two directions:
///
///   * **Ownership is proved, not assumed.** A marker records that PM put a model on the wire, but
///     the model can leave without PM — Ollama evicts under memory pressure, and a user can stop it.
///     A marker left standing across that would have PM claim the NEXT load of that model, which
///     might be the user's own, and unload it out from under their terminal. So a marked model that
///     is no longer resident has its marker retired instead.
///   * **The unload runs inside the lane.** Otherwise a chat call can start while a model is
///     mid-teardown — measured at ~850 ms, during which a request re-attaches to the dying runner and
///     comes back truncated or blank. Truncated scores as a strike, and three strikes cool the
///     endpoint down for chat too.
///
/// Records no health outcome at any point. Housekeeping is not evidence about the endpoint in either
/// direction: a success here would clear a failing host's strike streak, and a failure would eject a
/// healthy one.
async fn release_pm_models(
    state: &State<'_, AppState>,
    base_url: &str,
    token: Option<&str>,
) -> ReleaseSummary {
    let mut summary = ReleaseSummary::default();
    let loaded: Vec<(String, String)> = state
        .local_ai
        .pm_loaded_pairs()
        .into_iter()
        .filter(|(b, _)| b == base_url)
        .collect();
    if loaded.is_empty() {
        return summary;
    }
    // One `/api/ps` read, doing both jobs — and answering in three states, because two of them lead
    // somewhere different. A server with no such route has no `/api/chat` unload either, so PM
    // latches that and stops asking; "could not ask" is not a fact about the server at all, so PM
    // knows nothing and does nothing. Reading either as "nothing is loaded" would have PM act on no
    // evidence, and reading NoRoute as "could not ask" is what left the latch unreachable on exactly
    // the servers it was written for, at twenty seconds a try for the life of the process.
    let resident = match openai_compat::ollama_ps(base_url, token).await {
        openai_compat::PsAnswer::Resident(models) => models,
        openai_compat::PsAnswer::NoRoute => {
            state.local_ai.mark_no_unload_route(base_url);
            summary.no_route = true;
            return summary;
        }
        openai_compat::PsAnswer::Unknown => return summary,
    };
    let is_resident = |model: &str| openai_compat::model_in(&resident, model);

    let mut releasable: Vec<String> = Vec::new();
    for (_, model) in &loaded {
        if is_resident(model) {
            releasable.push(model.clone());
        } else {
            state.local_ai.clear_pm_loaded(base_url, model);
        }
    }
    if releasable.is_empty() {
        return summary;
    }

    let outcomes = state
        .local_ai
        .slot
        .run_exclusive(async {
            let mut out = Vec::new();
            for model in &releasable {
                out.push((
                    model.clone(),
                    openai_compat::unload_model(base_url, model, token, true).await,
                ));
            }
            out
        })
        .await;

    for (model, outcome) in outcomes {
        match outcome {
            openai_compat::UnloadOutcome::Freed => {
                state.local_ai.clear_pm_loaded(base_url, &model);
                // Confirmed gone, and gone because PM asked — the two facts the footer needs to say
                // "released" rather than the bare "not loaded" it would otherwise show for a thing
                // the user's own setting did.
                state.local_ai.cache_resident(base_url, &model, false);
                state.local_ai.mark_released(base_url, &model);
                summary.freed += 1;
            }
            openai_compat::UnloadOutcome::NoRoute => {
                state.local_ai.mark_no_unload_route(base_url);
                summary.no_route = true;
                break;
            }
            // Sent, never seen to take effect. The marker STAYS: PM has not been shown the memory
            // came back, and saying otherwise is the inversion this outcome exists to prevent.
            openai_compat::UnloadOutcome::Unconfirmed => summary.unconfirmed += 1,
        }
    }
    summary
}

/// What one "does this actually work" test found.
///
/// A real completion, because everything PM could already check is metadata: `/v1/models` proves the
/// server answers and lists ids, the disk crawl proves the weights are there, and neither of them
/// has ever asked the pair to produce a token. The setups that fail do so at exactly that step — a
/// model id the server does not recognise, a chat template that returns an empty string, a machine
/// that starts loading and never finishes.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LocalTestResult {
    /// The model that was asked, so a result cannot be read against the wrong row.
    pub model: String,
    /// It answered with something usable — not truncated, not blank.
    pub ok: bool,
    /// What it actually said, trimmed and capped. Evidence rather than a claim: a green tick that
    /// shows nothing is exactly the reassurance this feature exists to stop giving.
    pub reply: Option<String>,
    /// Wall-clock for the whole thing, cold load included.
    pub elapsed_ms: u64,
    /// The test had to load the model, so PM owns that load and the release policy applies to it.
    /// `None` when PM could not tell — the endpoint has no `/api/ps`, or did not answer it.
    pub loaded_for_test: Option<bool>,
    /// OTHER models the server was holding when the test started.
    ///
    /// PM cannot stop a server making room. Ollama's own FAQ says a model that will not fit beside
    /// a loaded one causes the loaded one to be unloaded, and that decision is the server's. So the
    /// honest thing is to say what was there before, and let someone reading a passed test know why
    /// their next chat message might be slow.
    pub was_holding: Vec<String>,
    /// What went wrong, in the user's words. `None` when nothing did.
    pub message: Option<String>,
}

/// The in-flight (or last-finished) test, for a settings view that has just mounted.
///
/// Backend-owned because the tab router unmounts this view on every switch and a test can take
/// minutes. Held in component state alone, a result would be lost by looking at another tab while it
/// ran — and worse, the button would come back enabled while the backend was still refusing a second
/// test, so the only thing a second click could produce was an error.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TestSnapshot {
    /// Which role is being tested: `"chat"` or `"background"`.
    pub role: String,
    /// The model it is asking. The view compares this against what the role is set to NOW, so a
    /// result cannot be shown under a model the user changed to while it was running.
    pub model: String,
    pub running: bool,
    /// The outcome, once there is one. Present with `running: false` is a finished test.
    pub result: Option<LocalTestResult>,
}

/// The in-flight or last-finished test.
#[tauri::command]
pub fn active_local_test(state: State<'_, AppState>) -> Option<TestSnapshot> {
    state.local_ai.active_test()
}

/// The one prompt every test sends. Short, so a cold load dominates the timing rather than the
/// generation, and phrased as an instruction so a reply that ignores it is still a usable reply —
/// this proves the pair can produce tokens, and is emphatically not a quality benchmark.
const TEST_PROMPT: &str = "Reply with just the word: ready";

/// The most reply PM will show back. A model that ignores the instruction and writes an essay is
/// still a pass; it is not a reason to put an essay in a settings panel.
const TEST_REPLY_CAP: usize = 240;

/// Ask the configured model to actually answer something, and report what happened.
///
/// Four rules, each of which the release work (#820) had to learn the hard way:
///
///   * **`/api/ps` is read FIRST, and what it says is reported rather than acted on.** If the model
///     is already resident the test costs nothing and PM says so; if it is not, PM says which other
///     models were there, because loading this one may make the server unload one of them. PM
///     deliberately does not "protect" anything by refusing to test: a server making room is the
///     server's decision, and a diagnostic that declines to run is not a diagnostic.
///   * **Ownership is only claimed for a load this test caused.** Marking a model PM found already
///     resident would have the idle timer unload something the user started in a terminal — the
///     precise consent violation the ownership rule exists to prevent. So the marker is written only
///     when `/api/ps` positively said the model was NOT there.
///   * **It yields to chat.** The lane is taken as background work, so a chat turn arriving mid-test
///     is not made to wait behind it. A preemption is reported as "busy", never as a failure.
///   * **It records no health outcome.** A diagnostic the user ran is not evidence about the server:
///     scoring a pass would clear a real failure streak, and scoring a failure would cool down an
///     endpoint the user is in the middle of debugging. That is what `Neutral` means, and it is why
///     a test is allowed to run during a cooldown at all — it is the natural thing to click when the
///     endpoint is resting, and it cannot make that better or worse.
#[tauri::command]
pub async fn test_local_llm(app: AppHandle, role: String) -> Result<LocalTestResult> {
    let model = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let (routing_key, model_key) = match role.as_str() {
            "chat" => (CHAT_ROUTING_KEY, LOCAL_CHAT_MODEL_KEY),
            "background" => (BACKGROUND_ROUTING_KEY, LOCAL_BACKGROUND_MODEL_KEY),
            other => return Err(Error::Other(format!("unknown role '{other}'"))),
        };
        role_local_model(
            db::get_setting(&conn, routing_key)?.as_deref(),
            db::get_setting(&conn, model_key)?.as_deref(),
        )
    };
    let Some(model) = model else {
        return Err(Error::Other(
            "this role is set to use the cloud, so there is no local model to test".into(),
        ));
    };
    let endpoint = configured_endpoint(&app).await?;
    let (base_url, token) = match endpoint {
        Endpoint::Ready(base_url, token) => (base_url, token),
        Endpoint::Refused => return Err(Error::Other(CALL_TIME_REFUSAL.into())),
        Endpoint::Unconfigured => {
            return Err(Error::Other("no local endpoint is configured".into()))
        }
    };
    let tok = token.as_ref().map(|s| s.expose());

    // Held for the whole command, so a second click while this one is in flight is refused rather
    // than queued behind it in the slot.
    let state = app.state::<AppState>();
    let Some(_test) = state.local_ai.begin_test(&role, &model) else {
        return Err(Error::Other(
            "a test is already running — give it a moment".into(),
        ));
    };

    let answer = openai_compat::ollama_ps(&base_url, tok).await;
    if answer == openai_compat::PsAnswer::NoRoute {
        state.local_ai.mark_no_unload_route(&base_url);
    }
    // `Some(true)` = PM was told it is not there and is about to put it there. `None` = PM could not
    // ask, and both possible answers are wrong to assume: claiming a load it did not cause takes
    // ownership of the user's model, and claiming none leaks memory PM can never free. It says so.
    let (loaded_for_test, was_holding) = match &answer {
        openai_compat::PsAnswer::Resident(models) => {
            // This read is fresher than anything the footer has; keep it, so the line above the
            // button cannot contradict the result printed under it.
            let here = openai_compat::model_in(models, &model);
            state.local_ai.cache_resident(&base_url, &model, here);
            let others = models
                .iter()
                .map(|m| m.model.clone())
                .filter(|m| !m.eq_ignore_ascii_case(&model))
                .collect();
            (Some(!here), others)
        }
        openai_compat::PsAnswer::NoRoute | openai_compat::PsAnswer::Unknown => (None, Vec::new()),
    };
    if loaded_for_test == Some(true) {
        // Before the wire, never on success: a load that then times out has still happened, and the
        // memory it took is exactly what PM must be able to hand back.
        state.local_ai.mark_pm_loaded(&base_url, &model);
    }

    let waiting_since = std::time::Instant::now();
    let messages = vec![crate::openrouter::ChatMessage {
        role: "user".to_string(),
        content: TEST_PROMPT.to_string(),
    }];
    // Timed from INSIDE the lane. The wait for the lane is unbounded — a test clicked mid-reply
    // queues behind the whole of it — and folding that into "answered in Xs" would present someone
    // else's generation as this model's latency, which is the one number the line claims to be.
    let attempt = async {
        let sent = std::time::Instant::now();
        let out = openai_compat::complete_within(
            &base_url,
            &model,
            tok,
            &messages,
            crate::local_slot::tunables::LOCAL_TEST_TOTAL_TIMEOUT,
        )
        .await;
        (sent.elapsed(), out)
    };
    // Background MANNERS, housekeeping IDENTITY. It must yield to chat like background work does —
    // a diagnostic that made someone wait for their reply would be a poor trade — but it is not the
    // Tasks model answering, and counting it as one would have the footer name the wrong role: click
    // Test on the Chat row and the Tasks row would light up.
    let outcome = state
        .local_ai
        .slot
        .run_background(crate::local_slot::Lane::Housekeeping, attempt)
        .await;
    // Every arm, including the ones that never reached the server. A test is never evidence.
    state
        .local_ai
        .record(crate::local_slot::CallOutcome::Neutral);
    // A preemption never reached the wire, so the only honest number is how long PM waited.
    let elapsed_ms = match &outcome {
        crate::local_slot::SlotOutcome::Ran((took, _)) => took.as_millis() as u64,
        crate::local_slot::SlotOutcome::Preempted => waiting_since.elapsed().as_millis() as u64,
    };

    let result = match outcome {
        crate::local_slot::SlotOutcome::Preempted => LocalTestResult {
            model: model.clone(),
            ok: false,
            reply: None,
            elapsed_ms,
            loaded_for_test,
            was_holding: was_holding.clone(),
            message: Some(
                "your model was busy answering a chat message, so the test stood aside. Try it \
                 again in a moment."
                    .into(),
            ),
        },
        crate::local_slot::SlotOutcome::Ran((_, Ok(completion))) => {
            // It produced tokens, so it is on the card whatever `/api/ps` said a moment ago.
            state.local_ai.cache_resident(&base_url, &model, true);
            match completion.usable_text() {
                Some(text) => LocalTestResult {
                    model: model.clone(),
                    ok: true,
                    reply: Some(cap_reply(text)),
                    elapsed_ms,
                    loaded_for_test,
                    was_holding: was_holding.clone(),
                    message: None,
                },
                // A 200 that delivered nothing usable. The gateway demotes this to `Alive` for
                // health; here it is simply a fail, because the question was "does this work".
                None => LocalTestResult {
                    model: model.clone(),
                    ok: false,
                    reply: None,
                    elapsed_ms,
                    loaded_for_test,
                    was_holding: was_holding.clone(),
                    message: Some(format!(
                        "the server answered, but {}.",
                        completion
                            .unusable_reason()
                            .unwrap_or("the reply was not usable")
                    )),
                },
            }
        }
        crate::local_slot::SlotOutcome::Ran((_, Err(failure))) => LocalTestResult {
            model: model.clone(),
            ok: false,
            reply: None,
            elapsed_ms,
            loaded_for_test,
            was_holding: was_holding.clone(),
            message: Some(crate::llm_gateway::local_failure_to_error(&failure).to_string()),
        },
    };
    // A marker written before the wire has to be settled against what actually happened.
    //
    // PM claims ownership BEFORE sending, because a load that then fails has still taken the memory.
    // But two of the arms above mean the model may never have loaded at all: a preemption can happen
    // before the request leaves (the early `chat_waiting` bail sends nothing), and a refused or
    // unreachable endpoint never loaded anything either. A marker left standing over a model that is
    // not there is not merely untidy — the next thing to load that model might be the USER, in a
    // terminal, and PM would then believe it owned their load and unload it under them. So on
    // anything but a proven answer, PM asks once more and keeps the claim only if the model is
    // really there.
    if loaded_for_test == Some(true) && !result.ok {
        match openai_compat::ollama_ps(&base_url, tok).await {
            openai_compat::PsAnswer::Resident(models) => {
                let here = openai_compat::model_in(&models, &model);
                if !here {
                    state.local_ai.clear_pm_loaded(&base_url, &model);
                }
                state.local_ai.cache_resident(&base_url, &model, here);
            }
            // Could not ask, so PM cannot prove it is NOT there. The claim stands: leaking a model
            // PM can free is recoverable, and unloading someone else's is not.
            openai_compat::PsAnswer::NoRoute | openai_compat::PsAnswer::Unknown => {}
        }
    }
    // Recorded where a view that was unmounted when it landed can still find it. The guard marks the
    // job finished on its way out, whatever happened.
    state.local_ai.finish_test(result.clone());
    // The residency and ownership this test may have changed are on the surfaces above the button.
    // Re-read them rather than leave a status line contradicting the result underneath it.
    crate::llm_gateway::ping_status(&app);
    Ok(result)
}

/// Trim a reply to something a settings panel can hold, on a character boundary.
fn cap_reply(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(TEST_REPLY_CAP) {
        Some((byte, _)) => format!("{}...", &trimmed[..byte]),
        None => trimmed.to_string(),
    }
}

/// How long PM may spend giving the graphics card back on the way out.
///
/// `RunEvent::Exit` is a PRE-teardown hook: tao dispatches it and then ends the process with
/// `process::exit`, so nothing after it runs and no destructor ever fires. That makes this the only
/// place a release can happen at shutdown — and it makes it a place that must never hang, because a
/// wedged unload here is an app that will not quit. Measured: the unload itself answers in under a
/// millisecond, so two seconds is generous for a loopback server and short enough that a wrong one
/// costs a blink.
const EXIT_RELEASE_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// Give the graphics card back as PM shuts down, when the user asked for that.
///
/// Deliberately blocking. A spawned task is killed by `process::exit` before its first poll, so this
/// is `block_on` — the first in production, on the main event-loop thread, which is not a runtime
/// worker and so may block safely. Every step is written to give up rather than to fail: an absent
/// state, an unreachable server and a slow one all end the same way, with PM quitting. A locked vault
/// uses the policy the release timer last read, and gives up only if it never read one.
///
/// Never records a health outcome. Housekeeping must not be evidence about the endpoint in either
/// direction, and at shutdown there is nobody left to tell anyway.
pub fn release_gpu_on_exit(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Read every setting up front and drop the guard before the first await — the DB mutex is not
    // reentrant, and this runs while the rest of the app is still alive. A locked vault falls back to
    // what the release timer last read, for the reason that cache exists: quitting with the library
    // locked is still quitting, and the model PM loaded is still on the card.
    let (policy, configured) = match state.conn() {
        Ok(conn) => (
            residency::ReleasePolicy::from_setting(
                db::get_setting(&conn, residency::RELEASE_POLICY_KEY)
                    .ok()
                    .flatten()
                    .as_deref(),
            ),
            db::get_setting(&conn, LOCAL_BASE_URL_KEY).ok().flatten(),
        ),
        Err(_) => match state.local_ai.cached_release_policy() {
            Some(cfg) => (cfg.policy, state.local_ai.cached_release_endpoint()),
            // Never read at all: PM has no policy to honour, so it does nothing.
            None => return,
        },
    };
    // `pm_loaded` is PM's own bookkeeping, rebuilt every launch, so this is empty unless PM itself
    // put a model on the wire during this run. A model the user loaded from a terminal is never in
    // it and is never touched.
    //
    // Deliberately NOT filtered to the currently-configured endpoint. Someone who changes or clears
    // the endpoint and then quits has still left PM's models resident on the old one, and gating the
    // cleanup on the new configuration would leak exactly those.
    let releasable = state.local_ai.pm_loaded_pairs();
    if releasable.is_empty() || !residency::should_release_on_exit(policy, true) {
        return;
    }
    let token = secrets::get_local_llm_endpoint_token()
        .ok()
        .flatten()
        .map(|s| s.expose().to_string());
    tauri::async_runtime::block_on(async move {
        // Fired CONCURRENTLY and without waiting for confirmation, which is the opposite of what
        // every other release path does — and both differences matter here. The process is about to
        // end, so a confirmation has no consumer; and a serial loop that confirms would spend the
        // whole budget on the first model and never send the second one's request at all.
        // The token goes only to the endpoint it was saved for (see `residency::token_for`).
        let requests = releasable.iter().map(|(base_url, model)| {
            let token = residency::token_for(base_url, configured.as_deref(), token.as_deref());
            openai_compat::unload_model(base_url, model, token, false)
        });
        let _ = tokio::time::timeout(
            EXIT_RELEASE_BUDGET,
            futures_util::future::join_all(requests),
        )
        .await;
    });
}

/// The release policy and quiet period, for the Local AI tab's lifecycle section.
#[derive(serde::Serialize)]
pub struct ReleaseSettings {
    /// `"server"` | `"on-exit"` | `"idle"`.
    pub policy: String,
    pub idle_minutes: u64,
    /// The on-battery quiet period (#432), in minutes; 0 = off.
    pub battery_idle_minutes: u64,
}

#[tauri::command]
pub fn get_local_release_policy(state: State<'_, AppState>) -> Result<ReleaseSettings> {
    let conn = state.conn()?;
    let policy = residency::ReleasePolicy::from_setting(
        db::get_setting(&conn, residency::RELEASE_POLICY_KEY)?.as_deref(),
    );
    let idle = residency::idle_after(
        db::get_setting(&conn, residency::RELEASE_IDLE_MINUTES_KEY)?.as_deref(),
    );
    let battery_idle = residency::battery_idle_after(
        db::get_setting(&conn, residency::BATTERY_IDLE_MINUTES_KEY)?.as_deref(),
    );
    Ok(ReleaseSettings {
        policy: policy.as_setting().to_string(),
        idle_minutes: idle.as_secs() / 60,
        battery_idle_minutes: battery_idle.map(|d| d.as_secs() / 60).unwrap_or(0),
    })
}

/// Store the release policy. Round-tripped through the enum and the clamp so an unrecognised policy
/// or an out-of-range period cannot be persisted — both resolve to something PM can act on. Each
/// `None` leaves its stored value alone, so the on-battery row can save without re-sending the
/// policy above it. The release scheduler's cache catches up on its next tick.
#[tauri::command]
pub fn set_local_release_policy(
    state: State<'_, AppState>,
    policy: Option<String>,
    idle_minutes: Option<u64>,
    battery_idle_minutes: Option<u64>,
) -> Result<()> {
    let conn = state.conn()?;
    if let Some(policy) = policy {
        let parsed = residency::ReleasePolicy::from_setting(Some(policy.as_str()));
        db::set_setting(&conn, residency::RELEASE_POLICY_KEY, parsed.as_setting())?;
    }
    if let Some(m) = idle_minutes {
        let clamped = m.clamp(residency::MIN_IDLE_MINUTES, residency::MAX_IDLE_MINUTES);
        db::set_setting(
            &conn,
            residency::RELEASE_IDLE_MINUTES_KEY,
            &clamped.to_string(),
        )?;
    }
    match battery_idle_minutes {
        None => {}
        // Zero is the off switch, so off leaves no row behind rather than a "0" to reinterpret.
        Some(0) => db::delete_setting(&conn, residency::BATTERY_IDLE_MINUTES_KEY)?,
        Some(m) => db::set_setting(
            &conn,
            residency::BATTERY_IDLE_MINUTES_KEY,
            &residency::clamp_battery_idle(m).to_string(),
        )?,
    }
    Ok(())
}

/// Store the On battery policy (#432). Each `None` leaves its stored value alone.
///
/// A lowered threshold, or "never", takes effect at once — the latch may move PM back to local
/// immediately. A raised one reaches the cloud only through the latch's minute-long settle, so a
/// settings change can never send work off the machine by itself.
#[tauri::command]
pub fn set_local_power_policy(
    app: AppHandle,
    threshold: Option<u8>,
    roles: Option<String>,
    consent: Option<String>,
) -> Result<()> {
    let state = app.state::<AppState>();
    let thr = {
        let conn = state.conn()?;
        if let Some(t) = threshold {
            db::set_setting(
                &conn,
                power::THRESHOLD_KEY,
                &t.min(power::MAX_THRESHOLD).to_string(),
            )?;
        }
        if let Some(roles) = roles {
            let scope = power::PowerScope::parse_strict(&roles)
                .ok_or_else(|| Error::Other("unknown On battery role choice".into()))?;
            db::set_setting(&conn, power::SCOPE_KEY, scope.as_setting())?;
        }
        if let Some(request) = consent.as_deref() {
            let stored = db::get_setting(&conn, power::CONSENT_KEY)?;
            match consent_write(stored.as_deref(), request)? {
                Some(scope) => db::set_setting(&conn, power::CONSENT_KEY, scope.as_setting())?,
                None => db::delete_setting(&conn, power::CONSENT_KEY)?,
            }
        }
        power::threshold_from(db::get_setting(&conn, power::THRESHOLD_KEY)?.as_deref())
    };
    state
        .local_ai
        .power_apply_threshold(thr, std::time::Instant::now());
    llm_gateway::ping_status(&app);
    Ok(())
}

/// What a consent request leaves stored: `Some(scope)` to write, `None` to delete the row.
///
/// `"none"` withdraws every yes — "not asked" is the state PM returns to, so the question comes back
/// the next time the battery is low, which is honest, because PM is local again. Anything else names
/// the roles a yes is about (the ones the question named) and is ADDED to any earlier yes: it never
/// covers a role the user wasn't asked about, and never quietly drops one they were. An unknown value
/// is refused rather than read as either.
fn consent_write(stored: Option<&str>, request: &str) -> Result<Option<power::PowerScope>> {
    match request.trim() {
        "none" => Ok(None),
        roles => {
            let scope = power::PowerScope::parse_strict(roles)
                .ok_or_else(|| Error::Other("unknown On battery consent".into()))?;
            Ok(power::Consent::from_setting(stored).with(scope).as_scope())
        }
    }
}

/// "Keep using local until I quit PM" — memory only, never saved. Deliberately not `set_*`: the
/// Settings "Saved ✓" tick announces any `set_` command as persisted, and this must not claim
/// something was saved.
#[tauri::command]
pub fn keep_local_on_battery(app: AppHandle, on: bool) {
    if app.state::<AppState>().local_ai.set_keep_local(on) {
        llm_gateway::ping_status(&app);
    }
}

/// Point the on-disk crawl (#449) at an extra folder, or clear it with `None`. Persisted, and drops
/// the cached crawl so the next recommendations call reflects the change.
#[tauri::command]
pub fn set_local_model_scan_dir(state: State<'_, AppState>, dir: Option<String>) -> Result<()> {
    let conn = state.conn()?;
    match dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => db::set_setting(&conn, LOCAL_MODEL_SCAN_DIR_KEY, d)?,
        None => db::set_setting(&conn, LOCAL_MODEL_SCAN_DIR_KEY, "")?,
    }
    drop(conn);
    state.local_ai.clear_disk_models();
    Ok(())
}

/// Score every curated catalog model — and any model the configured endpoint already serves — against
/// this machine's memory, so the Workbench can recommend what to run. Uses the cached hardware scan
/// (or runs one), never forcing a re-scan.
#[tauri::command]
pub async fn local_model_recommendations(app: AppHandle) -> Result<Recommendations> {
    let hardware = match app.state::<AppState>().local_ai.cached_hardware() {
        Some(hw) => hw,
        None => scan_hardware(&app).await?,
    };
    let fit_hw = fit::FitHardware {
        // Free RAM re-read LIVE, not taken from the cached scan: it is the one scanned field that
        // moves while the app is open, and a verdict frozen to whatever the machine looked like when
        // the tab was first opened is a verdict about a machine that no longer exists. Everything
        // else here stays cached — the GPU, CPU and disk probes are the expensive half and they do
        // not change mid-session. Falls back to the scanned figure where the platform won't say.
        available_ram_gb: crate::hardware::available_ram_gb().unwrap_or(hardware.available_ram_gb),
        vram_gb: hardware.vram_gb,
        gpu_bandwidth_gbps: hardware.gpu_bandwidth_gbps,
        unified_memory: hardware.unified_memory,
    };

    // Models the configured endpoint already serves (best-effort — no endpoint is fine). A REFUSED
    // endpoint degrades the same way an unreachable one already does: the tab still renders its
    // hardware fit, catalog and on-disk models, only the served-models probe is skipped. Failing the
    // whole command because the endpoint's address moved would be a far bigger regression than the
    // missing section.
    let endpoint = configured_endpoint(&app).await?;
    let endpoint_configured = !matches!(endpoint, Endpoint::Unconfigured);
    // Whether the endpoint ANSWERED, which `installed` alone cannot say: an empty list is both "a
    // server with nothing in it" and "no server answered", and the panel below has to tell a
    // first-time installer apart from someone whose address is wrong. `probe` already separates
    // them — a runner with an empty store returns `Ok(vec![])`, which #790 taught `is_models_list`
    // to accept — so this needs no second request.
    let probed = match &endpoint {
        Endpoint::Ready(base_url, token) => {
            probe_served(&app, base_url, token.as_ref().map(|s| s.expose())).await
        }
        Endpoint::Refused | Endpoint::Unconfigured => None,
    };
    let endpoint_answered = probed.is_some();
    let served = probed.unwrap_or_default();

    let disk = disk_scan(&app).await;

    // What the pick needs from settings, on one guard: the endpoint an on-disk model has to belong
    // to before it counts, and the models the two roles are set to, so the one in use comes first.
    let (base_url, bound) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let base_url = db::get_setting(&conn, LOCAL_BASE_URL_KEY)?;
        let mut bound = Vec::new();
        for key in [LOCAL_CHAT_MODEL_KEY, LOCAL_BACKGROUND_MODEL_KEY] {
            bound.extend(db::get_setting(&conn, key)?);
        }
        (base_url, bound)
    };
    let Sizing {
        curated,
        installed,
        on_disk,
        pick,
        ..
    } = size_for_machine(&fit_hw, served, &disk.models, base_url.as_deref(), &bound);

    // Rescan cadence — read-only in PR4 (the Local AI tab sets it and stamps the seen version in PR5).
    let cat = local_catalog::catalog();
    let (cadence, rescan_due) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let cadence = local_catalog::RescanCadence::from_setting(
            db::get_setting(&conn, local_catalog::RESCAN_CADENCE_KEY)?.as_deref(),
        );
        let seen = db::get_setting(&conn, local_catalog::CATALOG_VERSION_SEEN_KEY)?
            .and_then(|s| s.parse::<u32>().ok());
        let last =
            db::get_setting_time(&conn, local_catalog::LAST_RESCAN_KEY).map(|t| t.timestamp());
        let due = local_catalog::rescan_due(
            cadence,
            seen,
            cat.catalog_version,
            last,
            chrono::Utc::now().timestamp(),
        );
        (cadence.as_setting().to_string(), due)
    };

    // Which non-open licences the user has already read. Read here rather than from a second
    // command so the UI can decide whether a row needs the terms dialog without a round trip.
    let terms_accepted = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        accepted_terms(&conn)?
    };

    // Bound before the payload moves `installed`.
    let endpoint_inventory = endpoint_answered.then_some(installed.len());
    let co_residency = assigned_co_residency(&app, &installed, &fit_hw)?;

    Ok(Recommendations {
        hardware,
        reserve_gb: fit::reserve_gb(),
        gpu_reserve_gb: fit::gpu_reserve_gb(),
        catalog_version: cat.catalog_version,
        catalog_generated_at: cat.generated_at.clone(),
        endpoint_configured,
        cadence,
        rescan_due,
        curated,
        installed,
        on_disk,
        disk_sources_present: disk.sources_present.clone(),
        disk_blocked: disk.blocked.clone(),
        // Pre-filter: `on_disk` has already had everything the endpoint serves removed from it, so
        // it cannot answer "is there anything downloaded here at all".
        disk_found: disk.models.len(),
        endpoint_inventory,
        co_residency,
        disk_truncated: disk.truncated,
        scan_dir: scan_dir_setting(&app),
        terms_accepted,
        pick,
        live_available_ram_gb: fit_hw.available_ram_gb,
    })
}

/// What the endpoint serves, with what PM can measure about each model, or `None` when it did not
/// answer — which is not the same thing as a server with nothing in it.
async fn probe_served(
    app: &AppHandle,
    base_url: &str,
    token: Option<&str>,
) -> Option<Vec<ServedProbe>> {
    // The real byte size of every model in the store, and what is loaded right now and where. Both
    // are Ollama's own routes, so neither answers for anything else; those fall back to the
    // catalogue estimate, which is all PM ever had, and to "not known to be loaded". Asked together,
    // so the second read costs no extra wait.
    let (tags, ps) = tokio::join!(
        openai_compat::ollama_tags(base_url, token),
        openai_compat::ollama_ps(base_url, token)
    );
    let resident = ps.models().unwrap_or_default();
    let models = openai_compat::probe(base_url, token).await.ok()?;
    Some(
        models
            .into_iter()
            .map(|id| {
                // The window the server actually loaded it with, when it has been observed. Only a
                // PROVEN reading is used: an unproven one is either PM's own floor or the model's
                // trained capacity, and substituting either for the catalogue's figure would trade
                // one guess for another while looking like a measurement.
                let served_ctx = app
                    .state::<AppState>()
                    .local_ai
                    .cached_window(base_url, &id)
                    .filter(|w| w.source.is_proven())
                    .map(|w| w.tokens);
                let tag = tags
                    .iter()
                    .flatten()
                    .find(|t| t.name.eq_ignore_ascii_case(&id))
                    .cloned();
                let resident = resident
                    .iter()
                    .find(|m| m.model.eq_ignore_ascii_case(&id))
                    .cloned();
                ServedProbe {
                    id,
                    tag,
                    served_ctx,
                    resident,
                }
            })
            .collect(),
    )
}

/// One model the configured endpoint answered with, as the probe saw it.
struct ServedProbe {
    id: String,
    /// Its `/api/tags` row, when the server is an Ollama that listed it.
    tag: Option<openai_compat::OllamaTag>,
    /// The context window it was PROVEN loaded with, when one has been observed.
    served_ctx: Option<u32>,
    /// Its `/api/ps` row, when the server says it is loaded right now. `None` is "not known to be
    /// loaded": not loaded, or a server PM could not ask.
    resident: Option<openai_compat::ResidentModel>,
}

/// What [`local_model_recommendations`] works out about this machine once every read is done.
struct Sizing {
    curated: Vec<Recommendation>,
    installed: Vec<InstalledModel>,
    on_disk: Vec<OnDiskModel>,
    /// The models the user already has that the pick weighed: served, or on disk for a server that
    /// could serve them. Only the tests read them now — the better-fit notice takes its one copy from
    /// the pick itself ([`notice_copy`]).
    #[cfg(test)]
    owned: Vec<better_fit::OwnedOption>,
    pick: better_fit::Pick,
}

/// Size the curated list, the served models and the on-disk models against this machine, and make
/// PM's pick from them. Pure — every input has already been read, and the catalogue is compiled in —
/// so the catalogue-level tests run the command's own composition rather than a copy of it.
///
/// `base_url` is the stored endpoint (`None` when there is none) and `bound` the models the two
/// roles are set to, both exactly as stored.
fn size_for_machine(
    fit_hw: &fit::FitHardware,
    served: Vec<ServedProbe>,
    disk: &[local_disk::DiskModel],
    base_url: Option<&str>,
    bound: &[String],
) -> Sizing {
    // Score the curated catalog.
    let cat = local_catalog::catalog();
    let mut options = Vec::new();
    let mut curated: Vec<Recommendation> = Vec::with_capacity(cat.entries.len());
    for e in &cat.entries {
        // Honour the generator's judgment: an entry it marked fit-unknown is never silently
        // scored. When it is scored, also derive the faster GPU-resident config (if any).
        let (fit, gpu) = match e.fit {
            local_catalog::FitClass::Unknown => (
                fit::unknown("PM can't estimate this model's fit.".to_string()),
                fit::GpuFit::Single,
            ),
            local_catalog::FitClass::Computed => {
                let spec = local_catalog::entry_to_spec(e);
                let fit = fit::fit(&spec, fit_hw);
                let gpu = fit::gpu_fit(&spec, fit_hw, &fit);
                // What the pick weighs, from the same spec as the card. It changes nothing on the
                // card: the pick never filters, reorders or re-badges this list.
                options.push(catalogue_option(e, fit_hw, &spec));
                (fit, gpu)
            }
        };
        let (ollama_pull, sharded_quant) = pull_target_for(e, fit.quant);
        // The SECOND rung's download. `gpu_fit` was handed the same `entry_to_spec(e)` spec, so
        // its quant is one of this entry's own rows by construction and `pull_target_for` maps it
        // back exactly — the same round-trip the RAM rung relies on. Computed here rather than
        // inside `fit::GpuFit` on purpose: `fit.rs` is a pure module with no catalogue concepts,
        // and thirteen `gpu_fit` tests construct that enum.
        let gpu_pull = gpu_pull_target(e, &gpu, fit.quant);
        curated.push(Recommendation {
            repo: e.repo.clone(),
            display_name: e.display_name.clone(),
            architecture: e.architecture.clone(),
            role_hint: e.role_hint.clone(),
            parameters_b: e.parameters_b,
            active_parameters_b: e.active_parameters_b,
            context_length: e.context_length,
            multimodal: e.multimodal,
            reasoning: e.reasoning,
            // Resolved from the quant the FIT actually picked, not from the entry: the card's
            // memory verdict is about one specific quantization, so offering a download for a
            // different one would make that verdict describe a file the button never fetches.
            // Compared through `from_label` — the same function `entry_to_spec` used to build
            // the candidate list — so the round-trip is exact by construction.
            ollama_pull,
            sharded_quant,
            gpu_pull,
            licence: e.licence.clone(),
            fit,
            gpu,
        });
    }
    curated.sort_by(|a, b| {
        verdict_rank(a.fit.verdict)
            .cmp(&verdict_rank(b.fit.verdict))
            .then(
                b.parameters_b
                    .partial_cmp(&a.parameters_b)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let mut owned = Vec::new();
    let mut installed = Vec::with_capacity(served.len());
    for probe in served {
        // The parameter count `/api/tags` reports is what lets an Ollama library tag such as
        // `qwen2.5:latest` — a family with no size in its name — match a catalogue entry at all.
        let entry = local_catalog::match_served(
            &probe.id,
            probe.tag.as_ref().and_then(|t| t.parameter_size_b),
        );
        let sized = served_spec(entry, probe.tag.as_ref(), probe.served_ctx);
        let fit = sized
            .as_ref()
            .map_or_else(not_in_catalog, |(spec, _)| fit::fit(spec, fit_hw));
        if let (Some(entry), Some(sized)) = (entry, &sized) {
            owned.push(served_option(
                &probe.id,
                entry,
                sized,
                probe.served_ctx,
                fit_hw,
                &options,
                is_bound(&probe.id, bound),
            ));
        }
        let measured = sized.as_ref().is_some_and(|(_, m)| *m) && probe.served_ctx.is_some();
        // `fit_hw` is THIS computer's card, so a server elsewhere says nothing about it: its
        // `/api/ps` describes its own machine, and its files are loaded onto its own memory.
        let here = base_url.is_some_and(openai_compat::host_is_loopback_literal);
        let spills_gpu = here
            && spills_gpu(
                fit_hw,
                probe.resident.as_ref(),
                sized.as_ref().map(|(spec, _)| spec).filter(|_| measured),
            );
        let card_unused = here && card_unused(fit_hw, probe.resident.as_ref());
        installed.push(InstalledModel {
            id: probe.id,
            matched_repo: entry.map(|e| e.repo.clone()),
            fit,
            measured,
            spills_gpu,
            card_unused,
        });
    }

    // Models sitting on disk that no endpoint currently serves (#449). Scored on their REAL on-disk
    // size rather than the catalog's figure for that quant — the point of the card is to describe the
    // file you actually have. De-duplicated against the served list so a model that is both
    // downloaded and loaded appears once, under the endpoint that serves it.
    let served_keys = served_keys(&installed);
    let mut on_disk = Vec::new();
    for m in disk.iter().filter(|m| !already_served(m, &served_keys)) {
        let matched = local_catalog::match_installed(&m.name);
        let spec = on_disk_spec(m, matched);
        let fit = match &spec {
            Ok(spec) => fit::fit(spec, fit_hw),
            Err(unknown) => unknown.clone(),
        };
        // Only a file the connected server could actually serve counts towards the pick: an LM
        // Studio download is no use to an Ollama on 11434, however well it would fit.
        if let (Some(entry), Ok(spec)) = (matched, &spec) {
            if runner_can_serve(m.source, base_url) {
                owned.push(disk_option(
                    m,
                    entry,
                    spec,
                    fit_hw,
                    is_bound(&m.name, bound),
                ));
            }
        }
        on_disk.push(OnDiskModel {
            name: m.name.clone(),
            source: m.source,
            path: m.path.clone(),
            size_gb: m.size_gb,
            sidecar_gb: m.sidecar_gb,
            quant: m.quant.clone(),
            shards: m.shards,
            matched_repo: matched.map(|e| e.repo.clone()),
            fit,
        });
    }

    let pick = better_fit::pick(better_fit::basis_for(fit_hw), &options, &owned);
    Sizing {
        curated,
        installed,
        on_disk,
        #[cfg(test)]
        owned,
        pick,
    }
}

/// The share of a load that must sit off the card before `/api/ps` counts as showing a spill. A load
/// wholly on the card reports `size_vram` equal to `size` to the byte (30-08-2026), so anything past
/// rounding is a real offload; 5% keeps a sliver of one from being called "runs from system memory".
const RESIDENT_SPILL_SHARE: f64 = 0.05;

/// Whether PM can show that a served model does not fit this machine's graphics card, so runs (at
/// least partly) from system memory — the one ground on which the tab may say so. The caller asks
/// only for a server on this computer.
///
/// The server's word covers more than a model that is too big: one sharing the card with another
/// program offloads part of a model that would fit alone. Both run partly from system memory, which
/// is what the tab says, not that the model is larger than the card. A server that puts NOTHING on
/// the card is a different fact, with different advice ([`card_unused`]), and is left out here.
///
/// Only with a dedicated card: unified memory has no separate card to spill off, and with no card
/// there is nothing to compare against. Then, in order:
///
///   * **Loaded right now** (`resident`): the server's own word decides. Its `size_vram` is a FLOOR
///     — it leaves out the CUDA context and compute buffers — so it can prove a spill, when it is
///     clearly short of `size`, and never a fit; a load it reports wholly on the card is not
///     second-guessed from an estimate.
///   * **Not known to be loaded**: only from `measured`, the spec of the user's own file at the
///     window the server proved it serves (`None` when either is a guess). Even then it must outgrow
///     the card at its gentlest, with a q8_0 cache and past the estimate's error band
///     ([`fit::outgrows_card`]): the row's own fit takes an f16 cache whenever free RAM allows, and
///     PM cannot read which one the server runs, so that fit alone put a model "off the card" on an
///     Ollama where it sat on it.
fn spills_gpu(
    hw: &fit::FitHardware,
    resident: Option<&openai_compat::ResidentModel>,
    measured: Option<&fit::ModelSpec>,
) -> bool {
    if better_fit::basis_for(hw) != better_fit::PickBasis::Gpu {
        return false;
    }
    let Some(vram) = hw.vram_gb else {
        return false;
    };
    match resident {
        Some(r) => {
            r.size_gb > 0.0
                && r.size_vram_gb > 0.0
                && r.size_vram_gb < r.size_gb * (1.0 - RESIDENT_SPILL_SHARE)
        }
        None => measured.is_some_and(|spec| fit::outgrows_card(spec, spec.target_context, vram)),
    }
}

/// Whether the server loaded a model with none of it on this machine's dedicated graphics card: a
/// container started without the card, a card its runtime doesn't support, a CPU-only build. Then
/// every model it runs is in system memory, PM's pick included, so the tab says the server isn't
/// using the card rather than pointing at a smaller or a different model.
fn card_unused(hw: &fit::FitHardware, resident: Option<&openai_compat::ResidentModel>) -> bool {
    better_fit::basis_for(hw) == better_fit::PickBasis::Gpu
        && resident.is_some_and(|r| r.size_gb > 0.0 && r.size_vram_gb <= 0.0)
}

/// The one copy the better-fit notice may call "already on this device": the copy PM's pick itself
/// names, or none. Taken from the pick rather than worked out again, so the two cannot disagree.
///
/// Every copy the pick could use was the earlier rule, and with two of them the notice named the
/// larger while the pick, which prefers one a role uses and then one the server already serves,
/// named the other — directly under it. Matching the disk by repo alone, before that, counted an LM
/// Studio Q8_0 under an Ollama on 11434: a file that server cannot load.
fn notice_copy(pick: &better_fit::Pick) -> Vec<String> {
    match pick {
        better_fit::Pick::Owned { repo, .. } => vec![repo.clone()],
        better_fit::Pick::Catalogue { .. } | better_fit::Pick::Nothing { .. } => Vec::new(),
    }
}

/// The pick's view of one curated model. Shared by the recommendations and the better-fit notice, so
/// the two can never judge the same model differently.
fn catalogue_option(
    e: &local_catalog::CatalogEntry,
    hw: &fit::FitHardware,
    spec: &fit::ModelSpec,
) -> better_fit::CatalogueOption {
    // The pick names a download, so it is judged among the quants Ollama can fetch. Judging every
    // quant and only then refusing a config whose quant has no tag turned a model away for having a
    // larger, untagged file that fits — Qwen2.5 72B on a card or a machine big enough for its Q5_K_M
    // — while its tagged Q4_K_M fitted too. `judge` knows no catalogue, so this is done here, and it
    // makes `ram_runnable` and `system_ok` fetchable by construction as well.
    let fetchable = fit::ModelSpec {
        candidates: spec
            .candidates
            .iter()
            .copied()
            .filter(|c| pull_target_for(e, Some(c.quant)).0.is_some())
            .collect(),
        ..spec.clone()
    };
    let context = better_fit::pick_context(e.context_length, None);
    let judged = better_fit::judge(&fetchable, hw, context);
    let config_quant = judged.config.as_ref().and_then(|c| c.quant);
    let tag = pull_target_for(e, config_quant).0;
    // What `ollama pull` fetches for that tag: the weights and, for a multimodal model, the
    // projector layer that comes with them.
    let download_gb = quant_row(e, config_quant).map(|q| q.file_gb + e.projector_gb.unwrap_or(0.0));
    let system_ok = better_fit::system_config(&fetchable, hw, context).is_some();
    better_fit::CatalogueOption {
        repo: e.repo.clone(),
        display_name: e.display_name.clone(),
        parameters_b: e.parameters_b,
        judged,
        tag,
        download_gb,
        system_ok,
    }
}

/// The pick's view of a model the endpoint serves, judged at the context it will run at
/// ([`better_fit::pick_context`], raised to the window its server was seen loading it with).
///
/// When the served id IS a curated card's pull tag, it is the very file PM sized for that card, so
/// the card's judgement stands — measured, because PM knows exactly which file it is. Judged on its
/// own `/api/tags` size it now comes out the same, since that size is read in the catalogue's own
/// GiB; this keeps the two from differing by a rounding at a budget's edge.
///
/// When PM could NOT measure it — LM Studio and llama-server have no `/api/tags`, so neither its
/// size nor its quant is known — it is judged on the heaviest build the catalogue lists, never the
/// best one that fits. Judged the generous way, a server holding the Q8_0 was told its model "fits
/// entirely on your graphics card" from the Q5_K_M's figures, and the download that would actually
/// have fixed it was hidden. The pessimistic way, an unmeasured copy wins only if every build of it
/// would fit; otherwise the catalogue pick and its "get this quant" step stand.
fn served_option(
    id: &str,
    entry: &local_catalog::CatalogEntry,
    (spec, measured): &(fit::ModelSpec, bool),
    served_ctx: Option<u32>,
    hw: &fit::FitHardware,
    options: &[better_fit::CatalogueOption],
    bound: bool,
) -> better_fit::OwnedOption {
    let context = better_fit::pick_context(entry.context_length, served_ctx);
    // The catalogue judged its own file at the default pick context; a server proven to run it with a
    // longer window holds more than that, so it is judged afresh below.
    let same_file = (context == better_fit::pick_context(entry.context_length, None))
        .then(|| {
            options.iter().find(|o| {
                o.judged.config.is_some()
                    && o.tag.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(id))
            })
        })
        .flatten();
    let (config, measured) = match same_file {
        Some(o) => (o.judged.config.clone(), true),
        None if !*measured => (
            better_fit::judge(&heaviest(spec), hw, context).config,
            false,
        ),
        None => (better_fit::judge(spec, hw, context).config, true),
    };
    better_fit::OwnedOption {
        id: id.to_string(),
        repo: entry.repo.clone(),
        display_name: entry.display_name.clone(),
        parameters_b: entry.parameters_b,
        served: true,
        source: None,
        path: None,
        // A server answers for one loaded model, whatever it was built from.
        shards: 1,
        measured,
        config,
        bound,
    }
}

/// `spec` cut down to its heaviest candidate: the build to assume when PM cannot tell which one a
/// server loaded.
fn heaviest(spec: &fit::ModelSpec) -> fit::ModelSpec {
    fit::ModelSpec {
        candidates: spec
            .candidates
            .iter()
            .copied()
            .max_by(|a, b| a.weight_gb.total_cmp(&b.weight_gb))
            .into_iter()
            .collect(),
        ..spec.clone()
    }
}

/// The pick's view of a model found on disk. Always measured: `on_disk_spec` only succeeds when the
/// file's quant is known, and its size is read off the disk.
fn disk_option(
    m: &local_disk::DiskModel,
    entry: &local_catalog::CatalogEntry,
    spec: &fit::ModelSpec,
    hw: &fit::FitHardware,
    bound: bool,
) -> better_fit::OwnedOption {
    better_fit::OwnedOption {
        id: m.name.clone(),
        repo: entry.repo.clone(),
        display_name: entry.display_name.clone(),
        parameters_b: entry.parameters_b,
        served: false,
        source: Some(m.source),
        path: Some(m.path.clone()),
        shards: m.shards,
        measured: true,
        config: better_fit::judge(
            spec,
            hw,
            better_fit::pick_context(entry.context_length, None),
        )
        .config,
        bound,
    }
}

/// Whether a role is set to this model, as stored (compared case-insensitively, like every other id
/// comparison against the server's own listing).
fn is_bound(id: &str, bound: &[String]) -> bool {
    bound.iter().any(|b| b.trim().eq_ignore_ascii_case(id))
}

/// Whether the configured endpoint could serve a model found in this runner's folder, judged by the
/// endpoint's host and port. With no endpoint configured, every runner counts: the user has not
/// chosen one yet.
///
/// The host has to be this machine — `localhost` or a loopback address, the set the posture check
/// calls `Loopback` and the frontend's `isLoopback` agrees with. A file on this disk is no use to an
/// Ollama on another computer however its port reads; counting it, by port alone, made a local
/// `qwen2.5:7b` the pick for a LAN server that would never list it, and told the user it would
/// "show up by itself" once Ollama was connected — which it already was, to a different machine.
/// The cost is a server reached through this machine's own LAN address no longer counting its own
/// files, and for Ollama that costs nothing: what it serves already covers its own store.
///
/// Then the port, against [`KNOWN_PORTS`]. A port PM does not recognise counts none, since PM
/// cannot say what it is talking to.
fn runner_can_serve(source: local_disk::DiskSource, base_url: Option<&str>) -> bool {
    use local_disk::DiskSource;
    let Some(url) = base_url.map(str::trim).filter(|u| !u.is_empty()) else {
        return true;
    };
    let Ok((_, host, port)) = split_scheme_host_port(url) else {
        return false;
    };
    let on_this_machine = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| classify_ip(ip) == EndpointClass::Loopback);
    if !on_this_machine {
        return false;
    }
    match KNOWN_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, l)| *l)
    {
        Some("Ollama") => source == DiskSource::Ollama,
        Some("LM Studio") => source == DiskSource::LmStudio,
        Some("llama-server") => matches!(source, DiskSource::HuggingFace | DiskSource::Folder),
        _ => false,
    }
}

/// Weigh the two models the roles are actually bound to against this one machine (#786 item 6).
///
/// `None` — not a verdict, an absence of a question — whenever there is only one model in play: a
/// role on cloud, a role with nothing picked, or the same model on both. One server holding one
/// model costs exactly what the Workbench already said it would, and a co-residency line there would
/// be noise on the commonest setup of all.
///
/// Scored from `installed`, so the sum is of the very numbers those cards displayed and cannot
/// contradict them. A model the endpoint serves but PM could not size carries `Verdict::Unknown`
/// through to an `Unknown` verdict rather than a sum with a hole in it.
fn assigned_co_residency(
    app: &AppHandle,
    installed: &[InstalledModel],
    fit_hw: &fit::FitHardware,
) -> Result<Option<fit::CoResidencyFit>> {
    let (chat, background) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        (
            role_local_model(
                db::get_setting(&conn, CHAT_ROUTING_KEY)?.as_deref(),
                db::get_setting(&conn, LOCAL_CHAT_MODEL_KEY)?.as_deref(),
            ),
            role_local_model(
                db::get_setting(&conn, BACKGROUND_ROUTING_KEY)?.as_deref(),
                db::get_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY)?.as_deref(),
            ),
        )
    };
    Ok(co_residency_for_roles(
        chat.as_deref(),
        background.as_deref(),
        installed,
        fit_hw,
    ))
}

/// The co-residency verdict for two role bindings, or `None` when there is no question to ask.
///
/// Pure, and split out from the settings read for exactly that reason: every "no question" case is a
/// decision worth pinning, and each of them is a case where a warning would be actively wrong rather
/// than merely unhelpful.
fn co_residency_for_roles(
    chat: Option<&str>,
    background: Option<&str>,
    installed: &[InstalledModel],
    fit_hw: &fit::FitHardware,
) -> Option<fit::CoResidencyFit> {
    let (chat, background) = (chat?, background?);
    if chat.eq_ignore_ascii_case(background) {
        return None;
    }
    let find = |id: &str| installed.iter().find(|m| m.id.eq_ignore_ascii_case(id));
    // A role bound to something the endpoint is not serving is already its own visible problem — the
    // model cannot answer at all — and inventing a memory verdict about a model that is not there
    // would be a second, quieter wrong answer on top of it.
    let (a, b) = (find(chat)?, find(background)?);
    Some(fit::co_residency(&a.fit, &b.fit, fit_hw))
}

/// The licence ids the user has accepted, as stored. Empty when the setting has never been written.
///
/// Stored comma-separated because the ids are a closed, slug-shaped set from the catalogue's own
/// ledger (`apache-2.0`, `gemma`, `llama3.2`, …) — no separator can appear inside one.
fn accepted_terms(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    Ok(db::get_setting(conn, local_catalog::TERMS_ACCEPTED_KEY)?
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// Record that the user has read a licence's terms. Additive and idempotent: accepting a licence
/// that is already recorded rewrites the same set.
///
/// This is DISCLOSURE, not a permission system. PM downloads no weights — `pull_local_model` asks
/// the user's own Ollama to fetch them, and the user can run `ollama pull` without PM at all. What
/// this records is that PM showed the terms and the user said they had read them.
#[tauri::command]
pub async fn accept_local_model_terms(app: AppHandle, licence_id: String) -> Result<Vec<String>> {
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let mut accepted = accepted_terms(&conn)?;
    if !accepted.iter().any(|a| a == &licence_id) {
        accepted.push(licence_id);
        accepted.sort();
        db::set_setting(
            &conn,
            local_catalog::TERMS_ACCEPTED_KEY,
            &accepted.join(","),
        )?;
    }
    Ok(accepted)
}

/// The extra crawl folder as stored, or `None` when unset (an empty string is how clearing it is
/// recorded, since settings are additive).
fn scan_dir_setting(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let conn = state.conn().ok()?;
    db::get_setting(&conn, LOCAL_MODEL_SCAN_DIR_KEY)
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The on-disk crawl (#449), cached on the runtime like the hardware scan. Failure is not an error:
/// an unreadable home directory yields an empty scan and the Workbench simply shows nothing for it.
async fn disk_scan(app: &AppHandle) -> local_disk::DiskScan {
    if let Some(cached) = app.state::<AppState>().local_ai.cached_disk_models() {
        return cached;
    }
    let home = app.path().home_dir().ok();
    // Read the setting BEFORE the await — the DB guard must never be held across one.
    let extra = scan_dir_setting(app).map(std::path::PathBuf::from);
    let Some(home) = home else {
        return local_disk::DiskScan::default();
    };
    let scan =
        tauri::async_runtime::spawn_blocking(move || local_disk::scan(&home, extra.as_deref()))
            .await
            .unwrap_or_default();
    app.state::<AppState>()
        .local_ai
        .cache_disk_models(scan.clone());
    scan
}

/// The keys an on-disk model is de-duplicated against: every served model's own id AND the catalogue
/// repo it matched.
///
/// Both, because they answer for different on-disk names. Keying a matched model on its repo alone
/// was enough while a served Ollama library tag matched nothing; now `match_served` resolves
/// `qwen2.5:latest` to its repo, while the on-disk Ollama copy of the very same name still matches
/// nothing (there is no size to go on), and only the id can still recognise it as the same model.
fn served_keys(installed: &[InstalledModel]) -> Vec<String> {
    installed
        .iter()
        .flat_map(|m| std::iter::once(m.id.clone()).chain(m.matched_repo.clone()))
        .collect()
}

/// Whether an on-disk model is the same thing the endpoint already serves. Compared on the catalog
/// repo when both matched, else on the runner's own name.
///
/// The name fallback is strict EQUALITY, deliberately, and it carries the whole weight only for a
/// model outside the catalogue. It round-trips exactly for Ollama — `ollama_display_name` rebuilds
/// the same string `/v1/models` reports — and is unreliable for a file-based runner, whose
/// `owner/repo/file.gguf` merely *contains* the served id rather than equalling it. Loosening it to
/// a substring test would dedupe more, at the cost of silently merging two genuinely different
/// files whose names nest; a missed dedupe shows the model twice and is self-correcting, a wrong one
/// hides it. Left strict on purpose.
fn already_served(model: &local_disk::DiskModel, served_keys: &[String]) -> bool {
    let matched = local_catalog::match_installed(&model.name).map(|e| e.repo.as_str());
    served_keys.iter().any(|key| {
        if let Some(repo) = matched {
            if key.eq_ignore_ascii_case(repo) {
                return true;
            }
        }
        key.eq_ignore_ascii_case(&model.name)
    })
}

/// Said of a served or on-disk model that matches no catalogue entry. Shared so the two paths can't
/// drift apart.
fn not_in_catalog() -> fit::FitResult {
    fit::unknown("This model isn't in PM's catalog, so its fit can't be estimated.".to_string())
}

/// Said when PM has no usable quantization label at all — as opposed to having one it can't size,
/// which names the label instead. Shared so the two paths can't drift apart.
const UNREADABLE_QUANT: &str =
    "PM couldn't tell which quantization this file is, so its fit can't be estimated.";

/// A quant label is only ever as trustworthy as where it came from. A filename label is gated on
/// `Quant::from_label` before it gets this far, but Ollama's comes from a `file_type` field inside a
/// config blob — file content, so untrusted. Bound the length and drop anything that isn't
/// label-shaped before it reaches the UI. `None` when nothing usable survives, so the caller falls
/// back to the generic wording rather than printing an empty gap.
fn safe_quant_label(label: &str) -> Option<String> {
    let cleaned: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(24)
        .collect();
    (!cleaned.is_empty()).then(|| cleaned.to_ascii_uppercase())
}

/// The spec to score a model the endpoint is actually SERVING with, using what the server will tell
/// us about it, and whether it was measured (the file's real size and quant replaced the catalogue's
/// candidates). `None` when it matched no catalogue entry. `fit::fit` of the spec is the served
/// card's fit; PM's pick judges the same spec.
///
/// The difference from `fit::fit` on the catalogue entry is the whole point, and it is not a
/// refinement. `fit` picks the best quantization that FITS THE BUDGET — right advice for a model you
/// have not downloaded, and fiction for one you already have. Measured 30-08-2026: PM scored a served
/// Qwen2.5-7B as Q8_0 at 10.04 GB against the user's real Q5_K_M file of 5.44 GB, and a served
/// gemma-3-4b at 9.21 GB against a real 3.34 GB. The error also moved the wrong way — a bigger budget
/// lets `fit` reach a higher quant, so FREEING memory made PM's estimate of an unchanged file grow.
///
/// Two measurements replace two guesses whenever the server supplies them:
///
///   * **The file's real byte size**, from `/api/tags`. That figure is the manifest total — weights
///     plus any projector — so it goes into the weight term with the projector explicitly zeroed,
///     never added to a separate projector figure. Getting that backwards is the double-count #588
///     fixed.
///   * **The context the server actually loaded it with**, instead of the model's TRAINED capacity.
///     Not a nicety: gemma-3-4b trains at 131072, so the catalogue's KV term for it was 4.07 GB
///     under the old parameter-count proxy — 44% of the entire estimate — for a window the server
///     was never serving. #792 already ruled
///     that number unusable for the context meter, and it was still driving the memory estimate.
///
/// Falls back to the catalogue spec whenever either measurement is missing, which is exactly the
/// behaviour that shipped before this — never worse, and better wherever the server answers.
fn served_spec(
    entry: Option<&local_catalog::CatalogEntry>,
    tag: Option<&openai_compat::OllamaTag>,
    served_ctx: Option<u32>,
) -> Option<(fit::ModelSpec, bool)> {
    let entry = entry?;
    let mut spec = local_catalog::entry_to_spec(entry);
    if let Some(ctx) = served_ctx {
        spec.target_context = ctx;
    }
    let mut measured = false;
    // Both, or neither. A measured size with no quantization label cannot be made into a candidate —
    // `bytes_per_param` drives the throughput term — and pinning the weight while inventing a quant
    // would put a made-up number beside a measured one.
    if let (Some(tag), Some(quant)) = (tag, tag.and_then(served_quant)) {
        if tag.size_bytes > 0 {
            spec.candidates = vec![fit::QuantCandidate {
                quant,
                weight_gb: bytes_to_gb(tag.size_bytes),
            }];
            // The tag's size is the manifest TOTAL, so the projector is already inside the weight
            // term. `Some(0.0)` is a measurement here ("nothing further to add"), not a gap.
            spec.projector_gb = Some(0.0);
            measured = true;
        }
    }
    Some((spec, measured))
}

/// The quantization of a served model: what the server says, else what its own tag says.
///
/// Ollama reports `"unknown"` for some repos even when the tag it was pulled under names the quant
/// outright (`hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M`), so the tag suffix is a real second source
/// rather than a guess — it is the string the user typed to fetch that exact file.
fn served_quant(tag: &openai_compat::OllamaTag) -> Option<fit::Quant> {
    tag.quant
        .as_deref()
        .and_then(fit::Quant::from_label)
        .or_else(|| tag.name.rsplit(':').next().and_then(fit::Quant::from_label))
}

/// GiB (2^30 bytes), the unit of every size in this feature: the catalogue's `file_gb`, the on-disk
/// scan's `size_gb`, and the VRAM and free RAM the hardware probe reports.
///
/// It was 1e9 bytes, under a comment claiming the same — so a served file read 7.4% larger than the
/// very same file in the catalogue (5.44 against 5.07 for Qwen2.5 7B Q5_K_M), no longer fitted the
/// 8 GB card PM had picked it for, and PM told the user to download the model they were serving.
fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

/// The spec to score an on-disk model with, using the REAL file size on disk as the weight term, or
/// the `unknown` result its card shows instead. `fit::fit` of the spec is the card's fit; PM's pick
/// judges the same spec.
///
/// Per #449's rules a file PM can't characterise is never guessed at: a name that matches no catalog
/// entry, or a quant label that isn't one PM knows, comes back `unknown` with the reason said plainly.
/// When both are known the catalog supplies the architecture, active-parameter count and context
/// window, while the single quant candidate carries the measured on-disk size.
fn on_disk_spec(
    model: &local_disk::DiskModel,
    matched: Option<&local_catalog::CatalogEntry>,
) -> std::result::Result<fit::ModelSpec, fit::FitResult> {
    let Some(entry) = matched else {
        return Err(not_in_catalog());
    };
    // Two different situations, and collapsing them throws away real information. PM may have found
    // no quantization at all, or know exactly which one the file is and have no weight for it. Only
    // the first is honestly "couldn't tell" — and since the on-disk weight is MEASURED, the second is
    // worth naming so it reads as a gap in PM rather than a defect in the file.
    let quant = match model.quant.as_deref() {
        None => return Err(fit::unknown(UNREADABLE_QUANT.to_string())),
        Some(label) => match fit::Quant::from_label(label) {
            Some(quant) => quant,
            None => {
                return Err(match safe_quant_label(label) {
                    Some(shown) => fit::unknown(format!(
                        "PM doesn't have a size for the {shown} quantization yet, so its fit can't \
                         be estimated."
                    )),
                    None => fit::unknown(UNREADABLE_QUANT.to_string()),
                });
            }
        },
    };
    let mut spec = local_catalog::entry_to_spec(entry);
    spec.candidates = vec![fit::QuantCandidate {
        quant,
        weight_gb: model.size_gb,
    }];
    // The file set on THIS disk is ground truth for both terms, so the measured projector replaces
    // the catalog's figure. `Some(0.0)`, never `None`: "no projector on disk" is a measurement, not a
    // gap, and the two must stay distinguishable even now that an unsized projector no longer refuses
    // the whole fit. Leaving the catalog value here while `weight_gb` came from disk was the
    // double-count: `local_disk` had already folded the projector into `size_gb`.
    spec.projector_gb = Some(model.sidecar_gb);
    Ok(spec)
}

/// Run a fresh hardware scan off the async runtime and cache it. Shared by both commands.
async fn scan_hardware(app: &AppHandle) -> Result<hardware::Hardware> {
    let data_dir = paths::data_dir(app).ok();
    let hw = tauri::async_runtime::spawn_blocking(move || hardware::scan(data_dir.as_deref()))
        .await
        .map_err(|e| Error::Other(format!("hardware scan task failed: {e}")))?;
    app.state::<AppState>().local_ai.cache_hardware(hw.clone());
    Ok(hw)
}

/// Sort key so the best-fitting, most-capable models rise to the top of the list.
fn verdict_rank(v: fit::Verdict) -> u8 {
    match v {
        fit::Verdict::Comfortable => 0,
        fit::Verdict::Tight => 1,
        fit::Verdict::HalvedContext => 2,
        fit::Verdict::StayOnCloud => 3,
        fit::Verdict::Unknown => 4,
    }
}

/// The Ollama pull target for the quant a fit actually chose, and whether that quant is sharded.
///
/// Keyed on the FITTED quant, never the entry: the card's memory verdict is about one specific
/// quantization, so a per-entry tag would offer a download for a file the card never sized — and the
/// number it showed would be a lie about the thing the button fetches. Matched through
/// [`fit::Quant::from_label`], the same function [`local_catalog::entry_to_spec`] used to build the
/// candidate list, so the round-trip is exact rather than a string comparison that could drift.
///
/// Pure, so the invariant is testable across the whole catalogue without an `AppHandle`.
fn pull_target_for(
    entry: &local_catalog::CatalogEntry,
    chosen: Option<fit::Quant>,
) -> (Option<String>, bool) {
    let Some(row) = quant_row(entry, chosen) else {
        return (None, false);
    };
    (row.ollama.clone(), row.sharded)
}

/// The catalogue row for the quant a fit chose, matched as [`pull_target_for`] matches it.
fn quant_row(
    entry: &local_catalog::CatalogEntry,
    chosen: Option<fit::Quant>,
) -> Option<&local_catalog::CatalogQuant> {
    let chosen = chosen?;
    entry
        .quants
        .iter()
        .find(|q| fit::Quant::from_label(&q.quant) == Some(chosen))
}

/// The SECOND rung's download, or `None` when the card shows only one rung.
///
/// Pure, and separate from `fit::gpu_fit` on purpose: `fit.rs` carries no catalogue concepts and
/// thirteen tests construct `GpuFit` directly. `gpu_fit` was handed the same `entry_to_spec(entry)`
/// spec that produced the candidate list, so the GPU rung's quant is one of this entry's own rows by
/// construction and `pull_target_for` maps it back exactly.
fn gpu_pull_target(
    entry: &local_catalog::CatalogEntry,
    gpu: &fit::GpuFit,
    ram_quant: Option<fit::Quant>,
) -> Option<PullTarget> {
    let fit::GpuFit::Split { fit: g } = gpu else {
        return None;
    };
    let (tag, sharded) = pull_target_for(entry, g.quant);
    Some(PullTarget {
        tag,
        sharded,
        // `gpu_fit` splits when quant, context OR kv differ, so a rung that only drops the KV cache
        // to q8_0 names the SAME file run with different settings. Saying otherwise sends the user
        // hunting for a second download that does not exist.
        same_file: g.quant.is_some() && g.quant == ram_quant,
    })
}

/// One rung's Ollama download, resolved from the quant that rung was actually measured at.
///
/// `tag: None` is a real answer — a rung whose quant Ollama cannot fetch — and the UI must say why
/// rather than render a button that fails or a silent gap.
#[derive(Serialize)]
pub struct PullTarget {
    /// `hf.co/<repo>:<QUANT>`, or `None` when this quant has no fetchable tag.
    pub tag: Option<String>,
    /// The reason `tag` is `None`: a split GGUF, which Ollama's registry route refuses by design.
    pub sharded: bool,
    /// This rung names the SAME file as the highest-quality rung, differing only in the settings the
    /// runner is given (context, or a q8_0 KV cache). One download, two ways to run it.
    pub same_file: bool,
}

/// One curated model, scored against this machine.
#[derive(Serialize)]
pub struct Recommendation {
    pub repo: String,
    pub display_name: String,
    pub architecture: String,
    pub role_hint: Option<String>,
    pub parameters_b: f64,
    pub active_parameters_b: f64,
    pub context_length: u32,
    pub multimodal: bool,
    pub reasoning: Option<bool>,
    /// The Ollama pull target for the quant `fit` chose, or `None` when there is none to offer.
    /// `None` is the honest answer, not a gap: the UI must render no Download button rather than one
    /// that fails, and it says why when the reason is a sharded GGUF.
    pub ollama_pull: Option<String>,
    /// The fitted quant ships as split GGUF shards. Ollama's registry route refuses those by design,
    /// so this is the one reason a model PM would otherwise offer has no Download button — and the
    /// UI says so rather than leaving a silent gap.
    pub sharded_quant: bool,
    /// The download for the "fastest on GPU" rung, when the card shows one. `None` means there is no
    /// second rung at all — NOT that it can't be fetched; that is `Some(PullTarget { tag: None, .. })`.
    /// Kept beside `ollama_pull` rather than replacing it: three files pin the flat pair.
    pub gpu_pull: Option<PullTarget>,
    /// What the weights are licensed under. Rides with the row so the UI can label every model and
    /// show the terms before a restricted download without a second call.
    pub licence: local_catalog::EntryLicence,
    /// The highest-quality config that fits system RAM (unchanged from before the two-budget split).
    pub fit: fit::FitResult,
    /// Whether a faster GPU-resident config is worth showing beside `fit` (#457). `Single` when there
    /// is nothing distinct to add (no discrete GPU, unified memory, unscoreable, or already on GPU).
    pub gpu: fit::GpuFit,
}

/// A model the configured endpoint already serves, matched to the catalog when possible.
#[derive(Serialize)]
pub struct InstalledModel {
    pub id: String,
    pub matched_repo: Option<String>,
    pub fit: fit::FitResult,
    /// The fit describes the user's OWN file at the window the server proved it serves: its size and
    /// quant came from `/api/tags`, and the context from a load PM saw. Without both it is the
    /// catalogue's figure for the model, which is a fair guess but no grounds for telling someone
    /// their model "runs from system memory".
    pub measured: bool,
    /// PM can show it does not fit this machine's dedicated graphics card ([`spills_gpu`]): the
    /// server reports it loaded partly off the card, or, not known to be loaded, the user's own file
    /// at its served window outgrows the card even with a q8_0 cache. The only ground for "runs
    /// from system memory"; `false` whenever PM cannot show it, `fit.speed_basis` included — that
    /// is sized f16-first and says nothing about the cache the server really runs.
    pub spills_gpu: bool,
    /// The server loaded it with nothing on this machine's dedicated graphics card ([`card_unused`]):
    /// it isn't using the card at all. Distinct from `spills_gpu`, because switching models won't
    /// help, and the tab must not say it would.
    pub card_unused: bool,
}

/// One model the configured endpoint is serving, plus whether it can answer a chat turn.
///
/// The flag travels WITH the id rather than the id being filtered out, so the role pickers can show
/// an embedder disabled-with-a-reason instead of silently omitting it — a model the user can see in
/// Ollama but not in PM reads as a PM bug.
#[derive(Serialize)]
pub struct ServedModel {
    pub id: String,
    /// True when this is an embedding/reranking model, so nothing may bind it to a chat or
    /// background role.
    pub embedding: bool,
}

impl ServedModel {
    fn classify(id: String) -> Self {
        let embedding = local_catalog::is_embedding_or_reranker(&id);
        Self { id, embedding }
    }
}

/// A model found on disk that no endpoint is currently serving (#449), scored on its real file size.
#[derive(Serialize)]
pub struct OnDiskModel {
    pub name: String,
    pub source: local_disk::DiskSource,
    pub path: String,
    /// Weights only — the projector is `sidecar_gb`, so this stays comparable with the catalog.
    pub size_gb: f64,
    /// The projector that loads with it, measured on disk; `0.0` when there is none.
    pub sidecar_gb: f64,
    pub quant: Option<String>,
    pub shards: u32,
    pub matched_repo: Option<String>,
    pub fit: fit::FitResult,
}

/// The Workbench recommendations payload.
#[derive(Serialize)]
pub struct Recommendations {
    pub hardware: hardware::Hardware,
    /// System RAM kept free when scoring (surfaced so the UI can state it).
    pub reserve_gb: f64,
    /// VRAM kept free when sizing the GPU-resident config (surfaced beside `reserve_gb`).
    pub gpu_reserve_gb: f64,
    pub catalog_version: u32,
    /// The UTC date the catalog content last changed — for a "catalog from <date>" line.
    pub catalog_generated_at: String,
    pub endpoint_configured: bool,
    /// The rescan cadence as stored (`on-catalog-update` default).
    pub cadence: String,
    /// A read-only signal for PR5's passive "a better-fitting model is available" nudge.
    pub rescan_due: bool,
    pub curated: Vec<Recommendation>,
    pub installed: Vec<InstalledModel>,
    /// Downloaded but not currently served (#449) — de-duplicated against `installed`.
    pub on_disk: Vec<OnDiskModel>,
    /// Which runners' model folders exist on this machine AND could be read, so the UI can say
    /// "Ollama is here with nothing downloaded" rather than implying it isn't installed.
    pub disk_sources_present: Vec<local_disk::DiskSource>,
    /// Roots that are there and unreadable — a packaged Linux Ollama's store, or a folder the user
    /// pointed PM at that belongs to someone else. Separate from `disk_sources_present` because it
    /// supports a different and more useful sentence: PM can name the cause rather than reporting
    /// an absence it did not observe.
    pub disk_blocked: Vec<local_disk::BlockedRoot>,
    /// How many models the crawl found on disk, BEFORE `on_disk` removed the ones already served.
    /// Lets the UI separate "no model folder here" from "a folder, with nothing downloaded in it" —
    /// two different sentences, and the second is what a user sees the moment they remove their
    /// last model.
    pub disk_found: usize,
    /// How many models the configured endpoint answered with — the length of `installed`, but only
    /// when the probe actually succeeded.
    ///
    /// Three-valued on purpose, and a consumer must not flatten it. `None` = nothing answered: no
    /// endpoint configured, unreachable, or refused by the cleartext gate. `Some(0)` = a server
    /// that is running with nothing pulled into it yet — a first-time installer's exact state, and
    /// a completely different sentence from "PM couldn't find a model folder".
    ///
    /// This is deliberately NOT a second HTTP call. Ollama's `/v1/models` lists what has been
    /// PULLED, not what is loaded (`/api/ps` is the resident list), so the probe PM already makes
    /// carries the whole store — and unlike Ollama's native `/api/tags` it also answers for
    /// llama-server and LM Studio, and survives a proxy that only forwards `/v1`.
    pub endpoint_inventory: Option<usize>,
    /// The two role models weighed against this machine together, or `None` when only one model is
    /// in play (a role on cloud, a role unbound, or the same model on both).
    ///
    /// Here rather than on a catalogue card because a card is about ONE model and has no idea what
    /// the other role holds.
    pub co_residency: Option<fit::CoResidencyFit>,
    /// The crawl hit its bound, so `on_disk` is a prefix rather than everything on disk.
    pub disk_truncated: bool,
    /// The extra folder the crawl includes, when one is set.
    pub scan_dir: Option<String>,
    /// Licence ids the user has already read and accepted, so a second Gemma does not re-ask.
    pub terms_accepted: Vec<String>,
    /// PM's pick for this computer — the one model it would run here, or why there is none
    /// ([`better_fit::pick`]). It writes nothing and changes nothing else in this payload: `curated`
    /// keeps its own order and every badge is its own.
    pub pick: better_fit::Pick,
    /// The free RAM every verdict in this payload was scored against, read live for this call.
    /// `hardware.available_ram_gb` is the cached scan's figure, which can be minutes old.
    pub live_available_ram_gb: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_cards_second_rung_gets_its_own_download() {
        // The defect this pins: `Recommendation` carried ONE pull target, resolved from the RAM rung,
        // so the "fastest on GPU" row a user actually wants had no download at all — and the card
        // printed a caption admitting it instead of closing the gap.
        fn rung(q: Option<fit::Quant>) -> fit::FitResult {
            fit::FitResult {
                verdict: fit::Verdict::Comfortable,
                quant: q,
                context: Some(32768),
                kv: fit::KvCache::Q8_0,
                est_memory_gb: Some(6.6),
                est_tokens_per_sec: Some(71.0),
                speed_basis: None,
                notes: vec![],
            }
        }
        let cat = local_catalog::catalog();
        let e = cat
            .entries
            .iter()
            .find(|e| e.repo == "bartowski/Qwen2.5-7B-Instruct-GGUF")
            .expect("catalogue entry");

        // A rung with its OWN quant resolves to that quant's tag, never the RAM rung's.
        let split = fit::GpuFit::Split {
            fit: rung(Some(fit::Quant::Q5_K_M)),
        };
        let t =
            gpu_pull_target(e, &split, Some(fit::Quant::Q8_0)).expect("a split has a second rung");
        assert_eq!(
            t.tag.as_deref(),
            Some("hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M")
        );
        assert!(!t.sharded);
        assert!(!t.same_file, "different quants are different files");

        // Same quant, different settings: `gpu_fit` splits on context or kv alone, so this is ONE
        // file run two ways. Telling the user to find a second download would be false.
        let same = fit::GpuFit::Split {
            fit: rung(Some(fit::Quant::Q8_0)),
        };
        let t = gpu_pull_target(e, &same, Some(fit::Quant::Q8_0)).expect("still a split");
        assert!(t.same_file, "one file, two ways to run it");
        assert_eq!(
            t.tag.as_deref(),
            Some("hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q8_0")
        );

        // No second rung is None — distinct from a second rung that exists but cannot be fetched.
        assert!(gpu_pull_target(e, &fit::GpuFit::Single, Some(fit::Quant::Q8_0)).is_none());
        assert!(gpu_pull_target(e, &fit::GpuFit::NoGpuResident, Some(fit::Quant::Q8_0)).is_none());
    }

    #[test]
    fn a_fetchable_gpu_rung_survives_an_unfetchable_ram_rung() {
        // Reachable in the committed catalogue on a large-RAM machine: the 72B's Q5_K_M/Q6_K/Q8_0 are
        // sharded (no tag), while its Q4_K_M is tagged. PM used to offer nothing at all there,
        // because the single pull target came from the RAM rung.
        let cat = local_catalog::catalog();
        let e = cat
            .entries
            .iter()
            .find(|e| e.repo == "bartowski/Qwen2.5-72B-Instruct-GGUF")
            .expect("catalogue entry");
        let (ram_tag, ram_sharded) = pull_target_for(e, Some(fit::Quant::Q5_K_M));
        assert!(
            ram_tag.is_none() && ram_sharded,
            "the RAM rung is the sharded one"
        );

        let split = fit::GpuFit::Split {
            fit: fit::FitResult {
                verdict: fit::Verdict::Comfortable,
                quant: Some(fit::Quant::Q4_K_M),
                context: Some(8192),
                kv: fit::KvCache::Q8_0,
                est_memory_gb: Some(40.0),
                est_tokens_per_sec: Some(9.0),
                speed_basis: None,
                notes: vec![],
            },
        };
        let t = gpu_pull_target(e, &split, Some(fit::Quant::Q5_K_M)).expect("second rung");
        assert_eq!(
            t.tag.as_deref(),
            Some("hf.co/bartowski/Qwen2.5-72B-Instruct-GGUF:Q4_K_M"),
            "a fetchable rung must be offered even when the other one cannot be"
        );
        assert!(!t.sharded);
    }

    #[test]
    fn the_download_button_always_names_the_quant_the_card_sized() {
        // The defect this pins: the pull hint used to be per-ENTRY, so the button could fetch a
        // different quantization from the one the card's memory verdict described. Swept over the
        // whole committed catalogue rather than a fixture, so a future entry cannot slip past.
        let cat = local_catalog::catalog();
        let mut offered = 0usize;
        for e in &cat.entries {
            // Nothing to download when the fit could not pick a quant at all.
            assert_eq!(pull_target_for(e, None), (None, false), "{}", e.repo);

            for q in &e.quants {
                let chosen = fit::Quant::from_label(&q.quant)
                    .unwrap_or_else(|| panic!("{}: unknown quant {}", e.repo, q.quant));
                let (tag, sharded) = pull_target_for(e, Some(chosen));
                assert_eq!(sharded, q.sharded, "{} {}: sharded flag", e.repo, q.quant);
                match tag {
                    Some(t) => {
                        assert_eq!(
                            t,
                            format!("hf.co/{}:{}", e.repo, q.quant),
                            "{}: asked for {} and got a tag for something else",
                            e.repo,
                            q.quant
                        );
                        assert!(
                            !q.sharded,
                            "{} {}: offered a sharded GGUF, which Ollama's registry refuses",
                            e.repo, q.quant
                        );
                        offered += 1;
                    }
                    // The only legitimate refusal in the committed catalogue.
                    None => assert!(
                        q.sharded,
                        "{} {}: no tag, and not because it is sharded",
                        e.repo, q.quant
                    ),
                }
            }
        }
        // Guards the shipped state this replaced: every row null, and nothing noticed.
        assert!(
            offered >= 60,
            "only {offered} quant rows are downloadable — the catalogue lost its pull tags"
        );
    }

    #[test]
    fn the_footer_names_a_local_model_only_when_routing_actually_reaches_it() {
        // The defect: the model footer read the OpenRouter list for both rows and had no access to
        // routing at all, so a machine answering every turn from its own GPU displayed a cloud
        // model's name — with "Local connected" underneath it, stating the exact inverse.
        assert_eq!(
            role_local_model(Some("local"), Some("qwen2.5:7b")),
            Some("qwen2.5:7b".to_string())
        );
        assert_eq!(
            role_local_model(Some("local-then-cloud"), Some("qwen2.5:7b")),
            Some("qwen2.5:7b".to_string()),
            "local is tried FIRST, so it is what answers"
        );

        // Cloud routing: the binding is irrelevant however it is set, and the cloud model is the
        // honest answer for that row.
        assert_eq!(role_local_model(Some("cloud"), Some("qwen2.5:7b")), None);
        // An absent preference parses to cloud everywhere else; it must here too.
        assert_eq!(role_local_model(None, Some("qwen2.5:7b")), None);
        // A pointed-at-local role with nothing bound has no name to show.
        assert_eq!(role_local_model(Some("local"), None), None);
        assert_eq!(role_local_model(Some("local"), Some("")), None);
    }

    #[test]
    fn quant_labels_from_a_config_blob_are_bounded_before_they_reach_the_ui() {
        assert_eq!(safe_quant_label("Q4_0").as_deref(), Some("Q4_0"));
        assert_eq!(safe_quant_label("tq1_0").as_deref(), Some("TQ1_0"));
        // Ollama's `file_type` is file content, so it is untrusted: a long or markup-ish value must
        // not reach the message intact.
        assert_eq!(
            safe_quant_label("<script>alert(1)</script>").as_deref(),
            Some("SCRIPTALERT1SCRIPT")
        );
        let long = safe_quant_label(&"A".repeat(500)).unwrap();
        assert_eq!(long.len(), 24, "the label must be length-bounded");
        // Nothing label-shaped survives, so the caller uses the generic wording instead of a gap.
        assert_eq!(safe_quant_label("   "), None);
        assert_eq!(safe_quant_label(""), None);
        assert_eq!(safe_quant_label("//"), None);
    }

    fn installed_model(id: &str, gb: f64) -> InstalledModel {
        InstalledModel {
            id: id.to_string(),
            matched_repo: None,
            measured: false,
            spills_gpu: false,
            card_unused: false,
            fit: fit::FitResult {
                verdict: fit::Verdict::Comfortable,
                quant: Some(fit::Quant::Q4_K_M),
                context: Some(32768),
                kv: fit::KvCache::F16,
                est_memory_gb: Some(gb),
                est_tokens_per_sec: Some(30.0),
                speed_basis: None,
                notes: vec![],
            },
        }
    }

    #[test]
    fn a_served_model_is_scored_on_the_file_the_user_actually_has() {
        // The defect this closes, with the real numbers that exposed it. PM scored a model the
        // endpoint was SERVING with `fit::fit`, which picks the best quantization that fits the
        // budget — right advice for something you have not downloaded, fiction for something already
        // on your disk. On a 17.3 GB / 8 GB-card laptop it believed a served Qwen2.5-7B was Q8_0 at
        // 10.04 GB while the actual file was Q5_K_M at 5.44 GB, and a served gemma-3-4b was 9.21 GB
        // against a real 3.34 GB. Summed for a co-residency warning that is 19.25 GB of fiction.
        let hw = fit::FitHardware {
            available_ram_gb: 17.3,
            vram_gb: Some(8.0),
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        };
        let cat = local_catalog::catalog();
        let qwen = cat
            .entries
            .iter()
            .find(|e| e.repo.contains("Qwen2.5-7B-Instruct"))
            .expect("catalogue entry");

        // What shipped: the catalogue's best-fitting quant, not the user's file.
        let (spec, measured) = served_spec(Some(qwen), None, None).expect("a catalogue match");
        assert!(
            !measured,
            "nothing was measured, so the figures are the catalogue's"
        );
        let guessed = fit::fit(&spec, &hw);
        assert_eq!(guessed.quant, Some(fit::Quant::Q8_0));

        // With the server's own answer — 5.44 GB of Q5_K_M, loaded at the 32768 it really serves.
        let tag = openai_compat::OllamaTag {
            name: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M".to_string(),
            size_bytes: 5_444_833_987,
            quant: Some("Q5_K_M".to_string()),
            parameter_size_b: Some(7.62),
        };
        let (spec, measured) =
            served_spec(Some(qwen), Some(&tag), Some(32768)).expect("a catalogue match");
        assert!(
            measured,
            "the server's own size and quant replaced the guess"
        );
        let real = fit::fit(&spec, &hw);
        assert_eq!(real.quant, Some(fit::Quant::Q5_K_M));
        let est = real.est_memory_gb.expect("a measured file has a footprint");
        assert!(
            est < guessed.est_memory_gb.unwrap(),
            "the measured file must not cost more than the guess it replaces: {est}"
        );
        // Measured resident on that machine: 6.41 GB. The estimate must stay ABOVE it — the fit
        // bar is "never under" — while being far closer than the 10.04 GB it replaces.
        assert!(
            (6.41..8.5).contains(&est),
            "estimate should sit just above the 6.41 GB measured, got {est}"
        );
    }

    #[test]
    fn a_quantization_the_server_would_not_name_is_read_off_the_tag_it_was_pulled_under() {
        // Ollama reports `"unknown"` for some repos while the tag names the quant outright. That is
        // a real second source, not a guess: it is the string the user typed to fetch this exact
        // file. Measured on a live server for `hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M`.
        let tag = openai_compat::OllamaTag {
            name: "hf.co/ggml-org/gemma-3-4b-it-GGUF:Q4_K_M".to_string(),
            size_bytes: 3_341_010_115,
            quant: None,
            parameter_size_b: None,
        };
        assert_eq!(served_quant(&tag), Some(fit::Quant::Q4_K_M));

        // What the server says still wins when it says anything at all.
        let named = openai_compat::OllamaTag {
            quant: Some("Q5_K_M".to_string()),
            parameter_size_b: None,
            ..tag.clone()
        };
        assert_eq!(served_quant(&named), Some(fit::Quant::Q5_K_M));

        // And a tag that names nothing usable stays unknown rather than being invented.
        let bare = openai_compat::OllamaTag {
            name: "llama3.2:latest".to_string(),
            quant: None,
            parameter_size_b: None,
            ..tag
        };
        assert_eq!(served_quant(&bare), None);
    }

    #[test]
    fn co_residency_is_asked_only_when_two_different_models_are_really_in_play() {
        // Each of these is a case where a warning would be WRONG, not merely unhelpful — which is why
        // the absence is `None` (no question) rather than a verdict meaning "fine".
        let hw = fit::FitHardware {
            available_ram_gb: 32.0,
            vram_gb: None,
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        };
        let served = [
            installed_model("chat-model", 6.0),
            installed_model("bg-model", 6.0),
        ];

        // A role on cloud: one model on this machine, costing exactly what its card said.
        assert!(co_residency_for_roles(Some("chat-model"), None, &served, &hw).is_none());
        assert!(co_residency_for_roles(None, Some("bg-model"), &served, &hw).is_none());
        // The same model on both roles: one resident model, nothing to add up. The commonest setup
        // of all, and the one a permanently-on warning trained people to ignore.
        assert!(
            co_residency_for_roles(Some("chat-model"), Some("CHAT-MODEL"), &served, &hw).is_none()
        );
        // A role bound to something the endpoint is not serving.
        assert!(co_residency_for_roles(Some("chat-model"), Some("absent"), &served, &hw).is_none());

        // Two different served models — the one case where the choices interact.
        let out = co_residency_for_roles(Some("chat-model"), Some("bg-model"), &served, &hw)
            .expect("two different served models is a real question");
        assert_eq!(out.combined_gb, Some(12.0));
        assert_eq!(out.ram, fit::CoResidency::Fits);
    }

    #[test]
    fn splits_scheme_host_port_across_forms() {
        assert_eq!(
            split_scheme_host_port("http://localhost:11434").unwrap(),
            ("http".into(), "localhost".into(), 11434)
        );
        assert_eq!(
            split_scheme_host_port("https://box.local").unwrap(),
            ("https".into(), "box.local".into(), 443)
        );
        assert_eq!(
            split_scheme_host_port("http://127.0.0.1").unwrap(),
            ("http".into(), "127.0.0.1".into(), 80)
        );
        assert_eq!(
            split_scheme_host_port("http://[::1]:8080").unwrap(),
            ("http".into(), "::1".into(), 8080)
        );
        assert!(split_scheme_host_port("localhost:11434").is_err());
    }

    #[test]
    fn ip_literal_classification_needs_no_dns() {
        // Cheap tokio runtime for the async classifier over IP literals (no real DNS, no IO driver).
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            assert_eq!(
                resolve_endpoint_class("127.0.0.1", 11434).await.unwrap(),
                EndpointClass::Loopback
            );
            assert_eq!(
                resolve_endpoint_class("192.168.1.50", 8080).await.unwrap(),
                EndpointClass::PrivateRemote
            );
            assert_eq!(
                resolve_endpoint_class("8.8.8.8", 443).await.unwrap(),
                EndpointClass::PublicRemote
            );
        });
    }

    /// The call-time gate over IP literals (no DNS, so no network). This is the mechanical form of
    /// the "same policy, just asked later" claim: every shape a real setup can have — loopback,
    /// LAN, Tailscale/CGNAT, and https anywhere — is left alone, and the single combination
    /// `set_local_llm_endpoint` already refuses at save time is the only one refused here.
    /// `local_slot`'s `http_posture_refuses_only_public_cleartext` pins the POLICY; this pins that
    /// the same policy now runs at the I/O edge.
    #[test]
    fn endpoint_refused_now_refuses_only_public_cleartext() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            assert!(
                endpoint_refused_now("http://8.8.8.8:11434").await,
                "cleartext to a public address is the one refusal"
            );
            for allowed in [
                "http://127.0.0.1:11434",
                "http://[::1]:11434",
                "http://192.168.1.50:8080",
                "http://100.100.3.4:8080", // Tailscale's CGNAT range
                "https://8.8.8.8",         // https anywhere is fine
            ] {
                assert!(
                    !endpoint_refused_now(allowed).await,
                    "must keep working exactly as before: {allowed}"
                );
            }
        });
    }

    /// Fail OPEN on anything that cannot be classified. This is the property protecting a
    /// local-then-cloud user's fallback: making a DNS hiccup a refusal would silently cost them
    /// their cloud arm, while an endpoint that genuinely cannot be reached already fails as
    /// `Refused` moments later. The asymmetry — open on "don't know", closed only on a POSITIVE
    /// public-cleartext verdict — is deliberate, and a tidy-up that collapses it reintroduces the
    /// bug this test names.
    #[test]
    fn an_unclassifiable_endpoint_is_not_refused() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            // No scheme: `split_scheme_host_port` errors before any lookup is attempted.
            assert!(!endpoint_refused_now("localhost:11434").await);
            // A host the resolver rejects outright — resolution FAILS, and a failure is not a
            // refusal. (Rejected locally by getaddrinfo, so this issues no DNS query.)
            assert!(!endpoint_refused_now("http://not a host:11434").await);
        });
    }

    /// The test result shows the model's own words back, so the cap has to hold a String that may be
    /// any bytes the model produced — including multi-byte ones exactly on the boundary.
    #[test]
    fn a_test_reply_is_capped_on_a_character_boundary() {
        assert_eq!(cap_reply("  ready  "), "ready");
        assert_eq!(cap_reply("ready"), "ready");

        // Exactly at the cap: nothing to trim, and nothing appended.
        let exact: String = "a".repeat(TEST_REPLY_CAP);
        assert_eq!(cap_reply(&exact), exact);

        // Over it, in characters that are three bytes each — a byte-indexed slice here would panic.
        let long: String = "\u{4f60}".repeat(TEST_REPLY_CAP + 50);
        let capped = cap_reply(&long);
        assert_eq!(
            capped.chars().count(),
            TEST_REPLY_CAP + 3,
            "the cap plus the ellipsis"
        );
        assert!(capped.ends_with("..."));
    }

    // ---- the On battery section's status (#432) ----

    use crate::llm_gateway::{PowerBlocked, ProviderPref, RoutingPrefs};
    use crate::power::{PowerScope, PowerSource, PowerState};

    fn low_battery() -> PowerSnapshot {
        PowerSnapshot {
            reading: PowerReading {
                source: PowerSource::Battery,
                percent: Some(40),
                has_battery: true,
            },
            state: PowerState::BatteryLow,
            threshold: Some(60),
        }
    }

    fn policy(consent: bool) -> PowerSettings {
        PowerSettings {
            threshold: 60,
            scope: PowerScope::Both,
            consent: if consent {
                power::Consent::ALL
            } else {
                power::Consent::NONE
            },
        }
    }

    fn both(pref: ProviderPref) -> RoutingPrefs {
        RoutingPrefs {
            chat: pref,
            background: pref,
        }
    }

    #[test]
    fn a_routed_role_names_the_model_it_parked() {
        let view = power_view(
            &low_battery(),
            &policy(true),
            false,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            Some("qwen3:8b"),
            KeyPresence::Present,
            KeyPresence::Present,
        );
        assert_eq!(view.chat.route, PowerRoute::Cloud);
        assert_eq!(view.chat.local_model.as_deref(), Some("gemma3:4b"));
        assert_eq!(view.background.route, PowerRoute::Cloud);
        assert_eq!(view.background.local_model.as_deref(), Some("qwen3:8b"));
        assert!(!view.consent_needed);
        assert_eq!(view.threshold, 60);
        assert_eq!(view.return_at, 75);
        assert_eq!(view.state, PowerState::BatteryLow);
        assert_eq!(view.percent, Some(40));
    }

    #[test]
    fn with_no_endpoint_nothing_can_move() {
        for pref in [
            ProviderPref::Cloud,
            ProviderPref::Local,
            ProviderPref::LocalThenCloud,
        ] {
            let view = power_view(
                &low_battery(),
                &policy(true),
                false,
                &both(pref),
                false,
                Some("gemma3:4b"),
                Some("gemma3:4b"),
                KeyPresence::Present,
                KeyPresence::Present,
            );
            for role in [&view.chat, &view.background] {
                assert_eq!(role.route, PowerRoute::Unchanged, "{pref:?}");
                assert!(
                    matches!(
                        role.blocked,
                        Some(PowerBlocked::NoLocalModel) | Some(PowerBlocked::CloudRouting)
                    ),
                    "{pref:?}: {:?}",
                    role.blocked
                );
            }
        }
    }

    #[test]
    fn consent_is_needed_only_while_a_role_waits_for_it() {
        let asks = power_view(
            &low_battery(),
            &policy(false),
            false,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            None,
            KeyPresence::Present,
            KeyPresence::Present,
        );
        assert_eq!(asks.chat.route, PowerRoute::NeedsConsent);
        assert_eq!(asks.background.blocked, Some(PowerBlocked::NoLocalModel));
        assert!(asks.consent_needed);

        // The override silences the ask, and plugging in ends it.
        let kept = power_view(
            &low_battery(),
            &policy(false),
            true,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            None,
            KeyPresence::Present,
            KeyPresence::Present,
        );
        assert_eq!(kept.chat.route, PowerRoute::KeptLocal);
        assert!(!kept.consent_needed);
        let mains = power_view(
            &PowerSnapshot::default(),
            &policy(false),
            false,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            None,
            KeyPresence::Present,
            KeyPresence::Present,
        );
        assert!(!mains.consent_needed);

        // Plugged in while the latch still waits out its minute: nothing to ask about any more.
        let mut plugged = low_battery();
        plugged.reading.source = PowerSource::Ac;
        let settling = power_view(
            &plugged,
            &policy(false),
            false,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            None,
            KeyPresence::Present,
            KeyPresence::Present,
        );
        assert_eq!(settling.chat.route, PowerRoute::Unchanged);
        assert!(!settling.consent_needed);
    }

    #[test]
    fn a_consent_request_adds_the_roles_it_names_and_none_withdraws_them_all() {
        use power::PowerScope::*;
        assert_eq!(consent_write(None, "background").unwrap(), Some(Background));
        // Added to the earlier yes, never replacing it.
        assert_eq!(
            consent_write(Some("background"), "chat").unwrap(),
            Some(Both)
        );
        assert_eq!(consent_write(Some("both"), "chat").unwrap(), Some(Both));
        assert_eq!(consent_write(Some("chat"), " chat ").unwrap(), Some(Chat));
        assert_eq!(consent_write(Some("both"), "none").unwrap(), None);
        // Junk is refused, whatever is stored, rather than read as a yes or a no.
        assert!(consent_write(Some("both"), "true").is_err());
        assert!(consent_write(None, "").is_err());
    }

    #[test]
    fn a_background_key_alone_moves_background_and_not_chat() {
        // Chat uses only the primary key; background falls back to it but has its own. With only a
        // background key set up, chat has no cloud to move to and background does.
        let view = power_view(
            &low_battery(),
            &policy(true),
            false,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            Some("gemma3:4b"),
            KeyPresence::Absent,
            KeyPresence::Present,
        );
        assert_eq!(view.chat.blocked, Some(PowerBlocked::NoKey));
        assert_eq!(view.chat.route, PowerRoute::Unchanged);
        assert_eq!(view.background.blocked, None);
        assert_eq!(view.background.route, PowerRoute::Cloud);
        assert!(view.any_cloud_key, "a background key is a cloud provider");
    }

    #[test]
    fn the_reported_local_model_is_none_exactly_while_the_role_is_on_the_cloud() {
        for route in [
            PowerRoute::Unchanged,
            PowerRoute::NeedsConsent,
            PowerRoute::KeptLocal,
            PowerRoute::Cloud,
        ] {
            assert_eq!(
                reported_local(Some("gemma3:4b".into()), route).is_none(),
                route == PowerRoute::Cloud,
                "{route:?}"
            );
            assert_eq!(reported_local(None, route), None);
        }
    }

    /// The JSON the TypeScript mirror reads — every field name and enum value spelled once, here.
    #[test]
    fn the_power_view_serializes_the_shape_the_frontend_mirrors() {
        let view = power_view(
            &low_battery(),
            &policy(false),
            false,
            &RoutingPrefs {
                chat: ProviderPref::LocalThenCloud,
                background: ProviderPref::Cloud,
            },
            true,
            Some("gemma3:4b"),
            None,
            KeyPresence::Present,
            KeyPresence::Unreadable,
        );
        let v = serde_json::to_value(&view).unwrap();
        assert_eq!(v["source"], "battery");
        assert_eq!(v["percent"], 40);
        assert_eq!(v["has_battery"], true);
        assert_eq!(v["state"], "battery_low");
        assert_eq!(v["threshold"], 60);
        assert_eq!(v["return_at"], 75);
        assert_eq!(v["roles"], "both");
        assert_eq!(v["consent"], serde_json::Value::Null);
        assert_eq!(v["consent_needed"], true);
        assert_eq!(v["keep_local"], false);
        assert_eq!(v["any_cloud_key"], true);
        assert_eq!(v["chat"]["route"], "needs_consent");
        assert_eq!(v["chat"]["blocked"], serde_json::Value::Null);
        assert_eq!(v["chat"]["local_model"], "gemma3:4b");
        assert_eq!(v["background"]["route"], "unchanged");
        assert_eq!(v["background"]["blocked"], "cloud_routing");
        assert_eq!(v["background"]["local_model"], serde_json::Value::Null);
        assert_eq!(v["chat"]["effective"], "local_then_cloud");
        assert_eq!(v["chat"]["cloud_key"], "present");
        assert_eq!(v["background"]["effective"], "unknown");
        assert_eq!(v["background"]["cloud_key"], "unreadable");

        let release = serde_json::to_value(ReleaseSettings {
            policy: "server".into(),
            idle_minutes: 5,
            battery_idle_minutes: 0,
        })
        .unwrap();
        assert_eq!(release["battery_idle_minutes"], 0);
    }

    #[test]
    fn every_role_says_where_it_really_goes() {
        use crate::llm_gateway::EffectiveRoute;
        // "Local only" with nothing chosen has nothing to answer with. It does NOT use the cloud,
        // which is what the tab used to say of it; and a cloud role with no key is the same.
        let view = power_view(
            &PowerSnapshot::default(),
            &policy(false),
            false,
            &RoutingPrefs {
                chat: ProviderPref::Local,
                background: ProviderPref::Cloud,
            },
            true,
            None,
            None,
            KeyPresence::Present,
            KeyPresence::Absent,
        );
        assert_eq!(view.chat.effective, EffectiveRoute::Nothing);
        assert_eq!(view.background.effective, EffectiveRoute::Nothing);
        assert_eq!(view.chat.cloud_key, KeyPresence::Present);
        assert_eq!(view.background.cloud_key, KeyPresence::Absent);

        // Moved on battery, and the override that keeps a role local.
        let moved = power_view(
            &low_battery(),
            &policy(true),
            false,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            Some("gemma3:4b"),
            KeyPresence::Present,
            KeyPresence::Present,
        );
        assert_eq!(moved.chat.effective, EffectiveRoute::CloudForPower);
        assert_eq!(moved.background.effective, EffectiveRoute::CloudForPower);
        let kept = power_view(
            &low_battery(),
            &policy(true),
            true,
            &both(ProviderPref::LocalThenCloud),
            true,
            Some("gemma3:4b"),
            Some("gemma3:4b"),
            KeyPresence::Present,
            KeyPresence::Absent,
        );
        assert_eq!(kept.chat.effective, EffectiveRoute::LocalThenCloud);
        // No key behind it, so the fall-back is not there to take.
        assert_eq!(kept.background.effective, EffectiveRoute::LocalOnly);

        // No endpoint: a "Local, fall back to cloud" role with a key goes to the cloud, and one
        // whose key cannot be read cannot be placed at all.
        let unset = power_view(
            &PowerSnapshot::default(),
            &policy(false),
            false,
            &both(ProviderPref::LocalThenCloud),
            false,
            Some("gemma3:4b"),
            Some("gemma3:4b"),
            KeyPresence::Present,
            KeyPresence::Unreadable,
        );
        assert_eq!(unset.chat.effective, EffectiveRoute::Cloud);
        assert_eq!(unset.background.effective, EffectiveRoute::Unknown);
        assert_eq!(unset.background.cloud_key, KeyPresence::Unreadable);
    }

    // ---- PM's pick, against the committed catalogue (the redesign's pick-rule table) ----

    use crate::better_fit::{NoPick, OwnedRef, Pick, PickBasis, Rung};
    use crate::local_disk::{DiskModel, DiskSource};

    const QWEN_7B: &str = "bartowski/Qwen2.5-7B-Instruct-GGUF";

    fn entry(repo: &str) -> &'static local_catalog::CatalogEntry {
        local_catalog::catalog()
            .entries
            .iter()
            .find(|e| e.repo == repo)
            .unwrap_or_else(|| panic!("{repo} is in the committed catalogue"))
    }

    /// A discrete card of `vram` GB at `bandwidth` GB/s, with `free` GB of RAM free.
    fn card(vram: f64, bandwidth: Option<f64>, free: f64) -> fit::FitHardware {
        fit::FitHardware {
            available_ram_gb: free,
            vram_gb: Some(vram),
            gpu_bandwidth_gbps: bandwidth,
            unified_memory: false,
        }
    }

    /// The laptop the redesign's numbers were worked out on: an RTX 5060 Laptop GPU, 7.96 GB at
    /// 384 GB/s.
    fn laptop(free: f64) -> fit::FitHardware {
        card(7.96, Some(384.0), free)
    }

    fn no_gpu(free: f64) -> fit::FitHardware {
        fit::FitHardware {
            available_ram_gb: free,
            vram_gb: None,
            gpu_bandwidth_gbps: None,
            unified_memory: false,
        }
    }

    /// A model the endpoint serves, with the `/api/tags` row an Ollama would give for it.
    fn served(id: &str, bytes: u64, quant: &str, params_b: f64) -> ServedProbe {
        ServedProbe {
            id: id.to_string(),
            tag: Some(openai_compat::OllamaTag {
                name: id.to_string(),
                size_bytes: bytes,
                quant: Some(quant.to_string()),
                parameter_size_b: Some(params_b),
            }),
            served_ctx: None,
            resident: None,
        }
    }

    fn on_disk_file(name: &str, source: DiskSource, gb: f64, quant: &str) -> DiskModel {
        DiskModel {
            name: name.to_string(),
            source,
            path: format!("/home/example/models/{name}"),
            size_gb: gb,
            sidecar_gb: 0.0,
            quant: Some(quant.to_string()),
            shards: 1,
        }
    }

    fn pick_on(hw: &fit::FitHardware) -> Pick {
        size_for_machine(hw, Vec::new(), &[], None, &[]).pick
    }

    /// The repo, quant and memory figure of a catalogue pick, or a panic naming what it was instead.
    fn catalogue_pick(p: &Pick) -> (&str, Option<fit::Quant>, Option<f64>) {
        match p {
            Pick::Catalogue { repo, fit, .. } => (repo, fit.quant, fit.est_memory_gb),
            other => panic!("expected a catalogue pick, got {other:?}"),
        }
    }

    /// The repo of a pick's `also_have`, or a panic naming what it was instead.
    fn also_have_of(p: &Pick) -> Option<&str> {
        match p {
            Pick::Catalogue { also_have, .. } => also_have.as_ref().map(|o| o.id.as_str()),
            other => panic!("expected a catalogue pick, got {other:?}"),
        }
    }

    /// The owned option for `id`, as the pick weighed it.
    fn owned_named<'a>(s: &'a Sizing, id: &str) -> &'a better_fit::OwnedOption {
        s.owned
            .iter()
            .find(|o| o.id == id)
            .unwrap_or_else(|| panic!("{id} was weighed as a model you have"))
    }

    const GEMMA4_12B: &str = "unsloth/gemma-4-12b-it-GGUF";

    #[test]
    fn on_the_dev_laptop_the_pick_is_the_largest_model_that_stays_on_the_card() {
        // The redesign's headline case, at four amounts of free RAM. Sized from each model's own
        // attention geometry and judged at the 32768 PM runs it at, the largest model that fits the
        // 7.96 GB card with the reserve kept is gemma 4 12b at Q3_K_M: its sliding-window layers and
        // single-KV-head global layers make 32768 tokens of cache under 1 GB. Judged at its trained
        // 262144 it was a halved context and never eligible, so the pick was the 7.62B Qwen2.5.
        for free in [10.0, 13.4, 20.0, 24.0] {
            match pick_on(&laptop(free)) {
                Pick::Catalogue {
                    repo,
                    rung,
                    tag,
                    fit,
                    download_gb,
                    basis,
                    also_have,
                    ..
                } => {
                    assert_eq!(repo, GEMMA4_12B, "{free} GB");
                    assert_eq!(rung, Rung::Gpu, "{free} GB");
                    assert_eq!(tag, format!("hf.co/{GEMMA4_12B}:Q3_K_M"), "{free} GB");
                    assert_eq!(fit.quant, Some(fit::Quant::Q3_K_M), "{free} GB");
                    assert_eq!(fit.context, Some(32768), "{free} GB");
                    assert_eq!(fit.kv, fit::KvCache::F16, "{free} GB");
                    assert_eq!(fit.est_memory_gb, Some(6.93), "{free} GB");
                    assert_eq!(fit.verdict, fit::Verdict::Tight, "{free} GB");
                    assert_eq!(fit.est_tokens_per_sec, Some(65.8), "{free} GB");
                    assert_eq!(fit.speed_basis, Some(fit::SpeedBasis::GpuPublished));
                    // Weights plus the vision projector Ollama pulls with them.
                    assert!((download_gb - 5.46).abs() < 1e-9, "{download_gb}");
                    assert_eq!(basis, PickBasis::Gpu);
                    assert_eq!(also_have, None);
                }
                other => panic!("{free} GB: expected a catalogue pick, got {other:?}"),
            }
        }

        // The pick changes nothing about the list: its head at 20 GB is still the RAM config, and
        // the card keeps judging at the model's trained context.
        let s = size_for_machine(&laptop(20.0), Vec::new(), &[], None, &[]);
        let head = &s.curated[0];
        assert_eq!(head.repo, "unsloth/gemma-4-26B-A4B-it-GGUF");
        assert_eq!(head.fit.quant, Some(fit::Quant::Q3_K_M));
        assert_eq!(head.fit.context, Some(262144));
        assert_eq!(head.fit.speed_basis, Some(fit::SpeedBasis::System));
        let card = s.curated.iter().find(|r| r.repo == GEMMA4_12B).unwrap();
        assert_eq!(
            card.fit.context,
            Some(262144),
            "the card keeps its trained context"
        );
    }

    #[test]
    fn plenty_of_free_ram_never_talks_the_pick_into_a_model_the_card_cannot_hold() {
        // At 24 GB the Qwen2.5 14B and the 25B gemma 4 MoE both fit system RAM, and both are larger
        // than the pick — the very models a size-only rule would choose, at system-RAM speed. Neither
        // has a config on the card.
        let hw = laptop(24.0);
        let p = pick_on(&hw);
        let (repo, ..) = catalogue_pick(&p);
        for big in [
            "bartowski/Qwen2.5-14B-Instruct-GGUF",
            "unsloth/gemma-4-26B-A4B-it-GGUF",
        ] {
            let e = entry(big);
            let spec = local_catalog::entry_to_spec(e);
            let rf = fit::fit(&spec, &hw);
            assert!(better_fit::is_runnable(rf.verdict), "{big}: {rf:?}");
            assert_eq!(
                fit::gpu_fit(&spec, &hw, &rf),
                fit::GpuFit::NoGpuResident,
                "{big}"
            );
            assert_ne!(repo, e.repo);
        }
        assert_eq!(repo, GEMMA4_12B);
    }

    #[test]
    fn serving_the_picks_own_file_makes_it_the_model_you_already_have() {
        // The manifest total Hugging Face serves for this tag (model + projector + template +
        // params layers), which is what Ollama's `/api/tags` reports once it is pulled.
        let id = format!("hf.co/{GEMMA4_12B}:Q3_K_M");
        let hw = laptop(20.0);
        let s = size_for_machine(
            &hw,
            vec![served(&id, 5_868_989_011, "Q3_K_M", 11.91)],
            &[],
            Some("http://127.0.0.1:11434"),
            &[],
        );
        match &s.pick {
            Pick::Owned {
                id: got,
                repo,
                served,
                measured,
                fit,
                basis,
                ..
            } => {
                assert_eq!(got, &id);
                assert_eq!(repo, GEMMA4_12B);
                assert!(served);
                assert!(measured, "it is the very file PM sized");
                assert_eq!(fit.quant, Some(fit::Quant::Q3_K_M));
                assert_eq!(fit.est_memory_gb, Some(6.93));
                assert_eq!(*basis, PickBasis::Gpu);
            }
            other => panic!("expected the served model, got {other:?}"),
        }
    }

    #[test]
    fn a_served_file_is_measured_in_the_same_gib_as_the_catalogue() {
        // 5_444_833_987 bytes is the catalogue's Qwen2.5 7B Q5_K_M, 5.07 GiB. Read in decimal GB it
        // was 5.44, the same file no longer fitted the card at 32k, and a server that named it
        // anything but the catalogue's own tag was told to download the model it was serving.
        let hw = laptop(20.0);
        let probe = served("qwen2.5:7b-instruct-q5_K_M", 5_444_833_987, "Q5_K_M", 7.62);
        let (spec, measured) = served_spec(Some(entry(QWEN_7B)), probe.tag.as_ref(), None).unwrap();
        assert!(measured);
        assert!((spec.candidates[0].weight_gb - 5.07).abs() < 0.005);

        for other_name in [
            "qwen2.5:7b-instruct-q5_K_M",
            "hf.co/lmstudio-community/Qwen2.5-7B-Instruct-GGUF:Q5_K_M",
        ] {
            let s = size_for_machine(
                &hw,
                vec![served(other_name, 5_444_833_987, "Q5_K_M", 7.62)],
                &[],
                Some("http://127.0.0.1:11434"),
                &[],
            );
            let own = owned_named(&s, other_name);
            assert!(own.measured, "{other_name}");
            let config = own
                .config
                .as_ref()
                .expect("the user's own file fits the card at 32k");
            assert_eq!(config.quant, Some(fit::Quant::Q5_K_M), "{other_name}");
            assert_eq!(config.context, Some(32768), "{other_name}");
            assert_eq!(config.est_memory_gb, Some(6.5), "{other_name}");
            // So the pick, which is larger, names it as the model you already have.
            assert_eq!(also_have_of(&s.pick), Some(other_name));
        }
    }

    #[test]
    fn an_ollama_library_tag_is_recognised_and_judged_on_its_own_file() {
        // `qwen2.5:latest` names no size, so it matched nothing before `/api/tags` lent it one.
        let s = size_for_machine(
            &laptop(20.0),
            vec![served("qwen2.5:latest", 4_683_087_332, "Q4_K_M", 7.6)],
            &[],
            Some("http://127.0.0.1:11434"),
            &[],
        );
        assert_eq!(s.installed[0].matched_repo.as_deref(), Some(QWEN_7B));
        let own = owned_named(&s, "qwen2.5:latest");
        assert!(own.measured);
        let fit = own.config.as_ref().expect("its Q4_K_M fits the card");
        assert_eq!(fit.quant, Some(fit::Quant::Q4_K_M));
        assert_eq!(fit.context, Some(32768));
        assert_eq!(fit.kv, fit::KvCache::F16);
        assert_eq!(fit.est_memory_gb, Some(6.61));
        assert_eq!(fit.verdict, fit::Verdict::Tight);
        // 7.6 × 1.15 is short of gemma 4 12b's 11.91, so it is named beside the pick.
        assert_eq!(also_have_of(&s.pick), Some("qwen2.5:latest"));
    }

    #[test]
    fn a_smaller_model_you_have_is_named_beside_the_pick_rather_than_chosen() {
        // gemma 3 4b fits the card too, but 3.88 × 1.15 is well short of 11.91.
        let s = size_for_machine(
            &laptop(20.0),
            vec![served("gemma3:4b", 3_338_801_804, "Q4_K_M", 4.3)],
            &[],
            Some("http://127.0.0.1:11434"),
            &[],
        );
        match &s.pick {
            Pick::Catalogue {
                repo, also_have, ..
            } => {
                assert_eq!(repo, GEMMA4_12B);
                assert_eq!(
                    also_have.as_ref(),
                    Some(&OwnedRef {
                        id: "gemma3:4b".to_string(),
                        display_name: entry("ggml-org/gemma-3-4b-it-GGUF").display_name.clone(),
                        served: true,
                    })
                );
            }
            other => panic!("expected a catalogue pick, got {other:?}"),
        }
    }

    #[test]
    fn a_copy_on_disk_that_only_fits_the_card_at_half_its_context_is_not_the_pick() {
        // The Q6_K on disk spills past the reserve at 32k even with a q8_0 cache (7.25 GB against
        // 6.96), so it is not even named beside the pick: it has no acceptable config.
        let file = on_disk_file(
            "bartowski/Qwen2.5-7B-Instruct-GGUF/Qwen2.5-7B-Instruct-Q6_K.gguf",
            DiskSource::HuggingFace,
            5.82,
            "Q6_K",
        );
        let s = size_for_machine(&laptop(20.0), Vec::new(), &[file], None, &[]);
        assert_eq!(s.on_disk.len(), 1, "the card itself is still listed");
        assert_eq!(catalogue_pick(&s.pick).0, GEMMA4_12B);
        assert_eq!(also_have_of(&s.pick), None);
    }

    #[test]
    fn a_file_on_disk_counts_only_for_a_server_that_could_serve_it() {
        // An LM Studio download that fits the card at 32k with an f16 cache.
        let file = on_disk_file(
            "lmstudio-community/Qwen2.5-7B-Instruct-GGUF/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
            DiskSource::LmStudio,
            4.36,
            "Q4_K_M",
        );
        let pick_with = |url: Option<&str>| {
            size_for_machine(
                &laptop(20.0),
                Vec::new(),
                std::slice::from_ref(&file),
                url,
                &[],
            )
            .pick
        };
        for url in [None, Some("http://127.0.0.1:1234")] {
            assert_eq!(
                also_have_of(&pick_with(url)),
                Some(file.name.as_str()),
                "{url:?}"
            );
        }
        // Connected to an Ollama, to a port PM can't place, or to an LM Studio on ANOTHER machine:
        // no use to it however well it fits.
        for url in [
            "http://127.0.0.1:11434",
            "http://127.0.0.1:9999",
            "http://192.168.1.20:1234",
        ] {
            assert_eq!(also_have_of(&pick_with(Some(url))), None, "{url}");
        }

        assert!(runner_can_serve(
            DiskSource::HuggingFace,
            Some("http://127.0.0.1:8080")
        ));
        assert!(runner_can_serve(
            DiskSource::Folder,
            Some("http://localhost:8080")
        ));
        assert!(runner_can_serve(
            DiskSource::Ollama,
            Some("http://[::1]:11434")
        ));
        assert!(!runner_can_serve(
            DiskSource::Ollama,
            Some("http://127.0.0.1:8080")
        ));
        assert!(runner_can_serve(
            DiskSource::Ollama,
            Some("http://127.0.0.1:11434")
        ));
        assert!(!runner_can_serve(
            DiskSource::LmStudio,
            Some("http://127.0.0.1:11434")
        ));
        // The right port on another computer is still another computer: a file on this disk is no
        // use to it, and "it shows up by itself once Ollama is connected" would never come true.
        for (source, url) in [
            (DiskSource::Ollama, "http://192.168.1.20:11434"),
            (DiskSource::HuggingFace, "http://10.0.0.5:8080"),
            (DiskSource::LmStudio, "https://models.example.com:1234"),
        ] {
            assert!(!runner_can_serve(source, Some(url)), "{url}");
        }
    }

    #[test]
    fn a_server_that_cannot_say_which_build_it_loaded_is_judged_on_the_heaviest() {
        // LM Studio and llama-server have no `/api/tags`, so a served `qwen2.5-7b-instruct` could be
        // any of the catalogue's builds. With 8 GB free and no card the catalogue pick is its
        // Q4_K_M; judged generously, the served copy borrowed that Q4_K_M's figures and became the
        // pick "you already have" — whatever was really loaded. Judged on the Q8_0, it does not fit.
        let lm_studio = || ServedProbe {
            id: "qwen2.5-7b-instruct".to_string(),
            tag: None,
            served_ctx: None,
            resident: None,
        };
        let s = size_for_machine(
            &no_gpu(8.0),
            vec![lm_studio()],
            &[],
            Some("http://127.0.0.1:1234"),
            &[],
        );
        let own = owned_named(&s, "qwen2.5-7b-instruct");
        assert!(!own.measured);
        assert_eq!(own.config, None);
        let (repo, quant, _) = catalogue_pick(&s.pick);
        assert_eq!((repo, quant), (QWEN_7B, Some(fit::Quant::Q4_K_M)));

        // Where even the heaviest build fits, that is the one it is judged at.
        let s = size_for_machine(
            &card(24.0, Some(1008.0), 48.0),
            vec![lm_studio()],
            &[],
            Some("http://127.0.0.1:1234"),
            &[],
        );
        let own = owned_named(&s, "qwen2.5-7b-instruct");
        assert_eq!(
            own.config.as_ref().and_then(|c| c.quant),
            Some(fit::Quant::Q8_0)
        );
    }

    #[test]
    fn without_a_graphics_card_the_pick_must_clear_the_background_floor() {
        assert_eq!(
            pick_on(&no_gpu(3.0)),
            Pick::Nothing {
                reason: NoPick::TooLittleMemory,
                basis: PickBasis::System,
                system_fallback: false,
            }
        );
        for (free, repo, quant) in [
            (4.5, "bartowski/gemma-2-2b-it-GGUF", fit::Quant::Q4_K_M),
            // The MoE reads only its 3.82B active parameters a token, so its Q3_K_M clears the floor
            // at 21.4 tok/s from RAM where every dense model past 9B cannot.
            (16.0, "unsloth/gemma-4-26B-A4B-it-GGUF", fit::Quant::Q3_K_M),
            // Not the 72B, which fits at Q3_K_M and would reply at about 1.1 tok/s.
            (56.0, "unsloth/Qwen3.6-35B-A3B-GGUF", fit::Quant::Q8_0),
        ] {
            let p = pick_on(&no_gpu(free));
            let (got, got_quant, _) = catalogue_pick(&p);
            assert_eq!((got, got_quant), (repo, Some(quant)), "{free} GB");
            match p {
                Pick::Catalogue { basis, rung, .. } => {
                    assert_eq!(basis, PickBasis::System);
                    assert_eq!(rung, Rung::Quality);
                }
                _ => unreachable!(),
            }
        }

        // 12 GB free: Qwen3.5 9B's best quant that fits is too slow from RAM, and its Q3_K_M is
        // not. Judging only the best quant threw the model away and picked something smaller.
        match pick_on(&no_gpu(12.0)) {
            Pick::Catalogue {
                repo, fit, rung, ..
            } => {
                assert_eq!(repo, "unsloth/Qwen3.5-9B-GGUF");
                assert_eq!(fit.quant, Some(fit::Quant::Q3_K_M));
                assert_eq!(rung, Rung::Speed);
            }
            other => panic!("expected a catalogue pick, got {other:?}"),
        }
    }

    #[test]
    fn the_pick_never_shrinks_as_memory_is_freed() {
        // On every basis, more free memory can only widen what fits: the pick's size must never go
        // down as it grows. It did off the card — 8 GB free picked Qwen2.5 7B, and 9 GB, where that
        // model's best quant became a too-slow Q5_K_M, picked a 3.9B.
        let size = |p: &Pick| match p {
            Pick::Catalogue { repo, .. } => entry(repo).parameters_b,
            _ => 0.0,
        };
        for (label, hw_at) in [
            ("no card", no_gpu as fn(f64) -> fit::FitHardware),
            ("shared", |free| fit::FitHardware {
                vram_gb: Some(12.0),
                unified_memory: true,
                ..no_gpu(free)
            }),
            ("8 GB card", laptop),
            ("12 GB card", |free| card(12.0, Some(504.0), free)),
        ] {
            let mut best = 0.0_f64;
            for half in 4..=128 {
                let free = f64::from(half) / 2.0;
                let got = size(&pick_on(&hw_at(free)));
                assert!(
                    got >= best,
                    "{label}: the pick shrank to {got}B at {free} GB"
                );
                best = got;
            }
        }
    }

    #[test]
    fn on_shared_memory_the_floor_applies_too() {
        // A 16 GB Mac with 8 GB free: Qwen2.5 7B at Q4_K_M clears the floor at 8.6 tok/s.
        let mac = fit::FitHardware {
            available_ram_gb: 8.0,
            vram_gb: Some(12.0),
            gpu_bandwidth_gbps: None,
            unified_memory: true,
        };
        match pick_on(&mac) {
            Pick::Catalogue {
                repo, fit, basis, ..
            } => {
                assert_eq!(repo, QWEN_7B);
                assert_eq!(fit.quant, Some(fit::Quant::Q4_K_M));
                assert_eq!(basis, PickBasis::Shared);
                assert_eq!(fit.speed_basis, Some(fit::SpeedBasis::Shared));
            }
            other => panic!("expected a catalogue pick, got {other:?}"),
        }
    }

    #[test]
    fn a_card_too_small_for_anything_says_so_and_whether_system_memory_would_do() {
        assert_eq!(
            pick_on(&card(2.0, Some(48.0), 14.0)),
            Pick::Nothing {
                reason: NoPick::NothingOnGpu,
                basis: PickBasis::Gpu,
                system_fallback: true,
            }
        );
    }

    #[test]
    fn every_pick_anywhere_is_runnable_and_downloadable() {
        // Swept over cards, shared memory and none, at every amount of free RAM the spec's table
        // uses and more: whatever the pick names, it must run as it says and Ollama must fetch it.
        let mut picks = 0usize;
        for vram in [
            None,
            Some(2.0),
            Some(4.0),
            Some(6.0),
            Some(7.96),
            Some(12.0),
            Some(24.0),
        ] {
            for unified in [false, true] {
                if unified && vram.is_none() {
                    continue;
                }
                for free in [
                    2.5, 3.0, 4.5, 6.0, 8.0, 10.0, 11.0, 13.4, 16.0, 20.0, 24.0, 32.0, 48.0, 56.0,
                ] {
                    let hw = fit::FitHardware {
                        available_ram_gb: free,
                        vram_gb: vram,
                        gpu_bandwidth_gbps: vram.map(|_| 300.0),
                        unified_memory: unified,
                    };
                    let label = format!("vram {vram:?} unified {unified} free {free}");
                    match pick_on(&hw) {
                        Pick::Catalogue {
                            repo,
                            tag,
                            fit,
                            download_gb,
                            basis,
                            ..
                        } => {
                            picks += 1;
                            assert!(better_fit::is_runnable(fit.verdict), "{label}: {fit:?}");
                            let e = entry(&repo);
                            let (want, _) = pull_target_for(e, fit.quant);
                            assert_eq!(want.as_deref(), Some(tag.as_str()), "{label}");
                            assert!(download_gb > 0.0, "{label}");
                            let mem = fit.est_memory_gb.unwrap();
                            assert!(mem <= fit::ram_budget_gb(&hw) + 1e-6, "{label}");
                            match basis {
                                PickBasis::Gpu => assert!(
                                    mem <= vram.unwrap() - fit::gpu_reserve_gb() + 1e-6,
                                    "{label}"
                                ),
                                PickBasis::Shared | PickBasis::System => assert!(
                                    fit::system_tokens_per_sec(
                                        e.active_parameters_b,
                                        fit.quant.unwrap()
                                    ) >= better_fit::background_floor_tps(),
                                    "{label}"
                                ),
                            }
                        }
                        Pick::Nothing { .. } => {}
                        Pick::Owned { .. } => panic!("{label}: nothing is owned"),
                    }
                }
            }
        }
        assert!(picks > 50, "the sweep must mostly produce picks ({picks})");
    }

    #[test]
    fn the_better_fit_notice_never_suggests_what_the_pick_would_refuse() {
        // Both roles on Qwen2.5 7B, 24 GB free: the 14B and the 25B gemma 4 MoE fit RAM comfortably
        // and are the kind of "upgrade" the notice used to volunteer, though neither can live on
        // this card. What it suggests is what the pick would choose.
        let qwen = format!("hf.co/{QWEN_7B}:Q5_K_M");
        let s = better_fit_suggestion(&laptop(24.0), &[], Some(qwen.clone()), Some(qwen))
            .expect("the pick is larger than what runs");
        assert_eq!(s.repo, GEMMA4_12B);

        // No card and 16 GB free, both roles on gemma 2 2b: the pick is the gemma 4 MoE at Q3_K_M,
        // quick enough from RAM for background work. Judged at its trained 262144 tokens it was a
        // halved context, and Qwen3.5 9B a Tight against gemma 2 2b's Comfortable, so the notice
        // named Qwen2.5 7B directly above a pick card naming gemma 4 26B — which this assertion
        // used to pin.
        let gemma = "hf.co/bartowski/gemma-2-2b-it-GGUF:Q8_0".to_string();
        let s = better_fit_suggestion(&no_gpu(16.0), &[], Some(gemma.clone()), Some(gemma))
            .expect("a larger model clears the floor here");
        assert_eq!(s.repo, "unsloth/gemma-4-26B-A4B-it-GGUF");
        assert_eq!(s.repo, catalogue_pick(&pick_on(&no_gpu(16.0))).0);
    }

    #[test]
    fn on_the_dev_laptop_the_notice_names_the_pick_at_every_amount_of_free_ram() {
        // Both roles on Qwen2.5 7B Q5_K_M, and the pick is gemma 4 12b, 11.91B against 7.62B — past
        // the notice's 15%. Judged on each model's trained-context fit, the notice was silent at 10
        // GB free (gemma 4 12b a halved context) and at 13.4 (a Tight against the 7B's Comfortable),
        // and named Qwen3.5 9B at 20, so it agreed with the pick card at 24 GB only.
        let qwen = format!("hf.co/{QWEN_7B}:Q5_K_M");
        assert!(entry(GEMMA4_12B).parameters_b >= entry(QWEN_7B).parameters_b * 1.15);
        for free in [10.0, 13.4, 20.0, 24.0] {
            let hw = laptop(free);
            assert_eq!(catalogue_pick(&pick_on(&hw)).0, GEMMA4_12B, "{free} GB");
            // With and without the served Q5_K_M counted as a copy the user has: it fits the card,
            // so on a real Ollama it is one.
            for copies in [Vec::new(), vec![QWEN_7B.to_string()]] {
                let s = better_fit_suggestion(&hw, &copies, Some(qwen.clone()), Some(qwen.clone()))
                    .unwrap_or_else(|| panic!("{free} GB, {copies:?}: the notice is silent"));
                assert_eq!(s.repo, GEMMA4_12B, "{free} GB, {copies:?}");
                assert!(!s.already_downloaded, "{free} GB, {copies:?}");
            }
        }

        // The baseline is judged at the same context. Llama 3.2 3B at its trained 131072 tokens is
        // a halved context with 10 GB free, which left the notice no baseline at all.
        let llama = "hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M".to_string();
        let trained = fit::fit(
            &local_catalog::entry_to_spec(entry("bartowski/Llama-3.2-3B-Instruct-GGUF")),
            &laptop(10.0),
        );
        assert_eq!(trained.verdict, fit::Verdict::HalvedContext);
        let s = better_fit_suggestion(&laptop(10.0), &[], Some(llama.clone()), Some(llama))
            .expect("the pick is a 12B");
        assert_eq!(s.repo, GEMMA4_12B);
    }

    #[test]
    fn the_notice_never_names_a_download_other_than_the_pick() {
        // Across the redesign's machines, with every pair of catalogue models on the two roles:
        // whenever the notice names a model to download — anything but a copy the user already has —
        // it is the very model the pick card beside it names. Judged at each model's trained context
        // it disagreed on most of these machines: the 12 GB card at 24 GB free named gemma 4 12b
        // over a 3B while the pick was Qwen2.5 14B, and no card at 12 GB free named Qwen2.5 7B over
        // Llama 3.2 3B while the pick was Qwen3.5 9B.
        let shared = |vram: f64, free: f64| fit::FitHardware {
            available_ram_gb: free,
            vram_gb: Some(vram),
            gpu_bandwidth_gbps: None,
            unified_memory: true,
        };
        let machines = [
            ("laptop, 10 free", laptop(10.0)),
            ("laptop, 13.4 free", laptop(13.4)),
            ("laptop, 20 free", laptop(20.0)),
            ("laptop, 24 free", laptop(24.0)),
            ("12 GB card, 24 free", card(12.0, Some(504.0), 24.0)),
            ("no GPU, 12 free", no_gpu(12.0)),
            ("no GPU, 16 free", no_gpu(16.0)),
            ("Mac 16 GB, 11 free", shared(12.0, 11.0)),
            ("Mac 32 GB, 20 free", shared(24.0, 20.0)),
        ];
        // Each catalogue model as a role would name it: its first tag Ollama can fetch.
        let tags: Vec<(String, String)> = local_catalog::catalog()
            .entries
            .iter()
            .filter_map(|e| {
                let tag = e.quants.iter().find_map(|q| q.ollama.clone())?;
                assert_eq!(
                    local_catalog::match_installed(&tag).map(|m| m.repo.as_str()),
                    Some(e.repo.as_str()),
                    "{tag} names its own entry"
                );
                Some((e.repo.clone(), tag))
            })
            .collect();
        let mut named = 0usize;
        for (label, hw) in machines {
            let pick = pick_on(&hw);
            let pick_repo = match &pick {
                Pick::Catalogue { repo, .. } => Some(repo.as_str()),
                Pick::Nothing { .. } => None,
                Pick::Owned { .. } => panic!("{label}: nothing is owned"),
            };
            for (chat_repo, chat) in &tags {
                for (bg_repo, bg) in &tags {
                    // Without a copy of anything, and with the two in use counted as copies the user
                    // has, which is what a server serving them makes them.
                    for copies in [Vec::new(), vec![chat_repo.clone(), bg_repo.clone()]] {
                        let Some(s) = better_fit_suggestion(
                            &hw,
                            &copies,
                            Some(chat.clone()),
                            Some(bg.clone()),
                        ) else {
                            continue;
                        };
                        if s.already_downloaded {
                            assert!(copies.contains(&s.repo), "{label}: {s:?}");
                            continue;
                        }
                        named += 1;
                        assert_eq!(
                            Some(s.repo.as_str()),
                            pick_repo,
                            "{label}: {chat} + {bg}, copies {copies:?}"
                        );
                    }
                }
            }
        }
        assert!(
            named > 100,
            "the sweep must mostly name something ({named})"
        );
    }

    #[test]
    fn the_notice_names_the_pick_or_the_copy_the_pick_would_use_and_nothing_else() {
        // One model on both roles, so nothing else shares the machine, plus copies the user has: none,
        // one served, or one served and one only on disk — the pair where the notice, taking the
        // larger, used to name a different copy from the pick, which takes the served one. Built
        // through `size_for_machine` exactly as the command builds them. Whatever the notice names is
        // the pick's own: its download, or the copy it points at.
        let machines = [
            ("laptop, 10 free", laptop(10.0)),
            ("laptop, 20 free", laptop(20.0)),
            ("12 GB card, 24 free", card(12.0, Some(504.0), 24.0)),
            ("no GPU, 12 free", no_gpu(12.0)),
            ("no GPU, 16 free", no_gpu(16.0)),
        ];
        let url = Some("http://127.0.0.1:11434");
        let cat = local_catalog::catalog();
        // Each catalogue model as Ollama would hold it: its first fetchable tag and that file's size.
        let held: Vec<(&local_catalog::CatalogEntry, String, String, f64)> = cat
            .entries
            .iter()
            .filter(|e| e.fit == local_catalog::FitClass::Computed)
            .filter_map(|e| {
                let q = e.quants.iter().find(|q| q.ollama.is_some())?;
                Some((e, q.ollama.clone()?, q.quant.clone(), q.file_gb))
            })
            .collect();
        let as_served =
            |(e, tag, quant, gb): &(&local_catalog::CatalogEntry, String, String, f64)| {
                ServedProbe {
                    served_ctx: Some(32768),
                    ..served(tag, (gb * 1e9) as u64, quant, e.parameters_b)
                }
            };
        let as_file = |(e, _, quant, gb): &(&local_catalog::CatalogEntry, String, String, f64)| {
            on_disk_file(
                &format!(
                    "{}:{}",
                    e.repo.rsplit('/').next().unwrap(),
                    quant.to_lowercase()
                ),
                DiskSource::Ollama,
                *gb,
                quant,
            )
        };
        let (mut named_pick, mut named_copy, mut disk_picks) = (0usize, 0usize, 0usize);
        for (label, hw) in machines {
            for inuse in &held {
                let bound = vec![inuse.1.clone(), inuse.1.clone()];
                for (i, a) in held.iter().enumerate() {
                    for b in held.iter().skip(i).map(Some).chain([None]) {
                        let mut serve = vec![as_served(inuse)];
                        if a.1 != inuse.1 {
                            serve.push(as_served(a));
                        }
                        let files: Vec<DiskModel> = b.into_iter().map(as_file).collect();
                        let pick = size_for_machine(&hw, serve, &files, url, &bound).pick;
                        if matches!(pick, Pick::Owned { served: false, .. }) {
                            disk_picks += 1;
                        }
                        let s = better_fit_suggestion(
                            &hw,
                            &notice_copy(&pick),
                            Some(inuse.1.clone()),
                            Some(inuse.1.clone()),
                        );
                        let Some(s) = s else { continue };
                        match &pick {
                            Pick::Owned { repo, .. } => {
                                assert!(s.already_downloaded, "{label}: {s:?} against {pick:?}");
                                assert_eq!(&s.repo, repo, "{label}");
                                named_copy += 1;
                            }
                            Pick::Catalogue { repo, .. } => {
                                assert!(!s.already_downloaded, "{label}: {s:?} against {pick:?}");
                                assert_eq!(&s.repo, repo, "{label}");
                                named_pick += 1;
                            }
                            Pick::Nothing { .. } => panic!("{label}: {s:?} with no pick"),
                        }
                    }
                }
            }
        }
        assert!(
            named_pick > 100,
            "the sweep must name downloads ({named_pick})"
        );
        assert!(
            named_copy > 100,
            "the sweep must name copies ({named_copy})"
        );
        // The disk half of each pair must reach the pick at all, or the pair proves nothing.
        assert!(
            disk_picks > 100,
            "a copy only on disk must be the pick ({disk_picks})"
        );

        // And the converse: with nothing but the model in use, a pick at least 15% larger is named,
        // even over a model PM would not run here (Phi-3.5 mini's full-width cache on the laptop's
        // card at 32k), and a pick within 15% is not.
        for (label, hw) in [("laptop", laptop(20.0)), ("no GPU", no_gpu(16.0))] {
            let Pick::Catalogue {
                repo: pick_repo, ..
            } = pick_on(&hw)
            else {
                panic!("{label} has a pick");
            };
            let pick_b = cat
                .entries
                .iter()
                .find(|e| e.repo == pick_repo)
                .unwrap()
                .parameters_b;
            for (e, tag, ..) in &held {
                let s = better_fit_suggestion(&hw, &[], Some(tag.clone()), Some(tag.clone()));
                if pick_b >= e.parameters_b * 1.15 {
                    assert_eq!(s.map(|s| s.repo), Some(pick_repo.clone()), "{label}: {tag}");
                } else {
                    assert_eq!(s, None, "{label}: {tag}");
                }
            }
        }
    }

    #[test]
    fn already_on_this_device_means_a_copy_the_pick_could_use() {
        // A role on gemma 3 4b, and the notice's suggestion is the pick, gemma 4 12b. An LM Studio
        // Q8_0 of it sits on disk under an Ollama on 11434: a file that server cannot load, and at
        // 11.8 GB one that would not fit the card if it could. Matched by repo alone, the notice
        // called PM's pick "already on this device" right above a pick card offering its download.
        let gemma = "gemma3:4b".to_string();
        let lm_studio = |quant: &str, gb: f64| {
            on_disk_file(
                &format!("{GEMMA4_12B}/gemma-4-12b-it-{quant}.gguf"),
                DiskSource::LmStudio,
                gb,
                quant,
            )
        };
        let usable = |url: &str, file: DiskModel| {
            notice_copy(
                &size_for_machine(
                    &laptop(20.0),
                    vec![served(&gemma, 3_338_801_804, "Q4_K_M", 4.3)],
                    &[file],
                    Some(url),
                    std::slice::from_ref(&gemma),
                )
                .pick,
            )
        };
        let notice = |copies: &[String]| {
            better_fit_suggestion(
                &laptop(20.0),
                copies,
                Some(gemma.clone()),
                Some(gemma.clone()),
            )
            .expect("a larger model fits the card")
        };

        let copies = usable("http://127.0.0.1:11434", lm_studio("Q8_0", 11.8));
        assert!(!copies.iter().any(|r| r == GEMMA4_12B), "{copies:?}");
        let s = notice(&copies);
        assert_eq!(s.repo, GEMMA4_12B);
        assert!(!s.already_downloaded);

        // The right server, but the wrong file: the Q8_0 does not fit the card at 32k.
        let copies = usable("http://127.0.0.1:1234", lm_studio("Q8_0", 11.8));
        assert!(!copies.iter().any(|r| r == GEMMA4_12B), "{copies:?}");

        // A Q3_K_M LM Studio can serve, which runs on the card: that one is already here.
        let copies = usable("http://127.0.0.1:1234", lm_studio("Q3_K_M", 5.3));
        assert!(copies.iter().any(|r| r == GEMMA4_12B), "{copies:?}");
        let s = notice(&copies);
        assert_eq!(s.repo, GEMMA4_12B);
        assert!(s.already_downloaded);
    }

    #[test]
    fn a_served_library_tag_still_hides_its_own_copy_on_disk() {
        // `qwen2.5:latest` now matches its repo, while the same name on disk still matches nothing.
        // Keyed on the repo alone, the two stopped de-duplicating and the model showed twice.
        let s = size_for_machine(
            &laptop(20.0),
            vec![served("qwen2.5:latest", 4_683_087_332, "Q4_K_M", 7.6)],
            &[on_disk_file(
                "qwen2.5:latest",
                DiskSource::Ollama,
                4.68,
                "Q4_K_M",
            )],
            Some("http://127.0.0.1:11434"),
            &[],
        );
        assert_eq!(s.installed[0].matched_repo.as_deref(), Some(QWEN_7B));
        assert!(s.on_disk.is_empty(), "{:?}", s.on_disk.len());
        let keys = served_keys(&s.installed);
        assert!(keys.iter().any(|k| k == "qwen2.5:latest"));
        assert!(keys.iter().any(|k| k == QWEN_7B));
    }

    #[test]
    fn a_role_set_to_a_model_puts_it_first_among_the_ones_you_have() {
        // No card, 6.5 GB free: the pick would be Qwen3.5 4B (4.21B), and both of these are within
        // 15% of it and clear the floor, so both qualify. Unbound, the larger one wins; once a role
        // uses the smaller one, it does — matched case-insensitively, as the server's ids are. The
        // Qwen tag's size is the manifest total Hugging Face serves for it.
        let both = || {
            vec![
                served(
                    "hf.co/unsloth/Qwen3.5-4B-GGUF:Q4_K_M",
                    3_413_361_504,
                    "Q4_K_M",
                    4.21,
                ),
                served("gemma3:4b", 3_338_801_804, "Q4_K_M", 4.3),
            ]
        };
        let id = |bound: &[String]| match size_for_machine(
            &no_gpu(6.5),
            both(),
            &[],
            Some("http://127.0.0.1:11434"),
            bound,
        )
        .pick
        {
            Pick::Owned { id, .. } => id,
            other => panic!("expected an owned pick, got {other:?}"),
        };
        assert_eq!(id(&[]), "hf.co/unsloth/Qwen3.5-4B-GGUF:Q4_K_M");
        assert_eq!(id(&["GEMMA3:4b".to_string()]), "gemma3:4b");
    }

    #[test]
    fn the_pick_on_every_other_machine_in_the_redesign_table() {
        // The rest of the table the redesign was simulated against, re-run with every model's KV
        // cache sized from its own attention geometry and the pick judged at the 32768 PM runs it
        // at, so a change to the rule shows up as a changed row here rather than as a surprise on
        // someone's machine.
        let shared = |vram: f64, free: f64| fit::FitHardware {
            available_ram_gb: free,
            vram_gb: Some(vram),
            gpu_bandwidth_gbps: None,
            unified_memory: true,
        };
        use fit::KvCache::{F16 as KV_F16, Q8_0 as KV_Q8};
        use fit::Quant::*;
        for (label, hw, repo, quant, ctx, kv, gb) in [
            (
                "4 GB card, 10 free — the reserve band",
                card(4.0, Some(192.0), 10.0),
                "bartowski/gemma-2-2b-it-GGUF",
                Q6_K,
                8192,
                KV_Q8,
                2.84,
            ),
            (
                // Phi 3.5 mini was this row at 131072 and 4.85 GB, sized 13x too small: with no
                // grouped-query attention its cache alone is 6.4 GB at q8_0 at 32k.
                "6 GB card, 12 free",
                card(6.0, Some(288.0), 12.0),
                QWEN_7B,
                Q3_K_M,
                32768,
                KV_Q8,
                4.98,
            ),
            (
                "12 GB card, 24 free",
                card(12.0, Some(504.0), 24.0),
                "bartowski/Qwen2.5-14B-Instruct-GGUF",
                Q3_K_M,
                32768,
                KV_Q8,
                10.53,
            ),
            (
                "16 GB card, 32 free",
                card(16.0, Some(448.0), 32.0),
                "unsloth/gemma-4-26B-A4B-it-GGUF",
                Q3_K_M,
                32768,
                KV_F16,
                14.38,
            ),
            (
                "24 GB card, 48 free",
                card(24.0, Some(1008.0), 48.0),
                "unsloth/Qwen3.6-35B-A3B-GGUF",
                Q4_K_M,
                32768,
                KV_F16,
                22.64,
            ),
        ] {
            let p = pick_on(&hw);
            match &p {
                Pick::Catalogue { repo: r, fit, .. } => {
                    assert_eq!(r, repo, "{label}");
                    assert_eq!(fit.quant, Some(quant), "{label}");
                    assert_eq!(fit.context, Some(ctx), "{label}");
                    assert_eq!(fit.kv, kv, "{label}");
                    assert_eq!(fit.est_memory_gb, Some(gb), "{label}");
                }
                other => panic!("{label}: expected a catalogue pick, got {other:?}"),
            }
        }
        let phi = "bartowski/Phi-3.5-mini-instruct-GGUF";
        assert_ne!(
            catalogue_pick(&pick_on(&card(6.0, Some(288.0), 12.0))).0,
            phi
        );

        // The 24 GB card's MoE, whose speed estimate (about 479) is the one that needs its caveat.
        match pick_on(&card(24.0, Some(1008.0), 48.0)) {
            Pick::Catalogue { fit, .. } => {
                assert_eq!(fit.verdict, fit::Verdict::Tight);
                assert!((fit.est_tokens_per_sec.unwrap() - 479.0).abs() < 1.0);
            }
            other => panic!("expected a catalogue pick, got {other:?}"),
        }

        assert!(matches!(
            pick_on(&no_gpu(2.5)),
            Pick::Nothing {
                reason: NoPick::TooLittleMemory,
                ..
            }
        ));
        for (label, hw, repo, quant) in [
            (
                "no GPU, 6 free",
                no_gpu(6.0),
                "unsloth/Qwen3.5-4B-GGUF",
                Q3_K_M,
            ),
            ("no GPU, 8 free", no_gpu(8.0), QWEN_7B, Q4_K_M),
            (
                "no GPU, 9 free",
                no_gpu(9.0),
                "unsloth/Qwen3.5-9B-GGUF",
                Q3_K_M,
            ),
            (
                "no GPU, 24 free",
                no_gpu(24.0),
                "unsloth/Qwen3.6-35B-A3B-GGUF",
                Q3_K_M,
            ),
            (
                "Mac 16 GB, 6 free",
                shared(12.0, 6.0),
                "unsloth/Qwen3.5-4B-GGUF",
                Q3_K_M,
            ),
            (
                "Mac 16 GB, 11 free",
                shared(12.0, 11.0),
                "unsloth/Qwen3.5-9B-GGUF",
                Q3_K_M,
            ),
            (
                "Mac 32 GB, 20 free",
                shared(24.0, 20.0),
                "unsloth/Qwen3.6-35B-A3B-GGUF",
                Q3_K_M,
            ),
            // An integrated GPU shares the RAM it reports, so the floor applies to it too.
            (
                "iGPU, 10 free",
                shared(2.0, 10.0),
                "unsloth/Qwen3.5-9B-GGUF",
                Q3_K_M,
            ),
        ] {
            let p = pick_on(&hw);
            let (r, q, _) = catalogue_pick(&p);
            assert_eq!((r, q), (repo, Some(quant)), "{label}");
        }
    }

    // ---- "runs from system memory" ----

    /// An `/api/ps` row for a model the server holds `size_gb` of, `size_vram_gb` of it on the card.
    fn loaded(size_gb: f64, size_vram_gb: f64) -> openai_compat::ResidentModel {
        openai_compat::ResidentModel {
            model: "qwen2.5:7b-instruct-q6_K".to_string(),
            size_gb,
            size_vram_gb,
            context_length: Some(32768),
        }
    }

    /// The user's own Qwen2.5 7B file of `gb` GB, at the 32768 its server was seen loading it with:
    /// the spec `served_spec` builds for a measured row.
    fn own_qwen(gb: f64, quant: fit::Quant) -> fit::ModelSpec {
        fit::ModelSpec {
            target_context: 32768,
            candidates: vec![fit::QuantCandidate {
                quant,
                weight_gb: gb,
            }],
            projector_gb: Some(0.0),
            ..local_catalog::entry_to_spec(entry(QWEN_7B))
        }
    }

    #[test]
    fn a_spill_off_the_card_is_claimed_only_where_pm_can_show_one() {
        // 5.82 GB of Q6_K: 7.25 GB with a q8_0 cache at 32k, 8.07 with an f16 one.
        let q6 = own_qwen(5.82, fit::Quant::Q6_K);
        // 15.2 GB of F16, past the laptop's card at any cache.
        let f16 = own_qwen(15.2, fit::Quant::F16);
        let hw = laptop(20.0);

        // Loaded, with the server putting part or all of it in system memory: it spills, whatever
        // the estimate says — `size_vram` is a floor, so short of `size` is proof.
        assert!(spills_gpu(&hw, Some(&loaded(8.0, 5.5)), None));
        assert!(spills_gpu(&hw, Some(&loaded(8.0, 5.5)), Some(&q6)));
        // Nothing on the card is the server not using it, which is its own fact.
        assert!(!spills_gpu(&hw, Some(&loaded(8.0, 0.0)), None));
        assert!(card_unused(&hw, Some(&loaded(2.6, 0.0))));
        assert!(!card_unused(&hw, Some(&loaded(8.0, 5.5))));
        assert!(!card_unused(&hw, None));
        assert!(!card_unused(&no_gpu(20.0), Some(&loaded(2.6, 0.0))));
        // Loaded wholly on the card: the server's word stands over any estimate. A floor never
        // proves a fit, but a load the server reports on the card is not one PM may call spilled.
        assert!(spills_gpu(&hw, None, Some(&f16)));
        assert!(!spills_gpu(&hw, Some(&loaded(16.2, 16.2)), Some(&f16)));
        // A sliver short is not a spill, and a row claiming more on the card than in all, or
        // nothing at all, is no evidence of one.
        assert!(!spills_gpu(&hw, Some(&loaded(6.0, 5.8)), None));
        assert!(!spills_gpu(&hw, Some(&loaded(5.0, 5.5)), None));
        assert!(!spills_gpu(&hw, Some(&loaded(0.0, 0.0)), None));

        // Not known to be loaded: only the user's own file at its served window, and only when it
        // outgrows the card at q8_0 by more than the estimate's error band.
        assert!(
            !spills_gpu(&hw, None, Some(&q6)),
            "7.25 GB at q8_0 sits on a 7.96 GB card"
        );
        let small_card = card(6.0, Some(288.0), 20.0);
        assert!(
            spills_gpu(&small_card, None, Some(&q6)),
            "7.25 GB at q8_0 is past a 6 GB card by more than the band"
        );
        // Unmeasured: the catalogue's guess at the file proves nothing, on any card.
        assert!(!spills_gpu(&small_card, None, None));
        assert!(!spills_gpu(&card(2.0, None, 20.0), None, None));

        // No dedicated card: nothing to spill off, whatever the server or the estimate says.
        let mac = fit::FitHardware {
            unified_memory: true,
            ..card(6.0, None, 20.0)
        };
        for hw in [mac, no_gpu(20.0)] {
            assert!(
                !spills_gpu(&hw, Some(&loaded(8.0, 0.0)), Some(&f16)),
                "{hw:?}"
            );
            assert!(!spills_gpu(&hw, None, Some(&f16)), "{hw:?}");
        }
    }

    #[test]
    fn a_served_model_is_said_to_spill_from_the_server_or_its_own_measured_file() {
        // The dev laptop at 20 GB free, an Ollama serving Qwen2.5 7B Q6_K (5.82 GiB per
        // `/api/tags`) at a proven 32768. The row's fit takes an f16 cache because free RAM allows
        // it, which puts it past the card — the ground the tab used to say "runs from system memory"
        // on. With a q8_0 cache, which PM's own setup steps can tell Ollama to use for every model,
        // it sits on the card.
        let id = "qwen2.5:7b-instruct-q6_K";
        let probe = || ServedProbe {
            served_ctx: Some(32768),
            ..served(id, 6_249_177_416, "Q6_K", 7.62)
        };
        let row = |probe: ServedProbe, hw: &fit::FitHardware| {
            size_for_machine(hw, vec![probe], &[], Some("http://127.0.0.1:11434"), &[])
                .installed
                .remove(0)
        };
        let hw = laptop(20.0);
        let r = row(probe(), &hw);
        assert!(r.measured);
        assert_eq!(r.fit.kv, fit::KvCache::F16);
        assert_eq!(r.fit.speed_basis, Some(fit::SpeedBasis::System));
        assert!(!r.spills_gpu, "{:?}", r.fit);

        // The server saying it put part of it in system memory settles it.
        let split = ServedProbe {
            resident: Some(loaded(8.1, 6.4)),
            ..probe()
        };
        let r = row(split, &hw);
        assert!(r.spills_gpu);
        assert_eq!(serde_json::to_value(&r).unwrap()["spills_gpu"], true);

        // On a 6 GB card the same file outgrows the card even at q8_0 — but only once PM has seen
        // the window it is served with; before that the row is the catalogue's guess.
        let small_card = card(6.0, Some(288.0), 20.0);
        assert!(row(probe(), &small_card).spills_gpu);
        let unproven = ServedProbe {
            served_ctx: None,
            ..probe()
        };
        let r = row(unproven, &small_card);
        assert!(!r.measured);
        assert!(!r.spills_gpu);
        assert_eq!(serde_json::to_value(&r).unwrap()["spills_gpu"], false);

        // A server on another machine runs on that machine's memory, whatever this card is.
        let remote = |probe: ServedProbe| {
            size_for_machine(
                &small_card,
                vec![probe],
                &[],
                Some("https://gpu-box.example.ts.net"),
                &[],
            )
            .installed
            .remove(0)
        };
        assert!(!remote(probe()).spills_gpu);
        assert!(
            !remote(ServedProbe {
                resident: Some(loaded(8.1, 0.0)),
                ..probe()
            })
            .spills_gpu
        );
        assert!(
            !remote(ServedProbe {
                resident: Some(loaded(8.1, 0.0)),
                ..probe()
            })
            .card_unused
        );

        // On this computer, a server holding nothing of it on the card isn't using the card: that,
        // and not a spill, is what the row says, and the JSON carries it.
        let cpu_only = row(
            ServedProbe {
                resident: Some(loaded(6.2, 0.0)),
                ..probe()
            },
            &hw,
        );
        assert!(cpu_only.card_unused);
        assert!(!cpu_only.spills_gpu);
        assert_eq!(
            serde_json::to_value(&cpu_only).unwrap()["card_unused"],
            true
        );
    }
}
