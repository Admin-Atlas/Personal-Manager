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
    BACKGROUND_ROUTING_KEY, CHAT_ROUTING_KEY, LOCAL_BACKGROUND_MODEL_KEY, LOCAL_BASE_URL_KEY,
    LOCAL_CHAT_MODEL_KEY,
};
use crate::local_slot::{classify_ip, posture_for, EndpointClass, PostureVerdict};
use crate::{
    better_fit, db, fit, hardware, local_catalog, local_disk, openai_compat, paths, secrets,
    AppState,
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
    secrets::clear_local_llm_endpoint_token()?;
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

#[tauri::command]
pub fn set_local_llm_token(token: String) -> Result<()> {
    secrets::set_local_llm_endpoint_token(&token)
}

#[tauri::command]
pub fn clear_local_llm_token() -> Result<()> {
    secrets::clear_local_llm_endpoint_token()
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
    /// means the role goes to cloud, and the cloud model is the true answer for that row.
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
    // One connection for every setting this needs, dropped before the first await.
    let (base_url, configured, chat_local_model, background_local_model) = {
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
        (base_url, configured, chat, background)
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
        });
    }

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
                if ok {
                    let mut models: Vec<&str> = [
                        background_local_model.as_deref(),
                        chat_local_model.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    // One role usually, and very often the same model on both.
                    models.dedup();
                    for model in models {
                        if let Some(info) =
                            openai_compat::probe_proven_window(&base_url, model, tok).await
                        {
                            app.state::<AppState>()
                                .local_ai
                                .cache_window(&base_url, model, info);
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
        match window_for(background_local_model.as_deref())
            .or_else(|| window_for(chat_local_model.as_deref()))
        {
            Some(w) => (
                Some(w.tokens),
                w.source.is_proven(),
                Some(w.source.as_str().to_string()),
            ),
            None => (None, false, None),
        }
    };

    Ok(LocalLlmStatus {
        configured: true,
        reachable,
        in_cooldown,
        cooldown_remaining_s,
        probed_now: probe_now,
        chat_local_model,
        background_local_model,
        served_window,
        served_window_proven,
        window_source,
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

    // Which curated models are already downloaded (#449) — a suggestion the user can act on for free.
    //
    // Both rungs, because the crawl alone is not enough to answer this: on a packaged Linux install
    // it cannot read Ollama's store, so every model the user has pulled reads `on_disk: false` and
    // this notice cheerfully recommends downloading something already sitting on the disk. What the
    // endpoint serves is the second rung, and for Ollama it IS the store — `/v1/models` lists what
    // has been pulled, not what is resident. Best-effort: the gate, the keychain or the server being
    // unavailable degrades to the crawl's answer rather than failing a passive notice.
    let mut on_disk: Vec<String> = disk_scan(&app)
        .await
        .models
        .iter()
        .filter_map(|m| local_catalog::match_installed(&m.name).map(|e| e.repo.clone()))
        .collect();
    if let Endpoint::Ready(base_url, token) = configured_endpoint(&app)
        .await
        .unwrap_or(Endpoint::Unconfigured)
    {
        for id in openai_compat::probe(&base_url, token.as_ref().map(|s| s.expose()))
            .await
            .unwrap_or_default()
        {
            if let Some(entry) = local_catalog::match_installed(&id) {
                on_disk.push(entry.repo.clone());
            }
        }
    }

    let candidates: Vec<better_fit::Candidate> = cat
        .entries
        .iter()
        .filter(|e| e.fit == local_catalog::FitClass::Computed)
        .map(|e| better_fit::Candidate {
            repo: e.repo.clone(),
            display_name: e.display_name.clone(),
            parameters_b: e.parameters_b,
            verdict: fit::fit(&local_catalog::entry_to_spec(e), &fit_hw).verdict,
            on_disk: on_disk.iter().any(|r| r == &e.repo),
        })
        .collect();

    // The baseline is whatever the user already runs — the BEST of it, so someone with a large chat
    // model isn't nagged about something that only beats their small background one.
    let assigned: Vec<better_fit::Candidate> = [chat_model, background_model]
        .into_iter()
        .flatten()
        .filter(|m| !m.trim().is_empty())
        .filter_map(|m| local_catalog::match_installed(&m).map(|e| e.repo.clone()))
        .filter_map(|repo| candidates.iter().find(|c| c.repo == repo).cloned())
        .collect();

    Ok(better_fit::suggest(
        better_fit::baseline(assigned.iter()),
        &candidates,
    ))
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

    // Score the curated catalog.
    let cat = local_catalog::catalog();
    let mut curated: Vec<Recommendation> = cat
        .entries
        .iter()
        .map(|e| {
            // Honour the generator's judgment: an entry it marked fit-unknown is never silently
            // scored. When it is scored, also derive the faster GPU-resident config (if any).
            let (fit, gpu) = match e.fit {
                local_catalog::FitClass::Unknown => (
                    fit::unknown("PM can't estimate this model's fit.".to_string()),
                    fit::GpuFit::Single,
                ),
                local_catalog::FitClass::Computed => {
                    let spec = local_catalog::entry_to_spec(e);
                    let fit = fit::fit(&spec, &fit_hw);
                    let gpu = fit::gpu_fit(&spec, &fit_hw, &fit);
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
            Recommendation {
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
            }
        })
        .collect();
    curated.sort_by(|a, b| {
        verdict_rank(a.fit.verdict)
            .cmp(&verdict_rank(b.fit.verdict))
            .then(
                b.parameters_b
                    .partial_cmp(&a.parameters_b)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    // Models the configured endpoint already serves (best-effort — no endpoint is fine). A REFUSED
    // endpoint degrades the same way an unreachable one already does: the tab still renders its
    // hardware fit, catalog and on-disk models, only the served-models probe is skipped. Failing the
    // whole command because the endpoint's address moved would be a far bigger regression than the
    // missing section.
    let endpoint = configured_endpoint(&app).await?;
    let endpoint_configured = !matches!(endpoint, Endpoint::Unconfigured);
    let mut installed = Vec::new();
    // Whether the endpoint ANSWERED, which `installed` alone cannot say: an empty list is both "a
    // server with nothing in it" and "no server answered", and the panel below has to tell a
    // first-time installer apart from someone whose address is wrong. `probe` already separates
    // them — a runner with an empty store returns `Ok(vec![])`, which #790 taught `is_models_list`
    // to accept — so this needs no second request.
    let mut endpoint_answered = false;
    if let Endpoint::Ready(base_url, token) = endpoint {
        if let Ok(models) =
            openai_compat::probe(&base_url, token.as_ref().map(|s| s.expose())).await
        {
            endpoint_answered = true;
            for id in models {
                let (matched_repo, fit_result) = match local_catalog::match_installed(&id) {
                    Some(entry) => (
                        Some(entry.repo.clone()),
                        fit::fit(&local_catalog::entry_to_spec(entry), &fit_hw),
                    ),
                    None => (
                        None,
                        fit::unknown(
                            "This model isn't in PM's catalog, so its fit can't be estimated."
                                .to_string(),
                        ),
                    ),
                };
                installed.push(InstalledModel {
                    id,
                    matched_repo,
                    fit: fit_result,
                });
            }
        }
    }

    // Models sitting on disk that no endpoint currently serves (#449). Scored on their REAL on-disk
    // size rather than the catalog's figure for that quant — the point of the card is to describe the
    // file you actually have. De-duplicated against the served list so a model that is both
    // downloaded and loaded appears once, under the endpoint that serves it.
    let served_keys: Vec<String> = installed
        .iter()
        .map(|m| m.matched_repo.clone().unwrap_or_else(|| m.id.clone()))
        .collect();
    let disk = disk_scan(&app).await;
    let on_disk: Vec<OnDiskModel> = disk
        .models
        .iter()
        .filter(|m| !already_served(m, &served_keys))
        .map(|m| {
            let matched = local_catalog::match_installed(&m.name);
            let fit = score_on_disk(m, matched, &fit_hw);
            OnDiskModel {
                name: m.name.clone(),
                source: m.source,
                path: m.path.clone(),
                size_gb: m.size_gb,
                sidecar_gb: m.sidecar_gb,
                quant: m.quant.clone(),
                shards: m.shards,
                matched_repo: matched.map(|e| e.repo.clone()),
                fit,
            }
        })
        .collect();

    // Rescan cadence — read-only in PR4 (the Local AI tab sets it and stamps the seen version in PR5).
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
        disk_truncated: disk.truncated,
        scan_dir: scan_dir_setting(&app),
        terms_accepted,
    })
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

/// Score an on-disk model against this machine, using the REAL file size on disk as the weight term.
///
/// Per #449's rules a file PM can't characterise is never guessed at: a name that matches no catalog
/// entry, or a quant label that isn't one PM knows, comes back `unknown` with the reason said plainly.
/// When both are known the catalog supplies the architecture, active-parameter count and context
/// window, while the single quant candidate carries the measured on-disk size.
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

fn score_on_disk(
    model: &local_disk::DiskModel,
    matched: Option<&local_catalog::CatalogEntry>,
    hw: &fit::FitHardware,
) -> fit::FitResult {
    let Some(entry) = matched else {
        return fit::unknown(
            "This model isn't in PM's catalog, so its fit can't be estimated.".to_string(),
        );
    };
    // Two different situations, and collapsing them throws away real information. PM may have found
    // no quantization at all, or know exactly which one the file is and have no weight for it. Only
    // the first is honestly "couldn't tell" — and since the on-disk weight is MEASURED, the second is
    // worth naming so it reads as a gap in PM rather than a defect in the file.
    let quant = match model.quant.as_deref() {
        None => return fit::unknown(UNREADABLE_QUANT.to_string()),
        Some(label) => match fit::Quant::from_label(label) {
            Some(quant) => quant,
            None => {
                return match safe_quant_label(label) {
                    Some(shown) => fit::unknown(format!(
                        "PM doesn't have a size for the {shown} quantization yet, so its fit can't \
                         be estimated."
                    )),
                    None => fit::unknown(UNREADABLE_QUANT.to_string()),
                };
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
    fit::fit(&spec, hw)
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
    let Some(row) = chosen.and_then(|c| {
        entry
            .quants
            .iter()
            .find(|q| fit::Quant::from_label(&q.quant) == Some(c))
    }) else {
        return (None, false);
    };
    (row.ollama.clone(), row.sharded)
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
    /// The crawl hit its bound, so `on_disk` is a prefix rather than everything on disk.
    pub disk_truncated: bool,
    /// The extra folder the crawl includes, when one is set.
    pub scan_dir: Option<String>,
    /// Licence ids the user has already read and accepted, so a second Gemma does not re-ask.
    pub terms_accepted: Vec<String>,
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
}
