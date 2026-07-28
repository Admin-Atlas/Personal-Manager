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
            Some(format!(
                "Couldn't reach the endpoint ({}).",
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
pub fn set_local_llm_role_model(
    state: State<'_, AppState>,
    role: String,
    model: String,
) -> Result<()> {
    let key = role_model_key(&role)?;
    let conn = state.conn()?;
    if model.trim().is_empty() {
        db::delete_setting(&conn, key)?;
    } else {
        db::set_setting(&conn, key, model.trim())?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_local_llm_routing(state: State<'_, AppState>, role: String, pref: String) -> Result<()> {
    let key = role_routing_key(&role)?;
    // Validate the preference string so an unknown value can't silently read back as "cloud".
    if !matches!(pref.as_str(), "cloud" | "local" | "local-then-cloud") {
        return Err(Error::Other(format!("unknown routing preference '{pref}'")));
    }
    let conn = state.conn()?;
    db::set_setting(&conn, key, &pref)?;
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
    let base_url = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, LOCAL_BASE_URL_KEY)?
    };
    let Some(base_url) = base_url else {
        return Err(Error::Other("no local endpoint is configured".into()));
    };
    let token = secrets::get_local_llm_endpoint_token()?;
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
    let base_url = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, LOCAL_BASE_URL_KEY)?
    };
    let Some(base_url) = base_url else {
        return Err(Error::Other("no local endpoint is configured".into()));
    };
    let token = secrets::get_local_llm_endpoint_token()?;
    openai_compat::pull_ollama_model(&base_url, &model, token.as_ref().map(|s| s.expose()), |p| {
        let _ = on_event.send(p);
    })
    .await
    .map_err(|f| {
        Error::Other(format!(
            "couldn't download the model ({})",
            crate::error::truncate_detail(&f.detail)
        ))
    })
}

#[derive(Serialize)]
pub struct LocalLlmStatus {
    /// A base URL is configured.
    pub configured: bool,
    /// The endpoint answered a `/v1/models` probe on the last check (debounced — see below).
    pub reachable: bool,
    /// The host is resting inside its dead-host cooldown after repeated failures.
    pub in_cooldown: bool,
    /// Seconds left on the cooldown (0 when not in one).
    pub cooldown_remaining_s: u64,
    /// Whether the reachability figure came from a fresh probe this call, or is the last-known value
    /// (a probe was skipped by the debounce so a fast-polling UI can't spam the user's server).
    pub probed_now: bool,
}

/// A live status snapshot for the Local AI tab / the chat honesty surface (#297 PR5/PR6). Reads the
/// in-memory circuit-breaker state, and — at most once per [`tunables::HEALTH_PROBE_DEBOUNCE`] — runs
/// one `/v1/models` reachability probe so a fast UI poll can't hammer the user's server.
#[tauri::command]
pub async fn local_llm_status(app: AppHandle) -> Result<LocalLlmStatus> {
    let base_url = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, LOCAL_BASE_URL_KEY)?
    };
    let Some(base_url) = base_url else {
        return Ok(LocalLlmStatus {
            configured: false,
            reachable: false,
            in_cooldown: false,
            cooldown_remaining_s: 0,
            probed_now: false,
        });
    };

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
    let reachable = if probe_now {
        let token = secrets::get_local_llm_endpoint_token()?;
        openai_compat::probe(&base_url, token.as_ref().map(|s| s.expose()))
            .await
            .is_ok()
    } else {
        // No fresh probe this call; report "not in cooldown" as the best available liveness proxy.
        !in_cooldown
    };

    Ok(LocalLlmStatus {
        configured: true,
        reachable,
        in_cooldown,
        cooldown_remaining_s,
        probed_now: probe_now,
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
        available_ram_gb: hardware.available_ram_gb,
        vram_gb: hardware.vram_gb,
        gpu_bandwidth_gbps: hardware.gpu_bandwidth_gbps,
        unified_memory: hardware.unified_memory,
    };

    // Which curated models are already downloaded (#449) — a suggestion the user can act on for free.
    let on_disk: Vec<String> = disk_scan(&app)
        .await
        .models
        .iter()
        .filter_map(|m| local_catalog::match_installed(&m.name).map(|e| e.repo.clone()))
        .collect();

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
        available_ram_gb: hardware.available_ram_gb,
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
                install: e.install.clone(),
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

    // Models the configured endpoint already serves (best-effort — no endpoint is fine).
    let base_url = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::get_setting(&conn, LOCAL_BASE_URL_KEY)?
    };
    let endpoint_configured = base_url.is_some();
    let mut installed = Vec::new();
    if let Some(base_url) = base_url {
        let token = secrets::get_local_llm_endpoint_token()?;
        if let Ok(models) =
            openai_compat::probe(&base_url, token.as_ref().map(|s| s.expose())).await
        {
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
        disk_truncated: disk.truncated,
        scan_dir: scan_dir_setting(&app),
    })
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
/// repo when both matched, else on the runner's own name — an Ollama tag round-trips exactly, and a
/// file-based runner's `owner/repo/file.gguf` contains what the endpoint reports.
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
    // the catalog's figure. `Some(0.0)`, never `None`: a multimodal spec with `projector_gb: None`
    // is refused as fit-unknown (see `fit::fit_within`), and "no projector on disk" is a measurement,
    // not a gap. Leaving the catalog value here while `weight_gb` came from disk was the
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
    pub install: local_catalog::InstallHints,
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
    /// Which runners' model folders exist on this machine, so the UI can say "Ollama is here with
    /// nothing downloaded" rather than implying it isn't installed.
    pub disk_sources_present: Vec<local_disk::DiskSource>,
    /// The crawl hit its bound, so `on_disk` is a prefix rather than everything on disk.
    pub disk_truncated: bool,
    /// The extra folder the crawl includes, when one is set.
    pub scan_dir: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
