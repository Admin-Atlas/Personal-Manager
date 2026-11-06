// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sealed-frame regression test (Mods spec §1.4, "What PR-1 ships from this", item 4). Never
//! shipped; CI builds and runs it on all three engines (`.github/workflows/sealed-frame.yml`), weekly
//! and on every PR that touches the mod runtime, because WebView2 updates itself on users' machines
//! and an engine change has to show up as a red job, not as a leak.
//!
//! It loads a hostile probe into `<iframe sandbox="allow-scripts" allow="">` the way ModFrame does,
//! through a `pmmod` scheme, in a window built by PM's own code: `engine.rs` (the sinkhole, the
//! browser args, the navigation guard, the download refusal, both init scripts and the engine
//! switches, through the same `seal_builder` and `apply_switches` PM's `build_window` calls) and
//! `headers.rs` (every response header), both included from `src-tauri/src/mods/` by path, with every
//! mitigation on, as in a release build. The scheme here mirrors PM's handler (`mods/runtime.rs`): the
//! main window only, GET only, `pmmod://localhost` exactly, and a page rendered only once, for its
//! one-time entry token.
//!
//! Counted in Rust, where the frame can't fake it:
//! - calls that reached the `canary` command (an escape into IPC);
//! - TCP connections and UDP datagrams that reached the loopback listeners (a network leak: fetch,
//!   images, preconnect, WebSocket, WebRTC's STUN probe, a navigation);
//! - connections PM's sinkhole absorbed (attempts the sinkhole stopped: not leaks), also noted as
//!   each frame opens and at two marks the `relaxed` frame asks for around its plain loads
//!   (`harness_mark`: the frame fires no load until the harness has had the first noted), so the
//!   report shows how many those loads sent there;
//! - every `pmmod` request, every entry page served, and every engine switch's outcome.
//!
//! It exits 0 only when nothing escaped or leaked, the engine is sealed (every expected switch took),
//! both probe frames got their port and ran the probe to the end, each frame's page rendered exactly
//! once, no navigation frame loaded a document that ran a script, the `relaxed` frame's plain loads
//! all ran with its CSP refusing none of them (so the network wall below the CSP is what they met),
//! and the probe saw what a sealed frame must (an opaque origin, the WebRTC script, no parent DOM,
//! no storage, no `eval`, no WebRTC). "Refusing none" is read from the frame's CSP violation
//! reports, so the `strict` frame, sealed the same way, must have reported its CSP refusing a load
//! to the leak listener: an engine whose reports never fire fails rather than reading as "none".
//! On Linux, at least one connection per plain load to a host the engine's URL parser took
//! (probe.js `open-parse`; loopback must parse) must have reached the sinkhole between the two
//! marks. That can't say which load each came from (a retry adds one, and a late retry of an
//! earlier load could too), so it proves the proxy was reached, not that every load reached it.
//! Anything else, the watchdog included, is a non-zero exit. The report goes to
//! `PM_SEALED_FRAME_OUT` (default `sealed-frame-report.json`).
//!
//! **The dead-port run** (`PM_SEALED_FRAME_DEAD_PORT=1`; Linux and Windows, as macOS has no proxy
//! and refuses it) measures PM's fallback for a sinkhole it couldn't bind. The run binds none: the
//! bind fails through PM's own `bind_retrying` and `sinkhole_unbound`, and the window's proxy is
//! `proxy_port`'s with no sinkhole, `127.0.0.1:1` (`DEAD_PROXY_PORT`), where nothing listens. That
//! an engine doesn't connect directly when its proxy refuses is the engines' documented behaviour,
//! and this run is what measures it (engine.rs `DEAD_PROXY_PORT`). Everything above still holds,
//! with three changes:
//! - the `sinkhole` switch must have failed, once, with exactly `sinkhole_unbound`'s report, and
//!   every other expected switch must have taken; the engine then reads as unsealed, as it must;
//! - a connection to the dead port must be refused, before the window opens and again at the end,
//!   or the run would measure whatever listens there;
//! - on Linux, the connections owed between the marks are the kernel's failed TCP connections
//!   (`AttemptFails` in `/proc/net/snmp`, which a refused connection adds one to), not the
//!   sinkhole's. That count is system-wide: it counts a failed connection to any port from any
//!   process, retries included. So a shortfall means something stopped some loads before the dead
//!   port, and a pass shows only that at least that many TCP connections failed while the loads
//!   ran, which fits the loads meeting the refusing port; the leak listener's count, which no frame
//!   can fake, is what says none got out. On Windows, as in the default run, whether the loads
//!   reached the proxy is unmeasured.
//!
//! The variable unset is the default run; any value but `1` is refused. A dead-port report says
//! `"deadPortRun": true`.
//!
//! What can't be observed here: a DNS lookup. `dns-prefetch` and preconnect to a name are stopped by
//! the proxy where there is one (it is handed the name instead of resolving it; a regression in it
//! shows up as a loopback hit), and by WKWebView's prefetching switches and content rule list. A
//! mod's documents carry `X-DNS-Prefetch-Control: off`, but WebKit stopped reading it in WebKitGTK
//! 2.48 and Apple's 7622, and before that it governed only the implicit prefetching of a link's
//! host, not `<link rel=dns-prefetch>` (engine.rs's `MACOS_DNS_PREFETCHING`). WebKitGTK's own
//! DNS-prefetching setting can't be read back since 2.48 (engine.rs), so it proves nothing either
//! way.
//!
//! What this doesn't run: PM's serve decision (`mods/runtime.rs`, which file goes out and which as a
//! document). The scheme here is a small mirror of it; runtime.rs's unit tests are what hold it.
//!
//! Three kinds of frame:
//! - `strict`: PM's headers exactly (`headers::file_headers`). The IPC and network probes run. Its
//!   CSP refuses every load it can name, so of its network probes only those the CSP can't stop
//!   (preconnect, prefetch) reach the engine's network wall.
//! - `relaxed`: the same, with the CSP's fetch directives opened and its sandbox kept, so a refusal
//!   can only come from the engine or Tauri (the second wall). The IPC probes, then plain loads
//!   (`OPEN_LOADS`: a fetch, an image and a WebSocket, in the clear and over TLS, each to the leak
//!   listener and to the IDNA host): the one place a load meets the wall below the CSP, the sinkhole
//!   on Linux and Windows and the content rule list on macOS, which has no sinkhole. One that gets
//!   through is a TCP hit. A load to a host the engine's URL parser refuses fails before any of
//!   that, as the IDNA host's do on ubuntu-latest's WebKitGTK and macos-latest's WebKit
//!   (`IDNA_HOST`). This is the only measurement of the macOS rule list's blocking; the switch's
//!   report only says it compiled.
//! - `nav`: PM's headers, no probes; one fresh frame per navigation target (`NAV_TARGETS`), which
//!   navigates itself there once it has its port. Main's `frame-src` refuses most of these before
//!   any network wall is reached.

// PM's own files, so the test measures what ships. Each names nothing in PM's crate, by design (see
// their headers). This app uses only part of each.
#[allow(dead_code)]
#[path = "../../../../src-tauri/src/mods/engine.rs"]
mod engine;
#[allow(dead_code)]
#[path = "../../../../src-tauri/src/mods/headers.rs"]
mod headers;

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::http::{Method, Request, Response, StatusCode};
use tauri::{AppHandle, Url, WebviewWindowBuilder};

use engine::{Platform, Sinkhole, SwitchOutcome};

/// The run fails if the harness hasn't reported by then.
const WATCHDOG: Duration = Duration::from_secs(240);
/// Trailing leaks (a late preconnect, a STUN retry) get this long to land before the count.
const SETTLE: Duration = Duration::from_millis(2500);
/// As in a release build: no dev-server exemptions anywhere.
const IS_DEV: bool = false;

/// Each frame's variant and the mod id it is served under.
const VARIANTS: [(&str, &str); 3] = [
    ("strict", "sealed.strict"),
    ("relaxed", "sealed.relaxed"),
    ("nav", "sealed.nav"),
];
/// The `nav` variant's id.
const NAV_ID: &str = VARIANTS[2].1;
/// The targets the harness sends `nav` frames to, in order (harness.js `NAV_TARGETS`).
const NAV_TARGETS: [&str; 6] = [
    "own-entry",
    "other-entry",
    "data-doc",
    "blob-doc",
    "idna-host",
    "loopback",
];
/// A host the `url` crate refuses and some engines accept (engine.rs's
/// `rust_refuses_a_host_webkit_accepts` pins the first half). WebView2 and Fedora's WebKitGTK 2.54
/// accept it; the WebKitGTK on ubuntu-latest and the WebKit on macos-latest refuse it as an invalid
/// URL (pre-flight 2), so there it reaches neither Tauri's guard nor the network. The workflow maps
/// it to 127.0.0.1 in the hosts file, so a navigation or a load that gets through is a hit on the
/// leak listener.
const IDNA_HOST: &str = "xn--1ug.pm-probe.test";
/// The `relaxed` frame's plain loads (probe.js `OPEN_NET_PROBES`), each of which must have run: a
/// fetch, an image and a WebSocket, in the clear and over TLS, to loopback and to the IDNA host.
const OPEN_LOADS: [&str; 12] = [
    "open-fetch-loopback",
    "open-fetch-tls-loopback",
    "open-img-loopback",
    "open-img-tls-loopback",
    "open-ws-loopback",
    "open-ws-tls-loopback",
    "open-fetch-idna",
    "open-fetch-tls-idna",
    "open-img-idna",
    "open-img-tls-idna",
    "open-ws-idna",
    "open-ws-tls-idna",
];
/// The marks the `relaxed` frame asks for around its plain loads (probe.js `OPEN_NET_PROBES`): the
/// sinkhole's count at each is what the loads sent there.
const OPEN_MARKS: [&str; 2] = ["open-begin", "open-end"];
/// Where the sinkhole's port is remembered between runs on Windows, as PM remembers its own.
const PORT_FILE: &str = "pm-sealed-frame-sinkhole-port";
/// `1` for the dead-port run (see the header): no sinkhole, and the proxy on
/// [`engine::DEAD_PROXY_PORT`].
const DEAD_PORT_ENV: &str = "PM_SEALED_FRAME_DEAD_PORT";
/// What a dead-port run waits for a connection to the dead port to be answered. Windows can take a
/// couple of seconds to report a refused loopback connection.
const DEAD_PORT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

const PROBE_HTML: &str = include_str!("../../probe/index.html");
const PROBE_JS: &str = include_str!("../../probe/probe.js");
const PROBE_CHILD_JS: &str = include_str!("../../probe/probe-child.js");
const MODULE_JS: &str = include_str!("../../probe/module.js");
/// Not a font: the request the engine makes for it (its mode and Origin) is what is measured.
const FONT_BYTES: &[u8] = b"wOF2, but not a font";

struct Shared {
    start: Instant,
    events: Mutex<Vec<Value>>,
    tcp_port: u16,
    udp_port: u16,
    sinkhole: Option<Sinkhole>,
    /// Whether this is the dead-port run.
    dead_port: bool,
    /// Where the window's proxy points (`engine::proxy_port`).
    proxy_port: Option<u16>,
    /// The dead-port run's [`dead_port_answer`] before the window opened (`None` in the default
    /// run).
    dead_port_before: Option<String>,
    outcomes: Mutex<Vec<SwitchOutcome>>,
    /// Entry tokens not yet used, each bound to its frame's mod id.
    tickets: Mutex<HashMap<String, &'static str>>,
    /// How often each (mod id, file) was served: an entry page as a document, and every config.js.
    served: Mutex<HashMap<(&'static str, &'static str), u32>>,
    finished: AtomicBool,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

fn shared() -> &'static Shared {
    SHARED.get().expect("set at the top of main")
}

/// A panic elsewhere can't leave these half-written: read through a poisoned lock.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn record(kind: &str, detail: Value) {
    let ms = shared().start.elapsed().as_millis() as u64;
    lock(&shared().events).push(json!({ "ms": ms, "kind": kind, "detail": detail }));
}

/// A 64-hex token. Not PM's CSPRNG (that is runtime.rs's); a guess still has to beat std's
/// per-process random keys, and nothing here is a secret.
fn random_token() -> String {
    let mut out = String::with_capacity(64);
    for i in 0..4u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(shared().start.elapsed().as_nanos());
        let _ = write!(out, "{:016x}", h.finish());
    }
    out
}

fn variant_id(variant: &str) -> Option<&'static str> {
    VARIANTS
        .iter()
        .find(|(v, _)| *v == variant)
        .map(|(_, id)| *id)
}

/// `/<id>/app/<file>` for one of the two frames' ids.
fn split_path(path: &str) -> Option<(&'static str, &str)> {
    let (id, file) = path.strip_prefix('/')?.split_once('/')?;
    let file = file.strip_prefix(headers::APP_DIR)?;
    let id = VARIANTS.iter().find(|(_, i)| *i == id)?.1;
    Some((id, file))
}

fn entry_token(query: Option<&str>) -> Option<&str> {
    query?.strip_prefix("entry=")
}

// ---- The navigation guard's seam and the scheme ------------------------------------------------

/// PM's `is_pending_entry`: is `url` an entry URL a frame is about to load? Read-only, so asking
/// can't spend the token. The guard asks only about mod URLs, and only where it sees frames.
fn is_pending_entry(url: &Url) -> bool {
    let pending = (|| {
        let host = url.host_str()?;
        let plain = url.port().is_none() && url.username().is_empty() && url.password().is_none();
        if !plain || format!("{}://{host}", url.scheme()) != Platform::current().mod_origin() {
            return None;
        }
        let (id, file) = split_path(url.path())?;
        let token = entry_token(url.query())?;
        Some(file == "index.html" && lock(&shared().tickets).get(token) == Some(&id))
    })()
    .unwrap_or(false);
    record("guard", json!({ "url": url.as_str(), "pending": pending }));
    pending
}

/// Spend an entry token if it is this frame's: only then is its page a document. A token for the
/// other frame's id is burnt, as PM burns one (runtime.rs `take_ticket`).
fn take_ticket(query: Option<&str>, id: &'static str) -> bool {
    let Some(token) = entry_token(query) else {
        return false;
    };
    lock(&shared().tickets).remove(token) == Some(id)
}

fn refusal(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::builder().status(status);
    for (name, value) in headers::refusal_headers() {
        response = response.header(name, value);
    }
    response
        .body(Vec::new())
        .expect("a refusal is a valid response")
}

/// The `relaxed` frame's CSP: still sandboxed, every fetch target open. The four web schemes are
/// named as well as `*`, so no engine's reading of `*` (whether it covers `ws:`, or a scheme other
/// than the frame's own) decides a load.
fn relaxed_csp(id: &str) -> Option<String> {
    let base = headers::mod_source(Platform::current().mod_origin(), id)?;
    Some(format!(
        "sandbox allow-scripts; default-src * http: https: ws: wss: data: blob: 'unsafe-inline'; \
         connect-src * http: https: ws: wss: ipc: http://ipc.localhost; worker-src blob: *; \
         script-src {base}; frame-ancestors {}",
        Platform::current().app_origin()
    ))
}

/// The probe's per-run config, as a script on the mod's own path (the CSP allows no inline script).
fn config_js(id: &'static str) -> Vec<u8> {
    let s = shared();
    let variant = VARIANTS
        .iter()
        .find(|(_, i)| *i == id)
        .map_or("", |(v, _)| v);
    let other = VARIANTS
        .iter()
        .find(|(_, i)| *i != id)
        .map_or("", |(_, i)| i);
    let config = json!({
        "variant": variant,
        "id": id,
        "otherId": other,
        "tcpPort": s.tcp_port,
        "udpPort": s.udp_port,
        "modOrigin": Platform::current().mod_origin(),
        "appOrigin": Platform::current().app_origin(),
        "idnaHost": IDNA_HOST,
    });
    format!("self.PM_PROBE = {config};\n").into_bytes()
}

fn serve(label: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = req.uri();
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    record(
        "pmmod-request",
        json!({
            "webview": label,
            "method": req.method().as_str(),
            "uri": uri.to_string(),
            "origin": header("origin"),
            "secFetchMode": header("sec-fetch-mode"),
            "secFetchDest": header("sec-fetch-dest"),
        }),
    );
    // What PM's handler refuses before anything else (runtime.rs `serve_at`, `target_of_request`).
    if label != "main" {
        return refusal(StatusCode::NOT_FOUND);
    }
    if req.method() != Method::GET {
        return refusal(StatusCode::METHOD_NOT_ALLOWED);
    }
    if uri.scheme_str() != Some("pmmod") || uri.authority().map(|a| a.as_str()) != Some("localhost")
    {
        return refusal(StatusCode::NOT_FOUND);
    }
    let Some((id, file)) = split_path(uri.path()) else {
        return refusal(StatusCode::NOT_FOUND);
    };
    let (file, body): (&'static str, Vec<u8>) = match file {
        "index.html" => ("index.html", PROBE_HTML.as_bytes().to_vec()),
        "probe.js" => ("probe.js", PROBE_JS.as_bytes().to_vec()),
        "probe-child.js" => ("probe-child.js", PROBE_CHILD_JS.as_bytes().to_vec()),
        "module.js" => ("module.js", MODULE_JS.as_bytes().to_vec()),
        "config.js" => ("config.js", config_js(id)),
        "font.woff2" => ("font.woff2", FONT_BYTES.to_vec()),
        _ => return refusal(StatusCode::NOT_FOUND),
    };
    let is_entry = file == "index.html" && take_ticket(uri.query(), id);
    let platform = Platform::current();
    let path = format!("{}{file}", headers::APP_DIR);
    let Some(mut sent) = headers::file_headers(
        platform.mod_origin(),
        id,
        &path,
        &[platform.app_origin()],
        is_entry,
    ) else {
        return refusal(StatusCode::INTERNAL_SERVER_ERROR);
    };
    if id == VARIANTS[1].1 {
        let Some(policy) = relaxed_csp(id) else {
            return refusal(StatusCode::INTERNAL_SERVER_ERROR);
        };
        for (name, value) in sent.iter_mut() {
            if *name == "Content-Security-Policy" {
                *value = policy.clone();
            }
        }
    }
    if is_entry || file == "config.js" || file == "probe-child.js" {
        *lock(&shared().served).entry((id, file)).or_default() += 1;
    }
    let mut response = Response::builder().status(StatusCode::OK);
    for (name, value) in sent {
        response = response.header(name, value);
    }
    response
        .body(body)
        .unwrap_or_else(|_| refusal(StatusCode::INTERNAL_SERVER_ERROR))
}

// ---- Commands ----------------------------------------------------------------------------------

/// The decisive target: a forged call from a frame that lands here reached IPC.
#[tauri::command]
fn canary(via: Option<String>) -> &'static str {
    record("CANARY", json!({ "via": via }));
    "reached"
}

/// Mint a frame's one-time entry URL, as `mod_frame_open` does.
#[tauri::command]
fn harness_open(variant: String) -> Result<String, String> {
    let id = variant_id(&variant).ok_or("no such frame")?;
    let absorbed = shared().sinkhole.as_ref().map(Sinkhole::attempts);
    record(
        "open",
        json!({ "variant": variant, "sinkholeAbsorbedSoFar": absorbed }),
    );
    let token = random_token();
    lock(&shared().tickets).insert(token.clone(), id);
    Ok(format!(
        "{}/{id}/{}index.html?entry={token}",
        Platform::current().mod_origin(),
        headers::APP_DIR
    ))
}

/// Note the sinkhole's count at one of the `relaxed` frame's marks. The harness relays the frame's
/// ask and answers it only once this returns, and the frame waits for that answer before its first
/// load: so every connection a load makes is counted after `open-begin`.
#[tauri::command]
fn harness_mark(variant: String, mark: String) -> Result<(), String> {
    if variant != VARIANTS[1].0 || !OPEN_MARKS.contains(&mark.as_str()) {
        return Err("no such mark".into());
    }
    let absorbed = shared().sinkhole.as_ref().map(Sinkhole::attempts);
    let mut detail = json!({ "variant": variant, "mark": mark, "sinkholeAbsorbedSoFar": absorbed });
    if shared().dead_port {
        detail["failedConnectsSoFar"] = json!(failed_connects());
    }
    record("mark", detail);
    Ok(())
}

#[tauri::command]
fn harness_report(app: AppHandle, report: Value) {
    std::thread::spawn(move || {
        std::thread::sleep(SETTLE);
        let code = finish(report, "reported");
        app.exit(code);
        // If the event loop doesn't wind down, the verdict still stands.
        std::thread::sleep(Duration::from_secs(10));
        std::process::exit(code);
    });
}

// ---- The verdict -------------------------------------------------------------------------------

/// One probe result's `out`, from a run in the harness's report.
fn out<'a>(run: &'a Value, name: &str) -> Option<&'a Value> {
    run["results"]
        .as_array()?
        .iter()
        .find(|r| r["name"] == name)
        .map(|r| &r["out"])
}

fn starts_with(v: Option<&Value>, prefix: &str) -> bool {
    v.and_then(Value::as_str)
        .is_some_and(|s| s.starts_with(prefix))
}

/// Every reason the run fails; empty when it passes. `dead_port_after` is the dead-port run's
/// [`dead_port_answer`] at the end (`None` in the default run).
fn judge(
    outcome: &str,
    harness: &Value,
    events: &[Value],
    outcomes: &[SwitchOutcome],
    dead_port_after: Option<&str>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if outcome != "reported" {
        failures.push(format!("the harness never reported ({outcome})"));
    }
    if let Some(e) = harness["harnessError"].as_str() {
        failures.push(format!("the harness failed: {e}"));
    }
    // The same in both runs: no mode changes what counts as an escape or a leak.
    failures.extend(hit_failures(events));
    let s = shared();
    failures.extend(judge_switches(
        s.dead_port,
        Platform::current().expected_switches(),
        outcomes,
    ));
    if s.dead_port {
        failures.extend(judge_dead_port(
            s.proxy_port,
            s.dead_port_before.as_deref(),
            dead_port_after,
        ));
    }
    let served = lock(&shared().served).clone();
    for (variant, id) in VARIANTS {
        let children = served.get(&(id, "probe-child.js")).copied().unwrap_or(0);
        if children > 0 {
            failures.push(format!(
                "{variant}: a data: or blob: document ran a script in a sealed frame {children} \
                 time(s): main's frame-src should have refused the navigation"
            ));
        }
    }
    judge_navigations(harness, &served, &mut failures);
    judge_frames(harness, events, &served, s.dead_port, &mut failures);
    failures
}

/// Escapes and leaks: any call that reached the canary command, and any TCP connection or UDP
/// datagram that reached the loopback listeners. Judged the same in every run.
fn hit_failures(events: &[Value]) -> Vec<String> {
    let mut failures = Vec::new();
    let count = |kind: &str| events.iter().filter(|e| e["kind"] == kind).count();
    for (kind, what) in [
        (
            "CANARY",
            "call(s) from a frame reached Tauri's IPC (the canary command)",
        ),
        (
            "tcp",
            "TCP connection(s) reached the loopback leak listener",
        ),
        ("udp", "UDP datagram(s) reached the loopback STUN listener"),
    ] {
        let n = count(kind);
        if n > 0 {
            failures.push(format!("{n} {what}"));
        }
    }
    failures
}

/// The engine switches. The default run: every expected switch took. The dead-port run: the
/// sinkhole's switch failed, once, exactly as PM reports a sinkhole it couldn't bind
/// ([`engine::sinkhole_unbound`] of [`dead_port_bind_error`]), every other expected switch took,
/// and the engine reads as unsealed.
fn judge_switches(dead_port: bool, expected: &[&str], outcomes: &[SwitchOutcome]) -> Vec<String> {
    let verdict = engine::verdict(expected, outcomes);
    if !dead_port {
        return verdict
            .reasons
            .iter()
            .map(|reason| format!("the engine isn't sealed: {reason}"))
            .collect();
    }
    let mut failures = Vec::new();
    let want = engine::sinkhole_unbound(&dead_port_bind_error());
    let sinkhole: Vec<&SwitchOutcome> = outcomes
        .iter()
        .filter(|o| o.name == engine::SINKHOLE)
        .collect();
    if sinkhole != [&want] {
        let got: Vec<String> = sinkhole
            .iter()
            .map(|o| {
                let took = if o.took { "took" } else { "failed" };
                format!("{took}: {}", o.detail)
            })
            .collect();
        failures.push(format!(
            "the sinkhole switch should have failed once, as PM reports a sinkhole it couldn't \
             bind (\"{}\"), but it reported {got:?}: this run didn't measure the dead port",
            want.detail
        ));
    }
    let others: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|name| *name != engine::SINKHOLE)
        .collect();
    let rest: Vec<SwitchOutcome> = outcomes
        .iter()
        .filter(|o| o.name != engine::SINKHOLE)
        .cloned()
        .collect();
    for reason in engine::verdict(&others, &rest).reasons {
        failures.push(format!(
            "the engine isn't sealed but for the sinkhole: {reason}"
        ));
    }
    if verdict.sealed {
        failures.push("the engine reads as sealed with no sinkhole bound".to_owned());
    }
    failures
}

/// The dead-port run's own conditions: the window's proxy is [`engine::DEAD_PROXY_PORT`], and a
/// connection to it was refused before the window opened and again at the end, so nothing listened
/// there to take the loads.
fn judge_dead_port(
    proxy_port: Option<u16>,
    before: Option<&str>,
    after: Option<&str>,
) -> Vec<String> {
    let mut failures = Vec::new();
    if proxy_port != Some(engine::DEAD_PROXY_PORT) {
        failures.push(format!(
            "the window's proxy port is {proxy_port:?}, not the dead port ({})",
            engine::DEAD_PROXY_PORT
        ));
    }
    for (when, answer) in [("before the window opened", before), ("at the end", after)] {
        if answer != Some("refused") {
            failures.push(format!(
                "a connection to 127.0.0.1:{} {when} wasn't refused ({}): the run measures an \
                 engine whose proxy refuses, so nothing may listen there",
                engine::DEAD_PROXY_PORT,
                answer.unwrap_or("not tried")
            ));
        }
    }
    failures
}

/// The probe frames, `strict` and `relaxed`: each rendered and ran once, looks sealed, and (for
/// `relaxed`) ran its plain loads against the network wall.
fn judge_frames(
    harness: &Value,
    events: &[Value],
    served: &HashMap<(&'static str, &'static str), u32>,
    dead_port: bool,
    failures: &mut Vec<String>,
) {
    for (variant, id) in VARIANTS {
        if id == NAV_ID {
            continue;
        }
        let fail = &mut *failures;
        let mut expect = |ok: bool, what: &str| {
            if !ok {
                fail.push(format!("{variant}: {what}"));
            }
        };
        let pages = served.get(&(id, "index.html")).copied().unwrap_or(0);
        expect(
            pages == 1,
            &format!("its entry page rendered {pages} times, not once"),
        );
        let runs = served.get(&(id, "config.js")).copied().unwrap_or(0);
        expect(
            runs == 1,
            &format!("its page's scripts were fetched {runs} times, not once (a page without its token rendered)"),
        );
        let Some(run) = harness["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|r| r["variant"] == variant))
        else {
            expect(false, "the harness has no run for it");
            continue;
        };
        expect(run["connected"] == true, "the frame never got its port");
        expect(
            run["probeFinished"] == true,
            "the probe didn't run to the end",
        );
        let origin = out(run, "origin");
        expect(
            origin.is_some_and(|o| o["origin"] == "null"),
            "the frame's origin isn't opaque",
        );
        expect(
            out(run, "globals").is_some_and(|g| g["pmPreamble"] == true),
            "the WebRTC init script didn't run in the frame",
        );
        for name in ["parent-dom", "parent-tauri"] {
            expect(
                starts_with(out(run, name), "blocked"),
                &format!("{name} wasn't blocked"),
            );
        }
        if variant == "relaxed" {
            // The loads only measure the wall below the CSP if each ran and the CSP let it go:
            // a CSP refusal would answer for the sinkhole or the rule list and hide a hole in it.
            // That they never reached the listener is the TCP count's to say.
            for name in OPEN_LOADS {
                let got = out(run, name);
                expect(got.is_some(), &format!("{name} never ran"));
                // A load that completed reached a server, wherever its name led.
                let completed =
                    got.is_some_and(|v| v["ok"] == true || v == "LOADED" || v == "OPEN");
                expect(
                    !completed,
                    &format!("{name} completed: {}", got.unwrap_or(&Value::Null)),
                );
            }
            // Which hosts the engine's URL parser took (probe.js `open-parse`). Loopback must
            // parse, or no load measured anything. The IDNA host is up to the engine: Fedora's
            // WebKitGTK took it in round 2's local run, while the WebKitGTK on ubuntu-latest and the
            // WebKit on macos-latest refuse it (pre-flight 2), so there its loads fail before any
            // network. That is sealed for those loads, but they test nothing below.
            let parsed = out(run, "open-parse");
            expect(
                parsed.is_some_and(|p| p["loopback"] == true),
                &format!(
                    "the engine's URL parser refused the loopback host ({}), so no open load \
                     measured the wall",
                    parsed.unwrap_or(&Value::Null)
                ),
            );
            let owed = owed_by_open_loads(parsed);
            // On WebKitGTK they reach the sinkhole. Counted between the frame's two marks only: the
            // whole time the frame ran also holds its IPC probe's `http://ipc.localhost` fetch,
            // which its CSP allows and the proxy gets too (13 for 12 loads, in round 2's local
            // run). Fewer than one per load to a parsed host means something stopped some before
            // the proxy, and the job no longer tests the proxy for them. A retry can add a
            // connection, so this can't say which load each came from. Which hosts parsed is the
            // frame's word, and a frame that lied could only lower what is owed here: the TCP
            // count, which it can't fake, is what says none got through. Reported, not judged,
            // elsewhere: macOS has no sinkhole, and whether WebView2 sends a sandboxed frame's
            // loopback loads to the proxy is unmeasured.
            //
            // The dead-port run has no sinkhole to count at, so there each load is owed one TCP
            // connection the kernel saw fail before it was established (a refusal by the dead
            // port is one). The count is system-wide: a failed connection to any port from any
            // process adds to it, retries included. So a shortfall means something stopped some
            // loads before the dead port, and a pass shows only that at least that many TCP
            // connections failed while the loads ran, which fits the loads meeting the refusing
            // port; the leak listener's TCP count, which no frame can fake, is what says none got
            // out.
            if Platform::current() == Platform::Linux && dead_port {
                match failed_connects_by_open_loads(events) {
                    Some(failed) => expect(
                        failed >= owed,
                        &format!(
                            "only {failed} TCP connection(s) failed between its marks (the \
                             kernel's AttemptFails), not one per open load to a host the engine \
                             parsed ({owed} of {}): something stopped them before the dead port",
                            OPEN_LOADS.len()
                        ),
                    ),
                    None => expect(
                        false,
                        "its open loads weren't bracketed by failed-connection counts: no \
                         open-begin mark followed by an open-end one, or /proc/net/snmp \
                         unreadable at one",
                    ),
                }
            } else if Platform::current() == Platform::Linux {
                match absorbed_by_open_loads(events) {
                    Some(absorbed) => expect(
                        absorbed >= owed,
                        &format!(
                            "only {absorbed} connection(s) reached the sinkhole between its marks, \
                             not one per open load to a host the engine parsed ({owed} of {}): \
                             something stopped them before the proxy",
                            OPEN_LOADS.len()
                        ),
                    ),
                    None => expect(
                        false,
                        "its open loads weren't bracketed: no open-begin mark followed by an \
                         open-end one",
                    ),
                }
            }
            let refused = out(run, "open-csp");
            expect(
                refused.and_then(Value::as_array).is_some_and(Vec::is_empty),
                &format!(
                    "its CSP refused its open loads ({}), so they never reached the engine's \
                     network wall",
                    refused.map_or_else(|| "no report".to_owned(), Value::to_string)
                ),
            );
        }
        if variant != "strict" {
            continue;
        }
        // The positive control for the relaxed frame's empty `open-csp`: the same listener, in a
        // frame sealed the same way, heard this frame's CSP refuse a load to the leak listener (its
        // fetch-loopback, at the least). Matched by origin: an engine may strip a cross-origin
        // report to it.
        let leak_origin = format!("http://127.0.0.1:{}", shared().tcp_port);
        let heard = out(run, "csp-violations")
            .and_then(Value::as_array)
            .is_some_and(|list| {
                list.iter().any(|v| {
                    v["blocked"].as_str().is_some_and(|b| {
                        b == leak_origin
                            || b.strip_prefix(leak_origin.as_str())
                                .is_some_and(|rest| rest.starts_with('/'))
                    })
                })
            });
        expect(
            heard,
            &format!(
                "no report of its CSP refusing a load to {leak_origin} ({}): the frame's CSP \
                 violation reports don't arrive, so the relaxed frame's empty open-csp proves \
                 nothing",
                out(run, "csp-violations").map_or_else(
                    || "no report".to_owned(),
                    |v| v.to_string().chars().take(300).collect()
                )
            ),
        );
        let storage = out(run, "storage");
        expect(
            storage.is_some_and(|s| s["localStorage"] != "WORKS" && s["cookie"] != "WORKS"),
            "the frame has storage",
        );
        expect(starts_with(out(run, "eval"), "blocked"), "eval ran");
        expect(
            out(run, "window-open").is_some_and(|w| w != "OPENED"),
            "window.open opened a window",
        );
        expect(
            out(run, "webrtc").is_some_and(|w| w == "absent"),
            "WebRTC is in the frame",
        );
        expect(
            out(run, "webrtc-via-nosrc-iframe")
                .is_some_and(|w| !w.as_str().unwrap_or("").starts_with("constructed")),
            "WebRTC is reachable through a nested frame",
        );
    }
}

/// How many of `OPEN_LOADS` go to a host the engine's URL parser took (probe.js `open-parse`): each
/// is owed a connection to the sinkhole. A load is named for its host (`-loopback`, `-idna`); a host
/// the report doesn't say parsed is owed nothing.
fn owed_by_open_loads(parsed: Option<&Value>) -> u64 {
    OPEN_LOADS
        .iter()
        .filter(|name| {
            ["loopback", "idna"].iter().any(|target| {
                name.ends_with(&format!("-{target}")) && parsed.is_some_and(|p| p[*target] == true)
            })
        })
        .count() as u64
}

/// Connections the sinkhole absorbed between the `relaxed` frame's first `open-begin` mark and the
/// first `open-end` after it: its plain loads, and nothing it ran before them. `None` without a
/// sinkhole, or if either mark is missing.
fn absorbed_by_open_loads(events: &[Value]) -> Option<u64> {
    counted_by_open_loads(events, "sinkholeAbsorbedSoFar")
}

/// The dead-port run's [`absorbed_by_open_loads`]: TCP connections that failed before they were
/// established ([`failed_connects`]) between the same two marks. `None` off Linux, or if either
/// mark is missing or has no count.
fn failed_connects_by_open_loads(events: &[Value]) -> Option<u64> {
    counted_by_open_loads(events, "failedConnectsSoFar")
}

/// How far the count each mark noted as `field` rose between the `relaxed` frame's first
/// `open-begin` mark and the first `open-end` after it.
fn counted_by_open_loads(events: &[Value], field: &str) -> Option<u64> {
    let [begin, end] = OPEN_MARKS;
    let marks: Vec<&Value> = events
        .iter()
        .filter(|e| e["kind"] == "mark" && e["detail"]["variant"] == VARIANTS[1].0)
        .collect();
    let at = marks.iter().position(|e| e["detail"]["mark"] == begin)?;
    let to = marks[at..].iter().find(|e| e["detail"]["mark"] == end)?;
    let so_far = |e: &Value| e["detail"][field].as_u64();
    Some(so_far(to)?.saturating_sub(so_far(marks[at])?))
}

/// The navigation frames: one per target, in order, each with its port (so its own scripts ran
/// before it navigated), and each entry page rendered once with its scripts fetched once, so no
/// navigation brought in a page that rendered (an attachment, or another frame's entry, as a
/// document). Whether the navigation itself was refused is reported, not judged: an engine may
/// leave the frame alone or put an error page in it, and either is sealed.
fn judge_navigations(
    harness: &Value,
    served: &HashMap<(&'static str, &'static str), u32>,
    failures: &mut Vec<String>,
) {
    let runs = harness["navigations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let targets: Vec<&str> = runs.iter().filter_map(|r| r["to"].as_str()).collect();
    if targets != NAV_TARGETS {
        failures.push(format!(
            "nav: the harness navigated to {targets:?}, not {NAV_TARGETS:?}"
        ));
    }
    for run in &runs {
        if run["connected"] != true {
            failures.push(format!(
                "nav: the frame for {} never got its port",
                run["to"].as_str().unwrap_or("?")
            ));
        }
    }
    let n = NAV_TARGETS.len() as u32;
    for (file, what) in [
        ("index.html", "entry pages rendered"),
        ("config.js", "pages' scripts were fetched"),
    ] {
        let got = served.get(&(NAV_ID, file)).copied().unwrap_or(0);
        if got != n {
            failures.push(format!(
                "nav: {got} {what}, not one per target ({n}): a page without its token rendered, \
                 or a frame never loaded"
            ));
        }
    }
}

/// Write the report and say what happened. Returns the exit code. Runs once.
fn finish(harness: Value, outcome: &str) -> i32 {
    let s = shared();
    if s.finished.swap(true, Ordering::SeqCst) {
        return 1;
    }
    let events = lock(&s.events).clone();
    let outcomes = lock(&s.outcomes).clone();
    let verdict = engine::verdict(Platform::current().expected_switches(), &outcomes);
    let dead_port_after = s.dead_port.then(dead_port_answer);
    let failures = judge(
        outcome,
        &harness,
        &events,
        &outcomes,
        dead_port_after.as_deref(),
    );
    let count = |kind: &str| events.iter().filter(|e| e["kind"] == kind).count();
    let absorbed = s.sinkhole.as_ref().map_or(0, Sinkhole::attempts);
    let passed = failures.is_empty();
    let mut report = json!({
        "verdict": if passed { "pass" } else { "fail" },
        "failures": failures,
        "outcome": outcome,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "canaryHits": count("CANARY"),
        "tcpHits": count("tcp"),
        "udpHits": count("udp"),
        "sinkholeAbsorbed": absorbed,
        "sinkholeAbsorbedByOpenLoads": absorbed_by_open_loads(&events),
        "sinkholeOwedByOpenLoads": owed_by_open_loads(
            harness["runs"]
                .as_array()
                .and_then(|runs| runs.iter().find(|r| r["variant"] == VARIANTS[1].0))
                .and_then(|run| out(run, "open-parse"))
        ),
        "engineSealed": verdict.sealed,
        "switches": outcomes.iter().map(|o| json!({ "name": o.name, "took": o.took, "detail": o.detail })).collect::<Vec<_>>(),
        "events": events,
        "harness": harness,
    });
    if s.dead_port {
        report["deadPortRun"] = json!(true);
        report["proxyPort"] = json!(s.proxy_port);
        report["deadPortBefore"] = json!(s.dead_port_before);
        report["deadPortAfter"] = json!(dead_port_after);
        report["failedConnectsByOpenLoads"] = json!(failed_connects_by_open_loads(&events));
    }
    let path =
        std::env::var("PM_SEALED_FRAME_OUT").unwrap_or_else(|_| "sealed-frame-report.json".into());
    let written = serde_json::to_vec_pretty(&report)
        .map_err(|e| e.to_string())
        .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
    if let Err(e) = &written {
        eprintln!("sealed-frame: couldn't write the report to {path}: {e}");
    }
    summarise(&report);
    if passed && written.is_ok() {
        eprintln!("sealed-frame: PASS");
        0
    } else {
        eprintln!("sealed-frame: FAIL");
        1
    }
}

/// The report in a few lines for the CI log: the verdict, the counts, and the measurements that
/// decide later design questions (whether a mod's module script and font load, whether a busy frame
/// freezes PM).
fn summarise(report: &Value) {
    eprintln!(
        "sealed-frame: {}/{}: canary {}, tcp {}, udp {}, absorbed by the sinkhole {} ({} from the \
         relaxed frame's open loads), engine sealed {}",
        report["os"].as_str().unwrap_or("?"),
        report["arch"].as_str().unwrap_or("?"),
        report["canaryHits"],
        report["tcpHits"],
        report["udpHits"],
        report["sinkholeAbsorbed"],
        report["sinkholeAbsorbedByOpenLoads"],
        report["engineSealed"],
    );
    if report["deadPortRun"] == true {
        eprintln!(
            "  dead-port run: proxy port {}, a connection to it {} before and {} at the end; {} \
             TCP connection(s) failed between the relaxed frame's marks (Linux only), {} owed",
            report["proxyPort"],
            report["deadPortBefore"],
            report["deadPortAfter"],
            report["failedConnectsByOpenLoads"],
            report["sinkholeOwedByOpenLoads"],
        );
    }
    let harness = &report["harness"];
    eprintln!(
        "  page origin {}; {}",
        harness["pageOrigin"],
        harness["userAgent"].as_str().unwrap_or("no user agent")
    );
    for run in harness["runs"].as_array().into_iter().flatten() {
        eprintln!(
            "  {}: port {}, finished {}, loads {}, refused origins {}, busy {}",
            run["variant"].as_str().unwrap_or("?"),
            run["connected"],
            run["probeFinished"],
            run["loads"],
            run["refusedOrigins"],
            run["busy"],
        );
        for name in ["module-script", "font-own"] {
            if let Some(v) = out(run, name) {
                eprintln!("    {name}: {v}");
            }
        }
        for r in run["results"].as_array().into_iter().flatten() {
            let line = r["out"].to_string();
            let line: String = line.chars().take(140).collect();
            eprintln!("    {:<26} {line}", r["name"].as_str().unwrap_or("?"));
        }
    }
    for nav in harness["navigations"].as_array().into_iter().flatten() {
        eprintln!(
            "  navigate to {}: port {}, loads {} then {}{}",
            nav["to"].as_str().unwrap_or("?"),
            nav["connected"],
            nav["loadsBefore"],
            nav["loadsAfter"],
            if nav["loadsAfter"] == nav["loadsBefore"] {
                " (the frame kept its document)"
            } else {
                " (something committed in the frame: an error page, or a document)"
            },
        );
    }
    for e in report["events"].as_array().into_iter().flatten() {
        let kind = e["kind"].as_str().unwrap_or("");
        let shown = matches!(kind, "CANARY" | "tcp" | "udp" | "guard" | "open" | "mark")
            || (kind == "pmmod-request"
                && (e["detail"]["uri"].as_str().is_some_and(|u| {
                    u.contains("font") || u.contains("module") || u.contains("probe-child")
                })));
        if shown {
            let detail: String = e["detail"].to_string().chars().take(160).collect();
            eprintln!("  [{:>6} ms] {kind}: {detail}", e["ms"]);
        }
    }
    for f in report["failures"].as_array().into_iter().flatten() {
        eprintln!("  FAILURE: {}", f.as_str().unwrap_or("?"));
    }
}

// ---- The listeners -----------------------------------------------------------------------------

/// Accept connections on `listener` for the life of the run, logging each one's first line.
fn leak_listener(listener: TcpListener) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_millis(400)));
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let first: String = String::from_utf8_lossy(&buf[..n])
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            record("tcp", json!({ "firstLine": first, "bytes": n }));
            let _ = s.write_all(
                b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
            );
        }
    });
}

fn stun_listener(socket: UdpSocket) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((n, _)) = socket.recv_from(&mut buf) {
            record("udp", json!({ "bytes": n }));
        }
    });
}

// ---- The dead-port run -------------------------------------------------------------------------

/// Whether this is the dead-port run, from [`DEAD_PORT_ENV`]'s value: unset is the default run, `1`
/// the dead-port run, and anything else is refused rather than guessed at, so a typo can't turn a
/// dead-port run into a default one under the dead-port report's name.
fn dead_port_run(value: Option<&std::ffi::OsStr>) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(v) if v == "1" => Ok(true),
        Some(v) => Err(format!(
            "{DEAD_PORT_ENV} is {v:?}: set it to 1 for the dead-port run, or leave it unset"
        )),
    }
}

/// What every try at binding the sinkhole fails with in the dead-port run, and so what
/// [`engine::sinkhole_unbound`] reports. Made the same each time, so the verdict can expect it.
fn dead_port_bind_error() -> std::io::Error {
    std::io::Error::other(format!(
        "{DEAD_PORT_ENV} is set, so the sealed-frame app binds no sinkhole"
    ))
}

/// How a TCP connection to [`engine::DEAD_PROXY_PORT`] on loopback is answered: `refused` is the
/// only answer under which the dead-port run measures what it says, an engine whose proxy refuses.
fn dead_port_answer() -> String {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, engine::DEAD_PROXY_PORT));
    match TcpStream::connect_timeout(&addr, DEAD_PORT_PROBE_TIMEOUT) {
        Ok(_) => "accepted".to_owned(),
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => "refused".to_owned(),
        Err(e) => format!("{e} ({:?})", e.kind()),
    }
}

/// TCP connections the kernel has seen fail before they were established, for the whole system
/// (`AttemptFails` in `/proc/net/snmp`; a connection refused by the dead port adds one). Linux
/// only: `None` elsewhere, or if the file can't be read.
fn failed_connects() -> Option<u64> {
    if Platform::current() != Platform::Linux {
        return None;
    }
    attempt_fails(&std::fs::read_to_string("/proc/net/snmp").ok()?)
}

/// `AttemptFails` from `/proc/net/snmp`'s text: its first `Tcp:` line names the columns, the second
/// holds the values.
fn attempt_fails(snmp: &str) -> Option<u64> {
    let mut tcp = snmp.lines().filter(|line| line.starts_with("Tcp:"));
    let (names, values) = (tcp.next()?, tcp.next()?);
    let at = names
        .split_whitespace()
        .position(|name| name == "AttemptFails")?;
    values.split_whitespace().nth(at)?.parse().ok()
}

fn main() {
    let platform = Platform::current();
    let dead_port = match dead_port_run(std::env::var_os(DEAD_PORT_ENV).as_deref()) {
        Ok(dead_port) => dead_port,
        Err(why) => {
            eprintln!("sealed-frame: {why}");
            std::process::exit(2);
        }
    };
    if dead_port && !platform.uses_sinkhole() {
        eprintln!(
            "sealed-frame: {DEAD_PORT_ENV} is set, but this engine has no proxy, so there is no \
             dead-port run on it"
        );
        std::process::exit(2);
    }
    let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("the leak listener");
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("the STUN listener");
    // The sinkhole exactly as PM's EngineState::start binds it, where the platform has one: on
    // Windows on the port the last run used when that is free, so a run started right after another
    // usually gets the same WebView2 args (the workflow runs it twice back to back there). A bind
    // that still fails after its tries fails closed as PM's does: the sinkhole switch reports
    // failed (so a default run fails), nothing is remembered, and the window's proxy is the dead
    // port. The dead-port run takes that path on purpose: every try fails, so nothing is bound.
    let port_file = (platform == Platform::Windows).then(|| std::env::temp_dir().join(PORT_FILE));
    let remembered = port_file
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| engine::parse_remembered_port(&text));
    let bind = || {
        if dead_port {
            Err(dead_port_bind_error())
        } else {
            Sinkhole::bind_preferring(remembered)
        }
    };
    let (sinkhole, sinkhole_outcome) = if platform.uses_sinkhole() {
        match engine::bind_retrying(bind, std::thread::sleep) {
            Ok(s) => {
                if let Some(path) = &port_file {
                    let _ = std::fs::write(path, s.port().to_string());
                }
                (Some(s), Some(SwitchOutcome::took(engine::SINKHOLE)))
            }
            Err(e) => (None, Some(engine::sinkhole_unbound(&e))),
        }
    } else {
        (None, None)
    };
    // Where the window's proxy points, as in PM: the sinkhole's port, or the dead one. None only on
    // macOS, which has no proxy.
    let proxy_port = engine::proxy_port(platform, sinkhole.as_ref().map(Sinkhole::port));
    // Before any window: nothing may be listening on the dead port (judged at the end).
    let dead_port_before = dead_port.then(dead_port_answer);
    let shared_state = Shared {
        start: Instant::now(),
        events: Mutex::new(Vec::new()),
        tcp_port: tcp.local_addr().expect("a bound port").port(),
        udp_port: udp.local_addr().expect("a bound port").port(),
        sinkhole,
        dead_port,
        proxy_port,
        dead_port_before,
        outcomes: Mutex::new(sinkhole_outcome.into_iter().collect()),
        tickets: Mutex::new(HashMap::new()),
        served: Mutex::new(HashMap::new()),
        finished: AtomicBool::new(false),
    };
    if SHARED.set(shared_state).is_err() {
        unreachable!("main runs once");
    }
    leak_listener(tcp);
    stun_listener(udp);
    std::thread::spawn(|| {
        std::thread::sleep(WATCHDOG);
        finish(json!(null), "watchdog-timeout");
        std::process::exit(3);
    });

    // PM's WebView2 args: identical in every PM window, and carrying the proxy's port.
    let browser_args = engine::windows_browser_args(proxy_port, IS_DEV);
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("pmmod", |ctx, req, responder| {
            let label = ctx.webview_label().to_owned();
            responder.respond(serve(&label, &req));
        })
        .invoke_handler(tauri::generate_handler![
            canary,
            harness_open,
            harness_mark,
            harness_report
        ])
        .setup(move |app| {
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .ok_or("tauri.conf.json has no main window")?
                .clone();
            // As PM's build_main_window: on Windows a refused build is tried again while a previous
            // run's WebView2 winds down, each try hidden, and the window shown once its switches are
            // applied (on macOS, once the rule list's compile has started, before it is in place).
            let (hidden, show) = engine::hidden_until_built(&config);
            let window = engine::build_retrying(
                platform,
                || {
                    let builder = WebviewWindowBuilder::from_config(app.handle(), &hidden)?;
                    engine::seal_builder(builder, &browser_args, IS_DEV, is_pending_entry).build()
                },
                std::thread::sleep,
            )?;
            engine::apply_switches(&window, proxy_port, IS_DEV, |outcomes| {
                lock(&shared().outcomes).extend(outcomes);
            })?;
            if show {
                window.show()?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the sealed-frame app");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every expected switch of `platform` but the sinkhole, taken.
    fn others_took(expected: &[&'static str]) -> Vec<SwitchOutcome> {
        expected
            .iter()
            .copied()
            .filter(|name| *name != engine::SINKHOLE)
            .map(SwitchOutcome::took)
            .collect()
    }

    fn with(first: SwitchOutcome, rest: Vec<SwitchOutcome>) -> Vec<SwitchOutcome> {
        let mut all = vec![first];
        all.extend(rest);
        all
    }

    #[test]
    fn the_default_run_judges_the_switches_as_before() {
        for platform in [Platform::Linux, Platform::Windows, Platform::MacOs] {
            let expected = platform.expected_switches();
            let all: Vec<_> = expected.iter().copied().map(SwitchOutcome::took).collect();
            assert!(
                judge_switches(false, expected, &all).is_empty(),
                "{platform:?}"
            );
            // One that didn't take is the verdict's reason, as the run has always reported it.
            let mut one_failed = all.clone();
            one_failed[0] = SwitchOutcome::failed(expected[0], "no");
            assert_eq!(
                judge_switches(false, expected, &one_failed),
                engine::verdict(expected, &one_failed)
                    .reasons
                    .iter()
                    .map(|r| format!("the engine isn't sealed: {r}"))
                    .collect::<Vec<_>>(),
                "{platform:?}"
            );
            assert_eq!(judge_switches(false, expected, &one_failed).len(), 1);
        }
        // A sinkhole that couldn't be bound fails a default run.
        let expected = Platform::Linux.expected_switches();
        let unbound = with(
            engine::sinkhole_unbound(&dead_port_bind_error()),
            others_took(expected),
        );
        assert_eq!(judge_switches(false, expected, &unbound).len(), 1);
    }

    #[test]
    fn the_dead_port_run_wants_the_sinkhole_failed_as_pm_reports_it_and_the_rest_taken() {
        for platform in [Platform::Linux, Platform::Windows] {
            let expected = platform.expected_switches();
            let unbound = engine::sinkhole_unbound(&dead_port_bind_error());
            let good = with(unbound.clone(), others_took(expected));
            assert!(
                judge_switches(true, expected, &good).is_empty(),
                "{platform:?}: {:?}",
                judge_switches(true, expected, &good)
            );
            // The engine reads as unsealed then, as PM must report it.
            assert!(!engine::verdict(expected, &good).sealed);

            // A sinkhole that took: the run measured the sinkhole, not the dead port.
            let took = with(SwitchOutcome::took(engine::SINKHOLE), others_took(expected));
            let failures = judge_switches(true, expected, &took);
            assert!(
                failures
                    .iter()
                    .any(|f| f.starts_with("the sinkhole switch should have failed once")),
                "{failures:?}"
            );
            assert!(failures
                .iter()
                .any(|f| f == "the engine reads as sealed with no sinkhole bound"));

            // Failed, but not as PM reports a sinkhole it couldn't bind.
            let other = with(
                SwitchOutcome::failed(engine::SINKHOLE, "couldn't listen on loopback: other"),
                others_took(expected),
            );
            assert_eq!(judge_switches(true, expected, &other).len(), 1);

            // Never reported, or reported twice.
            assert_eq!(
                judge_switches(true, expected, &others_took(expected)).len(),
                1
            );
            let twice = with(unbound.clone(), good.clone());
            assert_eq!(judge_switches(true, expected, &twice).len(), 1);

            // Any other switch that didn't take, or never reported, still fails it.
            for (i, name) in others_took(expected).iter().map(|o| o.name).enumerate() {
                let mut failed = others_took(expected);
                failed[i] = SwitchOutcome::failed(name, "no");
                let failures = judge_switches(true, expected, &with(unbound.clone(), failed));
                assert_eq!(
                    failures,
                    [format!(
                        "the engine isn't sealed but for the sinkhole: {name} didn't take: no"
                    )],
                    "{platform:?}"
                );
                let mut missing = others_took(expected);
                missing.remove(i);
                assert_eq!(
                    judge_switches(true, expected, &with(unbound.clone(), missing)).len(),
                    1
                );
            }
        }
    }

    #[test]
    fn a_hit_fails_any_run() {
        assert!(hit_failures(&[]).is_empty());
        let others = [
            json!({ "kind": "mark", "detail": {} }),
            json!({ "kind": "pmmod-request", "detail": {} }),
        ];
        assert!(hit_failures(&others).is_empty());
        for (kind, said) in [
            (
                "tcp",
                "1 TCP connection(s) reached the loopback leak listener",
            ),
            (
                "udp",
                "1 UDP datagram(s) reached the loopback STUN listener",
            ),
            (
                "CANARY",
                "1 call(s) from a frame reached Tauri's IPC (the canary command)",
            ),
        ] {
            assert_eq!(
                hit_failures(&[json!({ "kind": kind, "detail": {} })]),
                [said]
            );
        }
    }

    #[test]
    fn the_dead_port_must_be_the_proxy_and_refuse_before_and_after() {
        let dead = Some(engine::DEAD_PROXY_PORT);
        assert!(judge_dead_port(dead, Some("refused"), Some("refused")).is_empty());
        assert_eq!(
            judge_dead_port(Some(4321), Some("refused"), Some("refused")).len(),
            1
        );
        assert_eq!(
            judge_dead_port(None, Some("refused"), Some("refused")).len(),
            1
        );
        for answer in [Some("accepted"), Some("timed out (TimedOut)"), None] {
            assert_eq!(judge_dead_port(dead, answer, Some("refused")).len(), 1);
            assert_eq!(judge_dead_port(dead, Some("refused"), answer).len(), 1);
        }
    }

    #[test]
    fn only_1_starts_a_dead_port_run() {
        use std::ffi::OsStr;
        assert_eq!(dead_port_run(None), Ok(false));
        assert_eq!(dead_port_run(Some(OsStr::new("1"))), Ok(true));
        for refused in ["", "0", "true", "yes", " 1", "1 "] {
            assert!(
                dead_port_run(Some(OsStr::new(refused))).is_err(),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn attempt_fails_is_read_from_the_tcp_lines() {
        // Linux 7.2's layout, from this machine.
        let snmp = "Ip: Forwarding DefaultTTL\nIp: 1 64\n\
            Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails \
            EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts InCsumErrors\n\
            Tcp: 1 200 120000 -1 52937 3008 142 3801 31 2847853 2675960 2699 3 11732 0\n\
            Udp: InDatagrams NoPorts\nUdp: 5 6\n";
        assert_eq!(attempt_fails(snmp), Some(142));
        assert_eq!(attempt_fails("Tcp: ActiveOpens AttemptFails\n"), None);
        assert_eq!(attempt_fails("Tcp: ActiveOpens\nTcp: 4\n"), None);
        assert_eq!(attempt_fails("Tcp: AttemptFails\nTcp: x\n"), None);
        assert_eq!(attempt_fails(""), None);
        if Platform::current() == Platform::Linux {
            assert!(failed_connects().is_some());
        } else {
            assert_eq!(failed_connects(), None);
        }
    }

    fn mark(variant: &str, name: &str, field: &str, n: Option<u64>) -> Value {
        json!({ "kind": "mark", "detail": { "variant": variant, "mark": name, field: n } })
    }

    #[test]
    fn the_failed_connects_are_counted_between_the_relaxed_frames_marks() {
        let f = "failedConnectsSoFar";
        let events = [
            mark("relaxed", "open-end", f, Some(1)),
            mark("strict", "open-begin", f, Some(2)),
            mark("relaxed", "open-begin", f, Some(10)),
            mark("relaxed", "open-end", f, Some(17)),
            mark("relaxed", "open-end", f, Some(30)),
        ];
        assert_eq!(failed_connects_by_open_loads(&events), Some(7));
        // The default run's count is a different field: none of it is read here.
        assert_eq!(absorbed_by_open_loads(&events), None);
        // A mark that couldn't read the count, or a missing mark, is no count.
        let unread = [
            mark("relaxed", "open-begin", f, None),
            mark("relaxed", "open-end", f, Some(17)),
        ];
        assert_eq!(failed_connects_by_open_loads(&unread), None);
        assert_eq!(
            failed_connects_by_open_loads(&[mark("relaxed", "open-begin", f, Some(1))]),
            None
        );
    }

    /// The dead-port run's one check, on Linux, that the `relaxed` frame's open loads met the dead
    /// port, through `judge_frames` itself: between its marks the kernel's failed TCP connections
    /// must have risen by one per open load to a host the engine parsed, and a mark without the
    /// count fails it. The harness holds only a `relaxed` run that passes everything else, and
    /// nothing was served, so every case shares the same other failures and only this check's
    /// differ.
    #[test]
    fn a_dead_port_run_owes_a_failed_connection_per_open_load_between_the_marks() {
        if Platform::current() != Platform::Linux {
            return;
        }
        let mut results: Vec<Value> = OPEN_LOADS
            .iter()
            .map(|name| json!({ "name": name, "out": "error" }))
            .collect();
        results.extend([
            // Loopback parsed and the IDNA host didn't, as on ubuntu-latest: 6 of the 12 are owed.
            json!({ "name": "open-parse", "out": { "loopback": true, "idna": false } }),
            json!({ "name": "open-csp", "out": [] }),
            // What a sealed frame shows, so the run fails nothing else of its own.
            json!({ "name": "origin", "out": { "origin": "null" } }),
            json!({ "name": "globals", "out": { "pmPreamble": true } }),
            json!({ "name": "parent-dom", "out": "blocked: SecurityError" }),
            json!({ "name": "parent-tauri", "out": "blocked: undefined" }),
        ]);
        let harness = json!({ "runs": [{
            "variant": "relaxed",
            "connected": true,
            "probeFinished": true,
            "results": results,
        }] });
        assert_eq!(
            owed_by_open_loads(out(&harness["runs"][0], "open-parse")),
            6
        );
        // A mark as `harness_mark` records one in a dead-port run: no sinkhole to count at, and
        // the kernel's count, or null where /proc/net/snmp couldn't be read.
        let mark = |ms: u64, name: &str, failed: Option<u64>| {
            json!({ "ms": ms, "kind": "mark", "detail": {
                "variant": "relaxed",
                "mark": name,
                "sinkholeAbsorbedSoFar": null,
                "failedConnectsSoFar": failed,
            } })
        };
        let judged = |events: &[Value]| {
            let mut failures = Vec::new();
            judge_frames(&harness, events, &HashMap::new(), true, &mut failures);
            failures
        };
        let served_none = |variant: &str| {
            [
                format!("{variant}: its entry page rendered 0 times, not once"),
                format!(
                    "{variant}: its page's scripts were fetched 0 times, not once (a page without \
                     its token rendered)"
                ),
            ]
        };
        let mut others: Vec<String> = served_none("strict").into();
        others.push("strict: the harness has no run for it".to_owned());
        others.extend(served_none("relaxed"));

        // Risen by exactly what is owed: nothing from this check.
        let met = [
            mark(10, "open-begin", Some(100)),
            mark(20, "open-end", Some(106)),
        ];
        assert_eq!(judged(&met), others);

        // One short.
        let short = [
            mark(10, "open-begin", Some(100)),
            mark(20, "open-end", Some(105)),
        ];
        let mut want = others.clone();
        want.push(
            "relaxed: only 5 TCP connection(s) failed between its marks (the kernel's \
             AttemptFails), not one per open load to a host the engine parsed (6 of 12): \
             something stopped them before the dead port"
                .to_owned(),
        );
        assert_eq!(judged(&short), want);

        // A mark without the count, or no open-end mark at all.
        let bracket = "relaxed: its open loads weren't bracketed by failed-connection counts: no \
                       open-begin mark followed by an open-end one, or /proc/net/snmp unreadable \
                       at one";
        let mut want = others.clone();
        want.push(bracket.to_owned());
        for events in [
            vec![
                mark(10, "open-begin", None),
                mark(20, "open-end", Some(106)),
            ],
            vec![
                mark(10, "open-begin", Some(100)),
                mark(20, "open-end", None),
            ],
            vec![mark(10, "open-begin", Some(100))],
        ] {
            assert_eq!(judged(&events), want, "{events:?}");
        }
    }

    /// The whole verdict of a dead-port run, through `judge` itself: a leak fails it, and so does a
    /// sinkhole that took, while neither failure is there without its cause. The harness's report
    /// is empty, so the run fails for that too; only these failures are looked for.
    #[test]
    fn a_dead_port_run_fails_on_a_leak_and_on_a_sinkhole_that_took() {
        let platform = Platform::current();
        if !platform.uses_sinkhole() {
            return;
        }
        let state = Shared {
            start: Instant::now(),
            events: Mutex::new(Vec::new()),
            tcp_port: 0,
            udp_port: 0,
            sinkhole: None,
            dead_port: true,
            proxy_port: engine::proxy_port(platform, None),
            dead_port_before: Some("refused".to_owned()),
            outcomes: Mutex::new(Vec::new()),
            tickets: Mutex::new(HashMap::new()),
            served: Mutex::new(HashMap::new()),
            finished: AtomicBool::new(false),
        };
        assert!(SHARED.set(state).is_ok(), "only this test sets SHARED");
        let expected = platform.expected_switches();
        let unbound = with(
            engine::sinkhole_unbound(&dead_port_bind_error()),
            others_took(expected),
        );
        let took = with(SwitchOutcome::took(engine::SINKHOLE), others_took(expected));
        let leak = [json!({ "ms": 1, "kind": "tcp", "detail": {} })];
        let harness = json!({});
        let ours = |f: &String| {
            f.contains("leak listener")
                || f.contains("sinkhole switch")
                || f.contains("reads as sealed")
                || f.contains("but for the sinkhole")
                || f.contains("wasn't refused")
                || f.contains("proxy port")
        };

        let clean = judge("reported", &harness, &[], &unbound, Some("refused"));
        assert!(!clean.iter().any(ours), "{clean:?}");
        assert!(!clean.is_empty(), "the empty harness report fails it");

        let leaked = judge("reported", &harness, &leak, &unbound, Some("refused"));
        assert!(leaked
            .iter()
            .any(|f| f == "1 TCP connection(s) reached the loopback leak listener"));

        let sinkhole_took = judge("reported", &harness, &[], &took, Some("refused"));
        assert!(sinkhole_took
            .iter()
            .any(|f| f.starts_with("the sinkhole switch should have failed once")));

        let answered = judge("reported", &harness, &[], &unbound, Some("accepted"));
        assert!(answered
            .iter()
            .any(|f| f.contains("at the end wasn't refused")));
    }
}
