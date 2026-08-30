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
    better_fit, db, fit, hardware, local_catalog, local_disk, openai_compat, paths, residency,
    secrets, AppState,
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
    pub chat_loaded: Option<bool>,
    pub background_loaded: Option<bool>,
    /// PM itself handed this model back, on the user's own release policy. Only meaningful while the
    /// model is not loaded, and it is what separates "your server let it go" from "you asked PM to".
    pub chat_released: bool,
    pub background_released: bool,
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
            chat_answering: false,
            background_answering: false,
            chat_loaded: None,
            background_loaded: None,
            chat_released: false,
            background_released: false,
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
                if !ok {
                    // PM asked and got nothing back, so it no longer knows what this server holds.
                    // Without this the last reading stands for its whole TTL — and stands in the
                    // SAME footer as the "unreachable" line directly beneath it, which is a display
                    // contradicting itself rather than admitting it cannot see.
                    app.state::<AppState>().local_ai.clear_resident(&base_url);
                }
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
            loaded(chat_local_model.as_deref()),
            loaded(background_local_model.as_deref()),
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
        chat_released: released(chat_local_model.as_deref()),
        background_released: released(background_local_model.as_deref()),
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
        .map(|e| {
            // The whole result now, not just its verdict: the joint check below needs the footprint,
            // and throwing it away here is what made "does this still fit beside the other role's
            // model?" unanswerable.
            let f = fit::fit(&local_catalog::entry_to_spec(e), &fit_hw);
            better_fit::Candidate {
                repo: e.repo.clone(),
                display_name: e.display_name.clone(),
                parameters_b: e.parameters_b,
                verdict: f.verdict,
                footprint_gb: f.est_memory_gb,
                on_disk: on_disk.iter().any(|r| r == &e.repo),
            }
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

    let current = better_fit::baseline(assigned.iter());
    // What a suggestion has to share the machine with: the model on the role `baseline` did NOT
    // pick, which it otherwise drops on the floor. Without this PM can talk someone into a model
    // that fits only if the machine is holding nothing else — manufacturing the very swapping the
    // co-residency line beside it was added to describe.
    //
    // Both sides come from the catalogue scoring, so they are compared like with like. That does
    // over-state the model the user already has (the catalogue picks the best quant that fits, not
    // the file they downloaded), which can suppress a suggestion that would in fact have fitted.
    // That is the safe direction for something PM volunteers unprompted.
    let beside = current.and_then(|cur| {
        assigned
            .iter()
            .find(|c| c.repo != cur.repo)
            .and_then(|other| other.footprint_gb)
            .map(|footprint_gb| better_fit::Beside {
                footprint_gb,
                budget_gb: fit::ram_budget_gb(&fit_hw),
            })
    });
    Ok(better_fit::suggest(current, &candidates, beside))
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
    /// Total bytes the server placed for it, in GB.
    pub size_gb: f64,
    /// The share the server reports as being on the GPU, in GB. A FLOOR: it excludes the CUDA
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
        let policy = residency::ReleasePolicy::from_setting(
            db::get_setting(&conn, residency::RELEASE_POLICY_KEY)
                .ok()
                .flatten()
                .as_deref(),
        );
        let idle = residency::idle_after(
            db::get_setting(&conn, residency::RELEASE_IDLE_MINUTES_KEY)
                .ok()
                .flatten()
                .as_deref(),
        );
        let base_url = db::get_setting(&conn, LOCAL_BASE_URL_KEY).ok().flatten();
        (policy, idle, base_url)
    });
    if let Some((policy, idle, _)) = fresh {
        state.local_ai.cache_release_policy(policy, idle);
    }
    // `None` here means PM has never been able to read the policy, which is not the same as "the
    // default policy" — it must not act on a setting it has never seen.
    let Some((policy, idle_after)) = state.local_ai.cached_release_policy() else {
        return;
    };
    // Every input handed over whole, and the decision made in one place. Nothing is pre-checked
    // here: a caller that filters first and then asks makes the pure reducer's own gates unreachable,
    // which leaves the real decision spread across two files with the tested one contributing
    // nothing. `quiet_for` of `None` means no call has ever run, which zero expresses correctly —
    // zero is never past a quiet period.
    let inputs = residency::ReleaseInputs {
        policy,
        pm_loaded: !state.local_ai.pm_loaded_pairs().is_empty(),
        in_flight: state.local_ai.slot.in_flight(),
        holds: state.local_ai.slot.holds(),
        quiet_for: state
            .local_ai
            .slot
            .quiet_for(std::time::Instant::now())
            .unwrap_or_default(),
        idle_after,
    };
    if !residency::should_release(&inputs) {
        return;
    }
    let Some(base_url) = fresh.and_then(|(_, _, b)| b) else {
        return;
    };
    // An endpoint that has already told PM it has no unload route never gets asked again. llama-server
    // and LM Studio have none, and neither does a proxy forwarding only `/v1` — without this latch the
    // scheduler posts at one of them every twenty seconds for the life of the process.
    if state.local_ai.has_no_unload_route(&base_url) {
        return;
    }
    let token = secrets::get_local_llm_endpoint_token()
        .ok()
        .flatten()
        .map(|s| s.expose().to_string());

    release_pm_models(&state, &base_url, token.as_deref()).await;
    crate::llm_gateway::ping_status(app);
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
/// worker and so may block safely. Every step is written to give up rather than to fail: a locked
/// vault, an absent state, an unreachable server and a slow one all end the same way, with PM
/// quitting.
///
/// Never records a health outcome. Housekeeping must not be evidence about the endpoint in either
/// direction, and at shutdown there is nobody left to tell anyway.
pub fn release_gpu_on_exit(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Read every setting up front and drop the guard before the first await — the DB mutex is not
    // reentrant, and this runs while the rest of the app is still alive.
    let policy = {
        let Ok(conn) = state.conn() else {
            return; // a locked or already-torn-down vault is not an error at this point
        };
        let policy = residency::ReleasePolicy::from_setting(
            db::get_setting(&conn, residency::RELEASE_POLICY_KEY)
                .ok()
                .flatten()
                .as_deref(),
        );
        policy
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
        let requests = releasable.iter().map(|(base_url, model)| {
            openai_compat::unload_model(base_url, model, token.as_deref(), false)
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
    Ok(ReleaseSettings {
        policy: policy.as_setting().to_string(),
        idle_minutes: idle.as_secs() / 60,
    })
}

/// Store the release policy. Round-tripped through the enum and the clamp so an unrecognised policy
/// or an out-of-range period cannot be persisted — both resolve to something PM can act on.
#[tauri::command]
pub fn set_local_release_policy(
    state: State<'_, AppState>,
    policy: String,
    idle_minutes: Option<u64>,
) -> Result<()> {
    let parsed = residency::ReleasePolicy::from_setting(Some(policy.as_str()));
    let conn = state.conn()?;
    db::set_setting(&conn, residency::RELEASE_POLICY_KEY, parsed.as_setting())?;
    if let Some(m) = idle_minutes {
        let clamped = m.clamp(residency::MIN_IDLE_MINUTES, residency::MAX_IDLE_MINUTES);
        db::set_setting(
            &conn,
            residency::RELEASE_IDLE_MINUTES_KEY,
            &clamped.to_string(),
        )?;
    }
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
        let tok = token.as_ref().map(|s| s.expose());
        // The real byte size of every model in the store. `None` for anything that is not an Ollama;
        // those fall back to the catalogue estimate, which is all PM ever had.
        let tags = openai_compat::ollama_tags(&base_url, tok).await;
        if let Ok(models) = openai_compat::probe(&base_url, tok).await {
            endpoint_answered = true;
            for id in models {
                let entry = local_catalog::match_installed(&id);
                // The window the server actually loaded it with, when it has been observed. Only a
                // PROVEN reading is used: an unproven one is either PM's own floor or the model's
                // trained capacity, and substituting either for the catalogue's figure would trade
                // one guess for another while looking like a measurement.
                let served_ctx = app
                    .state::<AppState>()
                    .local_ai
                    .cached_window(&base_url, &id)
                    .filter(|w| w.source.is_proven())
                    .map(|w| w.tokens);
                let tag = tags
                    .iter()
                    .flatten()
                    .find(|t| t.name.eq_ignore_ascii_case(&id));
                installed.push(InstalledModel {
                    id,
                    matched_repo: entry.map(|e| e.repo.clone()),
                    fit: score_served(entry, tag, served_ctx, &fit_hw),
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
    })
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

/// Score a model the endpoint is actually SERVING, using what the server will tell us about it.
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
///     Not a nicety: gemma-3-4b trains at 131072, so the catalogue's KV term for it is 4.07 GB —
///     44% of the entire estimate — for a window the server was never serving. #792 already ruled
///     that number unusable for the context meter, and it was still driving the memory estimate.
///
/// Falls back to the catalogue spec whenever either measurement is missing, which is exactly the
/// behaviour that shipped before this — never worse, and better wherever the server answers.
fn score_served(
    entry: Option<&local_catalog::CatalogEntry>,
    tag: Option<&openai_compat::OllamaTag>,
    served_ctx: Option<u32>,
    hw: &fit::FitHardware,
) -> fit::FitResult {
    let Some(entry) = entry else {
        return fit::unknown(
            "This model isn't in PM's catalog, so its fit can't be estimated.".to_string(),
        );
    };
    let mut spec = local_catalog::entry_to_spec(entry);
    if let Some(ctx) = served_ctx {
        spec.target_context = ctx;
    }
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
        }
    }
    fit::fit(&spec, hw)
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

/// Billions-of-bytes GB, matching every other size in this feature.
fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1e9
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

    fn installed_model(id: &str, gb: f64) -> InstalledModel {
        InstalledModel {
            id: id.to_string(),
            matched_repo: None,
            fit: fit::FitResult {
                verdict: fit::Verdict::Comfortable,
                quant: Some(fit::Quant::Q4_K_M),
                context: Some(32768),
                kv: fit::KvCache::F16,
                est_memory_gb: Some(gb),
                est_tokens_per_sec: Some(30.0),
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
        let guessed = score_served(Some(qwen), None, None, &hw);
        assert_eq!(guessed.quant, Some(fit::Quant::Q8_0));

        // With the server's own answer — 5.44 GB of Q5_K_M, loaded at the 32768 it really serves.
        let tag = openai_compat::OllamaTag {
            name: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M".to_string(),
            size_bytes: 5_444_833_987,
            quant: Some("Q5_K_M".to_string()),
        };
        let real = score_served(Some(qwen), Some(&tag), Some(32768), &hw);
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
        };
        assert_eq!(served_quant(&tag), Some(fit::Quant::Q4_K_M));

        // What the server says still wins when it says anything at all.
        let named = openai_compat::OllamaTag {
            quant: Some("Q5_K_M".to_string()),
            ..tag.clone()
        };
        assert_eq!(served_quant(&named), Some(fit::Quant::Q5_K_M));

        // And a tag that names nothing usable stays unknown rather than being invented.
        let bare = openai_compat::OllamaTag {
            name: "llama3.2:latest".to_string(),
            quant: None,
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
}
